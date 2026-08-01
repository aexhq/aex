//! The janitor: reclamation from tags alone.
//!
//! OD-35 makes release tests provision in `prd` deliberately. OD-36 is the
//! consequence: nothing may provision in `prd` that this cannot reclaim knowing
//! only a plane and a credential - no test process, no local ledger file, no
//! prior knowledge of the run. `aex_test_harness::CleanupLedger` closes the
//! normal path; only a sweep by tag survives `SIGKILL`, a runner eviction and a
//! `cargo clean`.
//!
//! # The guard
//!
//! A janitor that can delete a production resource is a worse defect than the
//! residue it removes, so the interesting property here is what it *refuses*.
//! A discovered resource is admitted only when **all** of these hold:
//!
//! 1. it carries `aex:test-synthetic` with exactly the policy's marker value;
//! 2. its `aex:test-run-id` has the exact `tr_` + 32 lowercase hex shape;
//! 3. its `aex:test-lane` is one of the closed lane set;
//! 4. its `aex:test-owner` is one of the closed owner set;
//! 5. its `aex:test-expires-at` parses as RFC 3339;
//! 6. its kind is a declared `[janitor.resource]` row with a discovery route;
//! 7. for a kind AEX names, its identity sits inside *that run's* namespace;
//! 8. the current instant is past `expires_at + residue_grace`.
//!
//! Rule 7 is what makes the tag set insufficient on its own: forging the tags
//! onto a production resource is not enough, because the resource would have to
//! be renamed into `aextest-tr_…-` as well.
//!
//! The refusal is enforced structurally, not by discipline. [`Reclaimer`] takes
//! an [`Admitted`], whose fields are private and whose only constructor is
//! [`admit`]. No adapter in this crate or any other can manufacture one for a
//! resource the guard refused; the compiler is the guard's enforcement, and the
//! hostile cases in `tests/janitor.rs` are its evidence.
//!
//! # What this module does not do
//!
//! It makes no AWS call and links no cloud SDK. [`sweep`] takes an
//! [`Inventory`] and a [`Reclaimer`]; producing the first and implementing the
//! second for a real plane needs the credentialed adapter OD-07 puts out of
//! scope for this run. The engine, the guard, the order and the report are
//! complete and tested; the adapter is one trait implementation away and is
//! named as an owed gap rather than faked.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;

use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

use aex_workspace_check::policy::{Janitor as JanitorPolicy, Policy};

use crate::admit::Plane;
use crate::error::{Exit, Result, ToolError};

/// The schema discriminator every sweep report carries.
pub const SWEEP_SCHEMA: &str = "aex.janitor-sweep.v1";
/// The schema discriminator every inventory document carries.
pub const INVENTORY_SCHEMA: &str = "aex.janitor-inventory.v1";

/// One resource the plane offered, exactly as discovered.
///
/// Deliberately untyped and untrusted. An inventory adapter does not pre-filter
/// by tag: a guard that is only ever offered safe input is not a guard, and the
/// hostile cases depend on being able to hand this module a production
/// resource.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DiscoveredResource {
    /// The kind name as the adapter classified it. May be unknown.
    pub kind: String,
    /// The identity the adapter would delete by.
    pub identity: String,
    /// Every tag the resource carries, unfiltered.
    #[serde(default)]
    pub tags: BTreeMap<String, String>,
}

/// A plane inventory: what a credentialed discovery pass found.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Inventory {
    /// Schema discriminator.
    pub schema: String,
    /// Which plane it was taken from.
    pub plane: Plane,
    /// Everything found.
    #[serde(default, rename = "resource")]
    pub resources: Vec<DiscoveredResource>,
}

impl Inventory {
    /// Parse an inventory document.
    ///
    /// # Errors
    /// Returns [`Exit::Usage`] when the document does not parse or carries the
    /// wrong schema discriminator.
    pub fn parse(text: &str) -> Result<Self> {
        let inventory: Self = serde_json::from_str(text).map_err(|err| {
            ToolError::single(
                Exit::Usage,
                "janitor-inventory-unparseable",
                format!("cannot read the janitor inventory: {err}"),
            )
        })?;
        if inventory.schema != INVENTORY_SCHEMA {
            return Err(ToolError::single(
                Exit::Usage,
                "janitor-inventory-schema",
                format!(
                    "inventory declares schema `{}`, expected `{INVENTORY_SCHEMA}`",
                    inventory.schema
                ),
            ));
        }
        Ok(inventory)
    }
}

/// Why the janitor refused to touch a resource.
///
/// Every variant is a *refusal to delete*. Whether the refusal also means real
/// residue is a separate fact, carried by [`RefusedRecord::is_residue`]: a
/// production resource the janitor declined is not the test suite's leak, and
/// an expired synthetic resource no sweep can find is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RefusalRule {
    /// No marker tag, or a marker with any other value. The floor.
    NotSynthetic,
    /// The run-id tag is absent or not `tr_` plus 32 lowercase hex characters.
    RunIdMalformed,
    /// The lane tag is absent or outside the closed lane set.
    LaneUnknown,
    /// The owner tag is absent or outside the closed owner set.
    OwnerUnknown,
    /// The expiry tag is absent or not RFC 3339. Never treated as expired.
    ExpiryUnparseable,
    /// The kind is not a declared `[janitor.resource]` row.
    KindUnknown,
    /// The kind has no discovery route, so no sweep can find one.
    KindNotReclaimable,
    /// An AEX-named kind whose identity is outside this run's namespace.
    IdentityOutsideRunNamespace,
}

impl RefusalRule {
    /// The stable rule id, as it appears in reports and violations.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::NotSynthetic => "not-synthetic",
            Self::RunIdMalformed => "run-id-malformed",
            Self::LaneUnknown => "lane-unknown",
            Self::OwnerUnknown => "owner-unknown",
            Self::ExpiryUnparseable => "expiry-unparseable",
            Self::KindUnknown => "kind-unknown",
            Self::KindNotReclaimable => "kind-not-reclaimable",
            Self::IdentityOutsideRunNamespace => "identity-outside-run-namespace",
        }
    }

    /// Whether a resource refused for this reason is nonetheless residue the
    /// release gate must fail on.
    ///
    /// The first four say "this is not a synthetic test resource, or it is not
    /// legible as one", so it is not this suite's leak to answer for. The last
    /// two say "this *is* ours, its run has expired, and no sweep can remove
    /// it", which is exactly the residue OD-36 forbids.
    #[must_use]
    pub const fn is_residue(self) -> bool {
        matches!(self, Self::KindUnknown | Self::KindNotReclaimable)
    }
}

impl std::fmt::Display for RefusalRule {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// One refusal.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RefusedRecord {
    /// What was refused.
    pub identity: String,
    /// The kind the adapter claimed.
    pub kind: String,
    /// Which rule refused it.
    pub rule: RefusalRule,
    /// What a human needs to know.
    pub detail: String,
    /// Whether this refusal is also residue.
    pub is_residue: bool,
}

/// Proof that a resource passed every guard.
///
/// The fields are private and there is no public constructor: [`admit`] is the
/// only function in the workspace that can produce one. [`Reclaimer::reclaim`]
/// takes one by reference, so the type system - not a code review - is what
/// stops an adapter deleting something the guard refused.
#[derive(Debug, Clone)]
pub struct Admitted {
    kind: String,
    identity: String,
    run_id: String,
    lane: String,
    owner: String,
    expires_at: OffsetDateTime,
    rank: u8,
    /// What reclaiming this kind means, from policy, so an adapter is told the
    /// required steps rather than inventing them.
    reclaim: String,
}

impl Admitted {
    /// The declared resource kind.
    #[must_use]
    pub fn kind(&self) -> &str {
        &self.kind
    }

    /// The identity to delete by.
    #[must_use]
    pub fn identity(&self) -> &str {
        &self.identity
    }

    /// The run that created it.
    #[must_use]
    pub fn run_id(&self) -> &str {
        &self.run_id
    }

    /// The lane that minted the run.
    #[must_use]
    pub fn lane(&self) -> &str {
        &self.lane
    }

    /// The owning stream.
    #[must_use]
    pub fn owner(&self) -> &str {
        &self.owner
    }

    /// When the run's own TTL ran out.
    #[must_use]
    pub const fn expires_at(&self) -> OffsetDateTime {
        self.expires_at
    }

    /// Where this kind sits in the reclamation order.
    #[must_use]
    pub const fn rank(&self) -> u8 {
        self.rank
    }

    /// What reclaiming this kind means, from policy.
    #[must_use]
    pub fn reclaim_steps(&self) -> &str {
        &self.reclaim
    }
}

/// A resource the guard admitted but whose deadline has not passed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DeferredRecord {
    /// What was deferred.
    pub identity: String,
    /// Its kind.
    pub kind: String,
    /// The run that created it.
    pub run_id: String,
    /// When it becomes sweepable: expiry plus the policy grace.
    pub deadline: String,
}

/// A reclaimed resource.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ReclaimedRecord {
    /// What was reclaimed.
    pub identity: String,
    /// Its kind.
    pub kind: String,
    /// The run that created it.
    pub run_id: String,
    /// The owning stream, so a sweep names a team rather than a machine.
    pub owner: String,
}

/// A reclamation that was attempted and failed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FailedRecord {
    /// What could not be reclaimed.
    pub identity: String,
    /// Its kind.
    pub kind: String,
    /// The run that created it.
    pub run_id: String,
    /// What the adapter reported.
    pub reason: String,
}

/// What the sweep is allowed to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, clap::ValueEnum)]
#[serde(rename_all = "kebab-case")]
#[clap(rename_all = "kebab-case")]
pub enum SweepMode {
    /// Classify everything and delete nothing. The default, and what a first
    /// scheduled run against a plane should do.
    Report,
    /// Classify and reclaim.
    Reclaim,
}

/// The residue verdict a receipt carries.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Residue {
    /// Every expired synthetic resource was reclaimed.
    None,
    /// Something the run created is past its deadline and still there.
    Unreclaimed {
        /// How many resources.
        count: usize,
        /// Which ones, and why each survived.
        detail: String,
    },
}

impl Residue {
    /// The value the evidence receipt's `data.residue` field carries.
    #[must_use]
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Unreclaimed { .. } => "unreclaimed",
        }
    }
}

/// Deletes the resource an [`Admitted`] names.
///
/// Implementations must be idempotent: reclaiming something already gone is
/// success, because two sweeps and a live ledger can race.
pub trait Reclaimer {
    /// Deletes it, or reports why it could not.
    ///
    /// # Errors
    /// Returns the adapter's own message when the resource still exists and
    /// could not be removed.
    fn reclaim(&self, admitted: &Admitted) -> std::result::Result<(), String>;
}

/// A reclaimer that deletes nothing, for [`SweepMode::Report`].
#[derive(Debug, Default)]
pub struct DryRun;

impl Reclaimer for DryRun {
    fn reclaim(&self, _admitted: &Admitted) -> std::result::Result<(), String> {
        Ok(())
    }
}

/// The reclaimer a `--mode reclaim` run gets when no credentialed adapter is
/// compiled in.
///
/// It refuses with a stated reason rather than reporting a green no-op, which
/// is the same rule every unbuildable release step in this crate follows: a
/// lane that reports success having done nothing is worse than one that is red
/// for a reason.
#[derive(Debug, Default)]
pub struct UnavailableAdapter;

impl Reclaimer for UnavailableAdapter {
    fn reclaim(&self, admitted: &Admitted) -> std::result::Result<(), String> {
        Err(format!(
            "no credentialed reclamation adapter is compiled into this binary, so `{}` was not \
             deleted; required steps: {}",
            admitted.kind(),
            admitted.reclaim_steps()
        ))
    }
}

/// Whether a string has the exact shape of a test run id.
///
/// The shape comes from `[janitor]` rather than from
/// `aex_test_harness::TestRunId`, which a release binary cannot link: the
/// harness is a test-support crate and may only appear in `[dev-dependencies]`.
/// Reading it from the document both crates already embed is what stops the
/// minter and the validator disagreeing.
#[must_use]
pub fn is_well_formed_run_id(text: &str) -> bool {
    let policy = &Policy::embedded().janitor;
    text.strip_prefix(policy.run_id_prefix.as_str())
        .is_some_and(|hex_part| {
            hex_part.len() == policy.run_id_hex_len
                && hex_part
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        })
}

/// The guard. Decides whether one discovered resource may be reclaimed at all.
///
/// Returns the witness on success and the refusal on failure. Expiry is *not*
/// part of admission: a resource inside its TTL is admitted and then deferred,
/// so the report can tell "not ours" from "not yet".
///
/// # Errors
/// Returns a [`RefusedRecord`] naming the first rule that refused, in the order
/// documented on this module: marker, run id, lane, owner, expiry, kind,
/// namespace. The order is deliberate - the marker is checked first so a
/// resource that is not ours is never reported as a malformed one of ours.
// One function on purpose: every rule of the guard, in the order a reader would
// check them, with the refusal message next to the rule that produced it.
// Splitting it would scatter the safety property across seven call sites, which
// is exactly how a guard loses a rule without anyone noticing.
#[allow(clippy::too_many_lines)]
pub fn admit(
    policy: &JanitorPolicy,
    values: &aex_workspace_check::policy::Values,
    resource: &DiscoveredResource,
) -> std::result::Result<Admitted, RefusedRecord> {
    let refuse = |rule: RefusalRule, detail: String| RefusedRecord {
        identity: resource.identity.clone(),
        kind: resource.kind.clone(),
        rule,
        detail,
        is_residue: rule.is_residue(),
    };

    // 1. The marker, exactly. This is the floor and it is checked first.
    match resource.tags.get(&policy.synthetic_tag) {
        Some(value) if *value == policy.synthetic_value => {}
        Some(value) => {
            return Err(refuse(
                RefusalRule::NotSynthetic,
                format!(
                    "`{}` carries `{}` = `{value}`, not the marker `{}`; the janitor reclaims \
                     nothing without an exact match",
                    resource.identity, policy.synthetic_tag, policy.synthetic_value
                ),
            ));
        }
        None => {
            return Err(refuse(
                RefusalRule::NotSynthetic,
                format!(
                    "`{}` carries no `{}` tag; the janitor reclaims nothing that does not declare \
                     itself a synthetic test resource",
                    resource.identity, policy.synthetic_tag
                ),
            ));
        }
    }

    // 2. The run id, in its exact shape.
    let run_id = resource
        .tags
        .get(&policy.run_id_tag)
        .cloned()
        .unwrap_or_default();
    if !is_well_formed_run_id(&run_id) {
        return Err(refuse(
            RefusalRule::RunIdMalformed,
            format!(
                "`{}` carries `{}` = `{run_id}`, which is not `{}` plus {} lowercase hex \
                 characters",
                resource.identity, policy.run_id_tag, policy.run_id_prefix, policy.run_id_hex_len
            ),
        ));
    }

    // 3. The lane, from the closed set.
    let lane = resource
        .tags
        .get(&policy.lane_tag)
        .cloned()
        .unwrap_or_default();
    if !values.lane_ttl_minutes.contains_key(&lane) {
        return Err(refuse(
            RefusalRule::LaneUnknown,
            format!(
                "`{}` carries `{}` = `{lane}`, which is not a declared lane",
                resource.identity, policy.lane_tag
            ),
        ));
    }

    // 4. The owner, from the closed set.
    let owner = resource
        .tags
        .get(&policy.owner_tag)
        .cloned()
        .unwrap_or_default();
    if !values.owner.contains(&owner) {
        return Err(refuse(
            RefusalRule::OwnerUnknown,
            format!(
                "`{}` carries `{}` = `{owner}`, which is not a declared owner",
                resource.identity, policy.owner_tag
            ),
        ));
    }

    // 5. The expiry. An unreadable expiry is never read as "expired".
    let expires_text = resource
        .tags
        .get(&policy.expires_at_tag)
        .cloned()
        .unwrap_or_default();
    let Ok(expires_at) = OffsetDateTime::parse(&expires_text, &Rfc3339) else {
        return Err(refuse(
            RefusalRule::ExpiryUnparseable,
            format!(
                "`{}` carries `{}` = `{expires_text}`, which is not RFC 3339; an unreadable \
                 expiry is never treated as an expired one",
                resource.identity, policy.expires_at_tag
            ),
        ));
    };

    // 6. The kind, and whether any sweep could find one.
    let Some(row) = policy.resources.get(&resource.kind) else {
        return Err(refuse(
            RefusalRule::KindUnknown,
            format!(
                "`{}` declares kind `{}`, which has no [janitor.resource] row",
                resource.identity, resource.kind
            ),
        ));
    };
    if !row.reclaimable_from_tags() {
        return Err(refuse(
            RefusalRule::KindNotReclaimable,
            format!(
                "`{}` is a `{}` from run {run_id}: {}",
                resource.identity, resource.kind, row.reclaim
            ),
        ));
    }

    // 7. For a kind AEX names, the identity must sit in this run's namespace.
    if row.naming == "aex_minted" {
        let namespaces: Vec<String> = policy
            .minted_name_templates
            .iter()
            .map(|template| template.replace("{run_id}", &run_id))
            .collect();
        if !namespaces
            .iter()
            .any(|prefix| resource.identity.starts_with(prefix.as_str()))
        {
            return Err(refuse(
                RefusalRule::IdentityOutsideRunNamespace,
                format!(
                    "`{}` is tagged for run {run_id} but its identity starts with none of {}; a \
                     tag alone never condemns a resource whose name AEX mints",
                    resource.identity,
                    namespaces.join(", ")
                ),
            ));
        }
    }

    Ok(Admitted {
        kind: resource.kind.clone(),
        identity: resource.identity.clone(),
        run_id,
        lane,
        owner,
        expires_at,
        rank: row.rank,
        reclaim: row.reclaim.clone(),
    })
}

/// What one sweep found and did.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SweepReport {
    /// Schema discriminator.
    pub schema: &'static str,
    /// Which plane was swept.
    pub plane: Plane,
    /// What the sweep was allowed to do.
    pub mode: SweepMode,
    /// When it ran.
    pub swept_at: String,
    /// How many resources the inventory offered.
    pub discovered: usize,
    /// What the guard admitted and the deadline had passed for. In
    /// [`SweepMode::Report`] this is what a reclaiming sweep would remove.
    pub reclaimable: Vec<ReclaimedRecord>,
    /// What was actually reclaimed. Empty in [`SweepMode::Report`].
    pub reclaimed: Vec<ReclaimedRecord>,
    /// What was admitted but is still inside its TTL.
    pub deferred: Vec<DeferredRecord>,
    /// What the guard refused, and why.
    pub refused: Vec<RefusedRecord>,
    /// What reclamation was attempted on and failed.
    pub failed: Vec<FailedRecord>,
    /// The verdict the receipt carries.
    pub residue: &'static str,
}

impl SweepReport {
    /// The residue verdict, with the detail a receipt explanation needs.
    #[must_use]
    pub fn residue(&self) -> Residue {
        let mut lines: Vec<String> = Vec::new();
        for failure in &self.failed {
            lines.push(format!(
                "{} `{}` from run {}: {}",
                failure.kind, failure.identity, failure.run_id, failure.reason
            ));
        }
        for refusal in self.refused.iter().filter(|entry| entry.is_residue) {
            lines.push(format!("[{}] {}", refusal.rule, refusal.detail));
        }
        if lines.is_empty() {
            Residue::None
        } else {
            Residue::Unreclaimed {
                count: lines.len(),
                detail: lines.join("; "),
            }
        }
    }

    /// The exit code a sweep reports.
    ///
    /// Residue lands on [`Exit::EvidenceUnsound`], the code the flake and
    /// hygiene rules already share: a lane that left something behind past its
    /// deadline has not produced sound evidence.
    #[must_use]
    pub fn exit(&self) -> Exit {
        match self.residue() {
            Residue::None => Exit::Ok,
            Residue::Unreclaimed { .. } => Exit::EvidenceUnsound,
        }
    }

    /// A human-readable summary, one line per class.
    #[must_use]
    pub fn summary(&self) -> String {
        format!(
            "janitor {} plane={} discovered={} reclaimed={} deferred={} refused={} failed={} \
             residue={}",
            match self.mode {
                SweepMode::Report => "report",
                SweepMode::Reclaim => "reclaim",
            },
            match self.plane {
                Plane::Dev => "dev",
                Plane::Prd => "prd",
            },
            self.discovered,
            self.reclaimed.len(),
            self.deferred.len(),
            self.refused.len(),
            self.failed.len(),
            self.residue
        )
    }
}

/// Sweep a plane inventory.
///
/// Every resource is passed through [`admit`]. Admitted resources whose
/// deadline has passed are reclaimed in dependency order - ascending
/// `[janitor.resource].rank`, then by kind and identity so two sweeps of the
/// same plane produce the same order. A failure does not stop the sweep: the
/// remaining kinds are still attempted and every failure is reported.
///
/// The sweep is idempotent because it is stateless: it acts on what the plane
/// still holds, so running it twice reclaims the same set minus whatever the
/// first run removed.
#[must_use]
pub fn sweep(
    inventory: &Inventory,
    reclaimer: &dyn Reclaimer,
    mode: SweepMode,
    now: OffsetDateTime,
) -> SweepReport {
    let policy = Policy::embedded();
    let janitor = &policy.janitor;
    let grace = time::Duration::minutes(janitor.residue_grace_minutes);

    let mut admitted: Vec<Admitted> = Vec::new();
    let mut deferred: Vec<DeferredRecord> = Vec::new();
    let mut refused: Vec<RefusedRecord> = Vec::new();

    for resource in &inventory.resources {
        match admit(janitor, &policy.values, resource) {
            Err(refusal) => refused.push(refusal),
            Ok(candidate) => {
                let deadline = candidate.expires_at + grace;
                if now < deadline {
                    deferred.push(DeferredRecord {
                        identity: candidate.identity.clone(),
                        kind: candidate.kind.clone(),
                        run_id: candidate.run_id.clone(),
                        deadline: deadline
                            .format(&Rfc3339)
                            .unwrap_or_else(|_| deadline.unix_timestamp().to_string()),
                    });
                } else {
                    admitted.push(candidate);
                }
            }
        }
    }

    // Dependency order, then a total order so two sweeps agree.
    admitted.sort_by(|left, right| {
        left.rank
            .cmp(&right.rank)
            .then_with(|| left.kind.cmp(&right.kind))
            .then_with(|| left.identity.cmp(&right.identity))
    });

    let reclaimable: Vec<ReclaimedRecord> = admitted.iter().map(record_of).collect();
    let mut removed: Vec<ReclaimedRecord> = Vec::new();
    let mut failed: Vec<FailedRecord> = Vec::new();
    if mode == SweepMode::Reclaim {
        for candidate in &admitted {
            match reclaimer.reclaim(candidate) {
                Ok(()) => removed.push(record_of(candidate)),
                Err(reason) => failed.push(FailedRecord {
                    identity: candidate.identity.clone(),
                    kind: candidate.kind.clone(),
                    run_id: candidate.run_id.clone(),
                    reason,
                }),
            }
        }
    }

    let mut report = SweepReport {
        schema: SWEEP_SCHEMA,
        plane: inventory.plane,
        mode,
        swept_at: now
            .format(&Rfc3339)
            .unwrap_or_else(|_| now.unix_timestamp().to_string()),
        discovered: inventory.resources.len(),
        reclaimable,
        reclaimed: removed,
        deferred,
        refused,
        failed,
        residue: "none",
    };
    report.residue = report.residue().as_str();
    report
}

fn record_of(candidate: &Admitted) -> ReclaimedRecord {
    ReclaimedRecord {
        identity: candidate.identity.clone(),
        kind: candidate.kind.clone(),
        run_id: candidate.run_id.clone(),
        owner: candidate.owner.clone(),
    }
}

/// The tag and TTL scheme, rendered for an operator.
#[must_use]
pub fn describe_scheme() -> String {
    let policy = Policy::embedded();
    let janitor = &policy.janitor;
    let mut text = String::new();
    text.push_str("tags every synthetic test resource carries:\n");
    for (key, note) in [
        (
            janitor.synthetic_tag.as_str(),
            format!(
                "= `{}` exactly; the marker, and the floor",
                janitor.synthetic_value
            ),
        ),
        (
            janitor.run_id_tag.as_str(),
            format!(
                "= `{}` + {} lowercase hex",
                janitor.run_id_prefix, janitor.run_id_hex_len
            ),
        ),
        (
            janitor.owner_tag.as_str(),
            "= one of the closed owner set".to_owned(),
        ),
        (
            janitor.lane_tag.as_str(),
            "= one of the closed lane set".to_owned(),
        ),
        (janitor.expires_at_tag.as_str(), "= RFC 3339".to_owned()),
    ] {
        let _ = writeln!(text, "  {key} {note}");
    }
    let _ = writeln!(
        text,
        "\nresidue deadline = aex:test-expires-at + {} minutes",
        janitor.residue_grace_minutes
    );
    text.push_str("\nidentity namespaces an AEX-named resource must sit inside:\n");
    for template in &janitor.minted_name_templates {
        let _ = writeln!(text, "  {template}");
    }
    text.push_str("\nreclamation order:\n");
    let mut rows: Vec<(&u8, &String)> = janitor
        .resources
        .iter()
        .map(|(name, row)| (&row.rank, name))
        .collect();
    rows.sort();
    for (rank, name) in rows {
        let row = &janitor.resources[name];
        let _ = writeln!(
            text,
            "  {rank:>2}  {name:<22} discovery={:<18} {}",
            row.discovery,
            if row.reclaimable_from_tags() {
                row.reclaim.as_str()
            } else {
                "NOT RECLAIMABLE - never prd-eligible"
            }
        );
    }
    let _ = writeln!(
        text,
        "\nlanes permitted against prd: {}",
        janitor.prd_lanes.join(", ")
    );
    text
}

/// Structural checks over the janitor policy itself.
///
/// A policy that declares a prerequisite outranking its dependent would reclaim
/// them in the wrong order, and no test of the engine would catch it, because
/// the engine faithfully implements whatever the table says.
///
/// # Errors
/// Returns [`Exit::GraphVerification`] carrying every violation.
pub fn verify_policy() -> Result<()> {
    let janitor = &Policy::embedded().janitor;
    let mut violations = Vec::new();
    let known: BTreeSet<&str> = janitor.resources.keys().map(String::as_str).collect();
    for (name, row) in &janitor.resources {
        if !matches!(
            row.discovery.as_str(),
            "resource_tag" | "name_prefix" | "provider_metadata" | "none"
        ) {
            violations.push(crate::error::Violation::new(
                "janitor-discovery-unknown",
                format!("`{name}` declares discovery `{}`", row.discovery),
            ));
        }
        if !matches!(row.naming.as_str(), "aex_minted" | "provider_minted") {
            violations.push(crate::error::Violation::new(
                "janitor-naming-unknown",
                format!("`{name}` declares naming `{}`", row.naming),
            ));
        }
        for prerequisite in &row.requires_reclaim_first {
            let Some(other) = janitor.resources.get(prerequisite) else {
                violations.push(crate::error::Violation::new(
                    "janitor-prerequisite-unknown",
                    format!("`{name}` requires unknown kind `{prerequisite}` first"),
                ));
                continue;
            };
            if other.rank >= row.rank {
                violations.push(crate::error::Violation::new(
                    "janitor-order-inverted",
                    format!(
                        "`{name}` (rank {}) requires `{prerequisite}` (rank {}) first, but the \
                         reclamation order would take `{name}` no later",
                        row.rank, other.rank
                    ),
                ));
            }
        }
    }
    if !known.contains("payment_customer") || !known.contains("auto_recharge_policy") {
        violations.push(crate::error::Violation::new(
            "janitor-money-kinds-missing",
            "the payment kinds must be declared; a saved card and an enabled auto-recharge \
             policy are the only residue with a recurring-charge tail"
                .to_owned(),
        ));
    }
    if janitor.minted_name_templates.is_empty() {
        violations.push(crate::error::Violation::new(
            "janitor-no-name-namespace",
            "no minted_name_templates: without one, a forged tag set alone would admit a \
             production resource"
                .to_owned(),
        ));
    }
    if violations.is_empty() {
        Ok(())
    } else {
        Err(ToolError::many(Exit::GraphVerification, violations))
    }
}

#[cfg(test)]
mod tests {
    use super::{
        DryRun, Inventory, UnavailableAdapter, describe_scheme, is_well_formed_run_id,
        verify_policy,
    };

    #[test]
    fn the_embedded_janitor_policy_is_internally_consistent() {
        verify_policy().expect("the shipped policy declares a sound reclamation order");
    }

    #[test]
    fn a_run_id_must_be_the_exact_shape() {
        assert!(is_well_formed_run_id(&format!("tr_{}", "0".repeat(32))));
        assert!(is_well_formed_run_id(&format!(
            "tr_{}",
            "abcdef0123456789".repeat(2)
        )));
        assert!(!is_well_formed_run_id(&format!("tr_{}", "A".repeat(32))));
        assert!(!is_well_formed_run_id(&format!("tr_{}", "0".repeat(31))));
        assert!(!is_well_formed_run_id(&format!("tr_{}", "0".repeat(33))));
        assert!(!is_well_formed_run_id(&format!("xx_{}", "0".repeat(32))));
        assert!(!is_well_formed_run_id("tr_"));
        assert!(!is_well_formed_run_id(""));
    }

    #[test]
    fn an_inventory_with_the_wrong_schema_is_refused() {
        let err = Inventory::parse(r#"{"schema":"something-else","plane":"prd"}"#)
            .expect_err("the schema is checked");
        assert!(err.rules().contains(&"janitor-inventory-schema"));
    }

    #[test]
    fn the_scheme_description_names_the_marker_and_the_unreclaimable_kinds() {
        let text = describe_scheme();
        assert!(text.contains("aex:test-synthetic"), "{text}");
        assert!(text.contains("aex-test-harness/v1"), "{text}");
        assert!(text.contains("NOT RECLAIMABLE"), "{text}");
        assert!(text.contains("aextest-{run_id}-"), "{text}");
    }

    /// The dry-run reclaimer must never be reachable from `--mode reclaim`, and
    /// the unavailable adapter must refuse rather than report success.
    #[test]
    fn the_unavailable_adapter_refuses_and_names_the_steps_it_did_not_take() {
        let policy = aex_workspace_check::policy::Policy::embedded();
        let mut tags = std::collections::BTreeMap::new();
        tags.insert(
            policy.janitor.synthetic_tag.clone(),
            policy.janitor.synthetic_value.clone(),
        );
        tags.insert(
            policy.janitor.run_id_tag.clone(),
            format!("tr_{}", "0".repeat(32)),
        );
        tags.insert(policy.janitor.owner_tag.clone(), "delivery".to_owned());
        tags.insert(policy.janitor.lane_tag.clone(), "e2e".to_owned());
        tags.insert(
            policy.janitor.expires_at_tag.clone(),
            "2026-01-01T00:00:00Z".to_owned(),
        );
        let resource = super::DiscoveredResource {
            kind: "auto_recharge_policy".to_owned(),
            identity: "topup_0001".to_owned(),
            tags,
        };
        let admitted = super::admit(&policy.janitor, &policy.values, &resource)
            .expect("a fully tagged provider-minted resource is admitted");
        let error = super::Reclaimer::reclaim(&UnavailableAdapter, &admitted)
            .expect_err("no adapter is compiled in");
        assert!(
            error.contains("no credentialed reclamation adapter"),
            "{error}"
        );
        assert!(error.contains("enabled = false"), "{error}");
        super::Reclaimer::reclaim(&DryRun, &admitted).expect("the dry run never fails");
    }
}
