//! The deployment ledger.
//!
//! Authority in a deployed system is a `DynamoDB` table with a conditional put on
//! a monotonic fence, because Git cannot enforce monotonicity across concurrent
//! workflows. That table does not exist in this rewrite and nothing here makes
//! an AWS call: the store is behind a trait, and the only implementation is the
//! append-only JSONL mirror the private repository keeps for audit. The fence
//! semantics are identical, so the `DynamoDB` adapter is a store, not a rewrite.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::canon;
use crate::error::{Exit, Result, ToolError, Violation, io};

/// The deployment state machine.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum State {
    /// Admitted and planned, nothing applied.
    Planned,
    /// Applying migrations.
    Migrating,
    /// Applying infrastructure and code.
    Deploying,
    /// Running post-apply evidence.
    Verifying,
    /// Committed.
    Committed,
    /// Failed from migrating, deploying or verifying.
    Failed,
    /// Desired and actual state disagree.
    Diverged,
    /// Divergence closed.
    Reconciled,
}

impl State {
    /// Whether a transition is legal.
    #[must_use]
    pub const fn can_transition_to(self, next: Self) -> bool {
        matches!(
            (self, next),
            (
                Self::Planned,
                Self::Migrating | Self::Deploying | Self::Failed
            ) | (Self::Migrating, Self::Deploying | Self::Failed)
                | (Self::Deploying, Self::Verifying | Self::Failed)
                | (Self::Verifying, Self::Committed | Self::Failed)
                | (Self::Committed, Self::Diverged)
                | (Self::Diverged, Self::Reconciled | Self::Failed)
        )
    }
}

/// One unit's intended and observed digest.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct UnitState {
    /// Unit id.
    pub id: String,
    /// What the manifest names.
    pub expected_digest: String,
    /// What the plane holds.
    pub actual_digest: String,
}

/// Migration heads across a deployment.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MigrationRecord {
    /// Head before.
    pub head_before: String,
    /// Head after.
    pub head_after: String,
    /// The receipt the schema admin produced.
    pub receipt_digest: String,
}

/// `aex.deployment-ledger-entry.v1`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LedgerEntry {
    /// Schema discriminator.
    pub schema: String,
    /// Plane.
    pub plane: String,
    /// Region, or `central`.
    pub region: String,
    /// Monotonic fence.
    pub fence: u64,
    /// State.
    pub state: State,
    /// Desired release.
    pub desired_release_id: String,
    /// Previous release.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub previous_release_id: Option<String>,
    /// Binding digest.
    pub binding_digest: String,
    /// Previous binding digest.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub previous_binding_digest: Option<String>,
    /// Per-unit state.
    #[serde(default)]
    pub units: Vec<UnitState>,
    /// The saved plan applied.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plan_digest: Option<String>,
    /// Migration heads.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub migrations: Option<MigrationRecord>,
    /// Evidence referenced.
    #[serde(default)]
    pub evidence: Vec<BTreeMap<String, String>>,
    /// The workflow that wrote the entry.
    pub workflow: BTreeMap<String, String>,
    /// Human decision recorded with the entry.
    pub decision: String,
    /// Start.
    pub started_at: String,
    /// End.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ended_at: Option<String>,
}

/// The append-only store.
pub trait LedgerStore {
    /// Every entry for a plane, in fence order.
    ///
    /// # Errors
    /// Returns a usage error when the store cannot be read.
    fn list(&self, plane: &str) -> Result<Vec<LedgerEntry>>;

    /// Append one entry, rejecting a fence that is not strictly greater than
    /// the last one for that plane and region.
    ///
    /// # Errors
    /// Returns [`Exit::LedgerFenceConflict`] on a stale or repeated fence.
    fn append(&self, entry: &LedgerEntry) -> Result<()>;
}

/// A JSONL file, one entry per line, append-only.
#[derive(Debug, Clone)]
pub struct JsonlLedger {
    path: PathBuf,
}

impl JsonlLedger {
    /// Open, or prepare to create, a JSONL ledger at a path.
    #[must_use]
    pub fn new(path: &Path) -> Self {
        Self {
            path: path.to_path_buf(),
        }
    }
}

impl LedgerStore for JsonlLedger {
    fn list(&self, plane: &str) -> Result<Vec<LedgerEntry>> {
        if !self.path.exists() {
            return Ok(Vec::new());
        }
        let text = std::fs::read_to_string(&self.path)
            .map_err(|err| io(&self.path.display().to_string(), &err))?;
        let mut entries = Vec::new();
        for (number, line) in text.lines().enumerate() {
            if line.trim().is_empty() {
                continue;
            }
            let entry: LedgerEntry = serde_json::from_str(line).map_err(|err| {
                ToolError::single(
                    Exit::StateDiverged,
                    "ledger-unparseable",
                    format!("{}:{}: {err}", self.path.display(), number + 1),
                )
            })?;
            if entry.plane == plane {
                entries.push(entry);
            }
        }
        entries.sort_by_key(|entry| entry.fence);
        Ok(entries)
    }

    fn append(&self, entry: &LedgerEntry) -> Result<()> {
        use std::io::Write as _;

        let existing = self.list(&entry.plane)?;
        if let Some(last) = existing
            .iter()
            .rfind(|candidate| candidate.region == entry.region)
        {
            if entry.fence <= last.fence {
                return Err(ToolError::single(
                    Exit::LedgerFenceConflict,
                    "ledger-fence-conflict",
                    format!(
                        "fence {} does not advance past {} for {}/{}",
                        entry.fence, last.fence, entry.plane, entry.region
                    ),
                ));
            }
            if !last.state.can_transition_to(entry.state)
                && last.desired_release_id == entry.desired_release_id
            {
                return Err(ToolError::single(
                    Exit::LedgerFenceConflict,
                    "ledger-illegal-transition",
                    format!(
                        "{:?} -> {:?} is not a legal deployment transition",
                        last.state, entry.state
                    ),
                ));
            }
        } else if entry.state != State::Planned {
            return Err(ToolError::single(
                Exit::LedgerFenceConflict,
                "ledger-illegal-transition",
                format!(
                    "the first entry for {}/{} is {:?}; a deployment starts PLANNED",
                    entry.plane, entry.region, entry.state
                ),
            ));
        }
        let line = format!("{}\n", canon::to_string(entry)?);
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|err| io(&parent.display().to_string(), &err))?;
        }
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
            .map_err(|err| io(&self.path.display().to_string(), &err))?;
        file.write_all(line.as_bytes())
            .map_err(|err| io(&self.path.display().to_string(), &err))
    }
}

/// What a plane actually holds, read back from the platform.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Readback {
    /// Plane.
    pub plane: String,
    /// Region.
    pub region: String,
    /// Binding digest in force.
    pub binding_digest: String,
    /// Observed unit digests.
    pub units: BTreeMap<String, String>,
}

/// Compare desired binding, ledger and readback.
///
/// # Errors
/// Returns [`Exit::StateDiverged`] on any disagreement.
pub fn verify_convergence(
    entries: &[LedgerEntry],
    binding_digest: &str,
    actual: &Readback,
) -> Result<()> {
    let Some(latest) = entries.iter().rfind(|entry| entry.region == actual.region) else {
        return Err(ToolError::single(
            Exit::StateDiverged,
            "ledger-no-entry",
            format!(
                "the ledger holds no entry for {}/{}, so nothing states what should be \
                 running",
                actual.plane, actual.region
            ),
        ));
    };
    let mut violations = Vec::new();
    if latest.binding_digest != binding_digest {
        violations.push(Violation::new(
            "state-binding-divergent",
            format!(
                "the ledger records binding `{}`; the desired binding is `{binding_digest}`",
                latest.binding_digest
            ),
        ));
    }
    if actual.binding_digest != binding_digest {
        violations.push(Violation::new(
            "state-binding-divergent",
            format!(
                "the plane reports binding `{}`; the desired binding is `{binding_digest}`",
                actual.binding_digest
            ),
        ));
    }
    for unit in &latest.units {
        match actual.units.get(&unit.id) {
            None => violations.push(Violation::new(
                "state-unit-absent",
                format!(
                    "unit `{}` is in the ledger and absent from the plane",
                    unit.id
                ),
            )),
            Some(observed) if observed != &unit.expected_digest => {
                violations.push(Violation::new(
                    "state-unit-divergent",
                    format!(
                        "unit `{}` runs `{observed}`; the ledger expects `{}`",
                        unit.id, unit.expected_digest
                    ),
                ));
            }
            Some(_) => {}
        }
    }
    for observed in actual.units.keys() {
        if !latest.units.iter().any(|unit| &unit.id == observed) {
            violations.push(Violation::new(
                "state-unit-unexpected",
                format!("unit `{observed}` runs in the plane and is in no ledger entry"),
            ));
        }
    }
    if violations.is_empty() {
        Ok(())
    } else {
        Err(ToolError::many(Exit::StateDiverged, violations))
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::{
        JsonlLedger, LedgerEntry, LedgerStore, Readback, State, UnitState, verify_convergence,
    };

    fn entry(fence: u64, state: State) -> LedgerEntry {
        LedgerEntry {
            schema: "aex.deployment-ledger-entry.v1".to_owned(),
            plane: "dev".to_owned(),
            region: "eu-west-1".to_owned(),
            fence,
            state,
            desired_release_id: "sha256:aa".to_owned(),
            previous_release_id: None,
            binding_digest: "sha256:bb".to_owned(),
            previous_binding_digest: None,
            units: vec![UnitState {
                id: "regional-session-api".to_owned(),
                expected_digest: "sha256:cc".to_owned(),
                actual_digest: "sha256:cc".to_owned(),
            }],
            plan_digest: None,
            migrations: None,
            evidence: Vec::new(),
            workflow: BTreeMap::from([("runId".to_owned(), "1".to_owned())]),
            decision: "routine".to_owned(),
            started_at: "2026-08-01T00:00:00Z".to_owned(),
            ended_at: None,
        }
    }

    #[test]
    fn a_repeated_or_stale_fence_conflicts() {
        let temp = tempfile::tempdir().unwrap();
        let ledger = JsonlLedger::new(&temp.path().join("ledger.jsonl"));
        ledger.append(&entry(1, State::Planned)).unwrap();
        ledger.append(&entry(2, State::Deploying)).unwrap();
        let err = ledger.append(&entry(2, State::Verifying)).unwrap_err();
        assert_eq!(err.exit.code(), 50);
        assert_eq!(err.rules(), vec!["ledger-fence-conflict"]);
        let err = ledger.append(&entry(1, State::Verifying)).unwrap_err();
        assert_eq!(err.exit.code(), 50);
    }

    #[test]
    fn an_illegal_transition_is_rejected() {
        let temp = tempfile::tempdir().unwrap();
        let ledger = JsonlLedger::new(&temp.path().join("ledger.jsonl"));
        ledger.append(&entry(1, State::Planned)).unwrap();
        let err = ledger.append(&entry(2, State::Committed)).unwrap_err();
        assert_eq!(err.rules(), vec!["ledger-illegal-transition"]);
    }

    #[test]
    fn a_deployment_starts_planned() {
        let temp = tempfile::tempdir().unwrap();
        let ledger = JsonlLedger::new(&temp.path().join("ledger.jsonl"));
        let err = ledger.append(&entry(1, State::Deploying)).unwrap_err();
        assert_eq!(err.rules(), vec!["ledger-illegal-transition"]);
    }

    #[test]
    fn the_full_lifecycle_is_legal() {
        let temp = tempfile::tempdir().unwrap();
        let ledger = JsonlLedger::new(&temp.path().join("ledger.jsonl"));
        for (fence, state) in [
            (1, State::Planned),
            (2, State::Migrating),
            (3, State::Deploying),
            (4, State::Verifying),
            (5, State::Committed),
        ] {
            ledger.append(&entry(fence, state)).unwrap();
        }
        assert_eq!(ledger.list("dev").unwrap().len(), 5);
        assert!(ledger.list("prd").unwrap().is_empty());
    }

    #[test]
    fn a_digest_the_plane_does_not_hold_is_divergence() {
        let entries = vec![entry(1, State::Planned)];
        let actual = Readback {
            plane: "dev".to_owned(),
            region: "eu-west-1".to_owned(),
            binding_digest: "sha256:bb".to_owned(),
            units: BTreeMap::from([("regional-session-api".to_owned(), "sha256:dd".to_owned())]),
        };
        let err = verify_convergence(&entries, "sha256:bb", &actual).unwrap_err();
        assert_eq!(err.exit.code(), 51);
        assert!(err.rules().contains(&"state-unit-divergent"));
    }

    #[test]
    fn a_unit_running_that_nobody_deployed_is_divergence() {
        let entries = vec![entry(1, State::Planned)];
        let actual = Readback {
            plane: "dev".to_owned(),
            region: "eu-west-1".to_owned(),
            binding_digest: "sha256:bb".to_owned(),
            units: BTreeMap::from([
                ("regional-session-api".to_owned(), "sha256:cc".to_owned()),
                ("ghost".to_owned(), "sha256:ee".to_owned()),
            ]),
        };
        let err = verify_convergence(&entries, "sha256:bb", &actual).unwrap_err();
        assert!(err.rules().contains(&"state-unit-unexpected"));
    }

    #[test]
    fn a_converged_plane_verifies() {
        let entries = vec![entry(1, State::Planned)];
        let actual = Readback {
            plane: "dev".to_owned(),
            region: "eu-west-1".to_owned(),
            binding_digest: "sha256:bb".to_owned(),
            units: BTreeMap::from([("regional-session-api".to_owned(), "sha256:cc".to_owned())]),
        };
        verify_convergence(&entries, "sha256:bb", &actual).unwrap();
    }
}
