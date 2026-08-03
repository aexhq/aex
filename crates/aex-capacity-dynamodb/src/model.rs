//! Pure capacity authority transitions.

use std::collections::BTreeMap;

use aex_wire::ids::WorkspaceId;
use aex_wire::limits::LimitId;
use aex_wire::models::{EffectiveWorkspaceLimit, LimitSource, LimitValue};
use aex_wire::types::Timestamp;
use serde::{Deserialize, Serialize};

use crate::defaults::CapacityDefaults;

/// One internal controller command.
///
/// There is deliberately no public mutation shape. An override command is
/// accepted only by the controller's IAM identity and carries both an audited
/// approval reference and a monotonic regional-capacity fence.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CapacityCommand {
    /// Materialize the canonical defaults for a newly placed workspace.
    Bootstrap {
        /// Workspace whose complete record is required.
        workspace_id: WorkspaceId,
    },
    /// Reconcile an existing workspace to the embedded default revision.
    Reconcile {
        /// Workspace to reconcile.
        workspace_id: WorkspaceId,
        /// Authority revision the caller inspected.
        expected_revision: u64,
    },
    /// Install or replace one support-approved override.
    SetOverride {
        /// Workspace whose override changes.
        workspace_id: WorkspaceId,
        /// Authority revision the approval was based on.
        expected_revision: u64,
        /// Monotonic regional headroom/capacity assessment fence.
        capacity_fence: u64,
        /// Stable audited support approval identity.
        approval_id: String,
        /// Registered limit.
        limit_id: LimitId,
        /// Complete replacement value.
        value: LimitValue,
    },
    /// Remove one support-approved override and return to the current default.
    RemoveOverride {
        /// Workspace whose override changes.
        workspace_id: WorkspaceId,
        /// Authority revision the approval was based on.
        expected_revision: u64,
        /// Monotonic regional headroom/capacity assessment fence.
        capacity_fence: u64,
        /// Stable audited support approval identity.
        approval_id: String,
        /// Registered limit.
        limit_id: LimitId,
    },
}

impl CapacityCommand {
    /// Workspace selected by the command.
    #[must_use]
    pub const fn workspace(&self) -> WorkspaceId {
        match self {
            Self::Bootstrap { workspace_id }
            | Self::Reconcile { workspace_id, .. }
            | Self::SetOverride { workspace_id, .. }
            | Self::RemoveOverride { workspace_id, .. } => *workspace_id,
        }
    }
}

/// The override mutation encoded by a command.
#[derive(Clone, Debug, PartialEq)]
pub enum OverrideChange {
    /// No override changes; only bootstrap/default reconciliation is allowed.
    None,
    /// Install the value.
    Set(LimitId, LimitValue),
    /// Remove the value.
    Remove(LimitId),
}

/// One complete durable workspace capacity record.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CapacityState {
    /// Owning workspace.
    pub workspace_id: WorkspaceId,
    /// Monotonic authority revision.
    pub revision: u64,
    /// Embedded defaults revision used to calculate this record.
    pub defaults_revision: u64,
    /// Digest of the embedded defaults bytes.
    pub defaults_digest: String,
    /// Last admitted regional-capacity assessment fence.
    pub capacity_fence: u64,
    /// Current override values only.
    pub overrides: BTreeMap<LimitId, LimitValue>,
    /// Complete effective values, including defaults.
    pub effective: BTreeMap<LimitId, LimitValue>,
    /// When this authority record changed.
    pub changed_at: Timestamp,
    /// Approval that authored the latest override mutation, when any.
    pub last_approval_id: Option<String>,
    /// Exact last committed command, used for durable retry classification.
    pub last_command: CapacityCommand,
}

impl CapacityState {
    /// Projects the complete public records in registry order.
    #[must_use]
    pub fn projected_limits(&self) -> Vec<EffectiveWorkspaceLimit> {
        LimitId::ALL
            .iter()
            .copied()
            .map(|id| EffectiveWorkspaceLimit {
                changed_at: self.changed_at,
                effective_value: self
                    .effective
                    .get(&id)
                    .expect("a planned state is complete")
                    .clone(),
                id,
                revision: self.revision,
                source: if self.overrides.contains_key(&id) {
                    LimitSource::WorkspaceOverride
                } else {
                    LimitSource::Default
                },
            })
            .collect()
    }
}

/// A transition and whether it writes a new revision.
#[derive(Clone, Debug, PartialEq)]
pub struct PlannedCapacity {
    /// Complete desired record.
    pub state: CapacityState,
    /// False only for an exact reconcile replay.
    pub changed: bool,
}

/// Why an authority transition was refused.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum CapacityError {
    /// A bootstrap raced an existing record.
    #[error("workspace capacity already exists")]
    AlreadyExists,
    /// A non-bootstrap command has no authority record.
    #[error("workspace capacity does not exist")]
    Missing,
    /// The caller planned against another revision.
    #[error("expected authority revision {expected}, found {actual}")]
    Revision {
        /// Expected revision.
        expected: u64,
        /// Durable revision.
        actual: u64,
    },
    /// The capacity assessment fence did not advance.
    #[error("capacity fence {attempted} does not advance {current}")]
    CapacityFence {
        /// Durable fence.
        current: u64,
        /// Attempted fence.
        attempted: u64,
    },
    /// Approval identity was absent or malformed.
    #[error("approval identity must be 1..=128 printable ASCII bytes")]
    Approval,
    /// The default document moved backwards.
    #[error("defaults revision regressed from {current} to {attempted}")]
    DefaultsRegression {
        /// Durable revision.
        current: u64,
        /// Embedded revision.
        attempted: u64,
    },
    /// An authored revision was changed in place.
    #[error("defaults revision {revision} has digest `{actual}`, not `{expected}`")]
    DefaultsDigest {
        /// Authored revision whose bytes disagree.
        revision: u64,
        /// Digest stored in authority.
        expected: String,
        /// Digest embedded in the running artifact.
        actual: String,
    },
    /// An override has the wrong shape, dimensions or a zero value.
    #[error("override `{limit}` is invalid: {reason}")]
    Override {
        /// Limit spelling.
        limit: String,
        /// Exact refusal.
        reason: String,
    },
    /// Revision arithmetic exhausted.
    #[error("capacity authority revision exhausted")]
    RevisionExhausted,
}

/// Applies one command to the durable state.
///
/// # Errors
///
/// Returns [`CapacityError`] for a stale revision/fence, malformed approval,
/// default regression, invalid override or impossible bootstrap/reconcile.
pub fn plan_capacity_change(
    current: Option<&CapacityState>,
    defaults: &CapacityDefaults,
    command: &CapacityCommand,
    now: Timestamp,
) -> Result<PlannedCapacity, CapacityError> {
    let workspace = command.workspace();
    let (mut overrides, current_revision, current_fence) = match (current, command) {
        (None, CapacityCommand::Bootstrap { .. }) => (BTreeMap::new(), 0, 0),
        (None, _) => return Err(CapacityError::Missing),
        (Some(state), CapacityCommand::Bootstrap { .. }) => {
            if state.last_command == *command {
                return Ok(PlannedCapacity {
                    state: state.clone(),
                    changed: false,
                });
            }
            return Err(CapacityError::AlreadyExists);
        }
        (Some(state), _) => {
            if state.workspace_id != workspace {
                return Err(CapacityError::Missing);
            }
            if defaults.revision < state.defaults_revision {
                return Err(CapacityError::DefaultsRegression {
                    current: state.defaults_revision,
                    attempted: defaults.revision,
                });
            }
            if defaults.revision == state.defaults_revision
                && defaults.digest != state.defaults_digest
            {
                return Err(CapacityError::DefaultsDigest {
                    revision: defaults.revision,
                    expected: state.defaults_digest.clone(),
                    actual: defaults.digest.clone(),
                });
            }
            (
                state.overrides.clone(),
                state.revision,
                state.capacity_fence,
            )
        }
    };

    let (change, approval_id, next_fence, expected) = command_parts(command);
    if let Some(expected) = expected
        && expected != current_revision
    {
        if current.is_some_and(|state| exact_command_replay(state, command, expected)) {
            return Ok(PlannedCapacity {
                state: current
                    .expect("an override replay has current state")
                    .clone(),
                changed: false,
            });
        }
        return Err(CapacityError::Revision {
            expected,
            actual: current_revision,
        });
    }
    if !matches!(change, OverrideChange::None) {
        validate_approval(approval_id.as_deref().unwrap_or_default())?;
        if next_fence <= current_fence {
            return Err(CapacityError::CapacityFence {
                current: current_fence,
                attempted: next_fence,
            });
        }
    }
    let override_changed = !matches!(change, OverrideChange::None);
    match change {
        OverrideChange::None => {}
        OverrideChange::Set(id, value) => {
            validate_override(id, &value, defaults.value(id))?;
            overrides.insert(id, value);
        }
        OverrideChange::Remove(id) => {
            overrides.remove(&id);
        }
    }

    let changed = current.is_none()
        || override_changed
        || current.is_some_and(|state| state.defaults_revision != defaults.revision);
    if !changed {
        return Ok(PlannedCapacity {
            state: current
                .expect("an unchanged transition has current state")
                .clone(),
            changed: false,
        });
    }
    let revision = current_revision
        .checked_add(1)
        .ok_or(CapacityError::RevisionExhausted)?;
    let effective = LimitId::ALL
        .iter()
        .copied()
        .map(|id| {
            (
                id,
                overrides
                    .get(&id)
                    .cloned()
                    .unwrap_or_else(|| defaults.value(id).clone()),
            )
        })
        .collect();
    Ok(PlannedCapacity {
        state: CapacityState {
            workspace_id: workspace,
            revision,
            defaults_revision: defaults.revision,
            defaults_digest: defaults.digest.clone(),
            capacity_fence: if !override_changed {
                current_fence
            } else {
                next_fence
            },
            overrides,
            effective,
            changed_at: now,
            last_approval_id: approval_id,
            last_command: command.clone(),
        },
        changed: true,
    })
}

fn exact_command_replay(
    current: &CapacityState,
    command: &CapacityCommand,
    expected_revision: u64,
) -> bool {
    if current.revision != expected_revision.saturating_add(1) {
        return false;
    }
    current.last_command == *command
}

fn command_parts(command: &CapacityCommand) -> (OverrideChange, Option<String>, u64, Option<u64>) {
    match command {
        CapacityCommand::Bootstrap { .. } => (OverrideChange::None, None, 0, None),
        CapacityCommand::Reconcile {
            expected_revision, ..
        } => (OverrideChange::None, None, 0, Some(*expected_revision)),
        CapacityCommand::SetOverride {
            expected_revision,
            capacity_fence,
            approval_id,
            limit_id,
            value,
            ..
        } => (
            OverrideChange::Set(*limit_id, value.clone()),
            Some(approval_id.clone()),
            *capacity_fence,
            Some(*expected_revision),
        ),
        CapacityCommand::RemoveOverride {
            expected_revision,
            capacity_fence,
            approval_id,
            limit_id,
            ..
        } => (
            OverrideChange::Remove(*limit_id),
            Some(approval_id.clone()),
            *capacity_fence,
            Some(*expected_revision),
        ),
    }
}

fn validate_approval(approval: &str) -> Result<(), CapacityError> {
    if approval.is_empty()
        || approval.len() > 128
        || !approval.bytes().all(|byte| (0x21..=0x7e).contains(&byte))
    {
        return Err(CapacityError::Approval);
    }
    Ok(())
}

fn validate_override(
    id: LimitId,
    value: &LimitValue,
    default: &LimitValue,
) -> Result<(), CapacityError> {
    if !value.is_complete_for(id) {
        return Err(invalid_override(
            id,
            "value is incomplete, zero or differs from registered dimensions",
        ));
    }
    match (value, default) {
        (LimitValue::Scalar(value), LimitValue::Scalar(_)) if value.value.get() > 0 => Ok(()),
        (LimitValue::Map(value), LimitValue::Map(default)) => {
            if value.values.keys().ne(default.values.keys()) {
                return Err(invalid_override(
                    id,
                    "dimensions differ from the canonical default",
                ));
            }
            if value.values.values().any(|dimension| dimension.get() == 0) {
                return Err(invalid_override(id, "a dimension is zero"));
            }
            Ok(())
        }
        (LimitValue::Scalar(_), LimitValue::Scalar(_)) => {
            Err(invalid_override(id, "the scalar value is zero"))
        }
        _ => Err(invalid_override(id, "shape differs from the default")),
    }
}

fn invalid_override(id: LimitId, reason: &str) -> CapacityError {
    CapacityError::Override {
        limit: id.as_str().to_owned(),
        reason: reason.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use aex_wire::ids::{PrefixedId as _, Uuid7, WorkspaceId};
    use aex_wire::limits::LimitId;
    use aex_wire::models::{LimitScalarValue, LimitSource, LimitValue};
    use aex_wire::types::{DecimalU128, Timestamp};

    use super::{CapacityCommand, CapacityError, plan_capacity_change};
    use crate::defaults::canonical_defaults;

    fn workspace() -> WorkspaceId {
        WorkspaceId::from_uuid7(Uuid7::compose(1, [7; 10]))
    }

    fn now(value: i64) -> Timestamp {
        Timestamp::from_unix_millis(value).expect("timestamp")
    }

    #[test]
    fn bootstrap_is_complete_and_an_exact_reconcile_is_a_no_op() {
        let defaults = canonical_defaults().expect("defaults");
        let bootstrap = CapacityCommand::Bootstrap {
            workspace_id: workspace(),
        };
        let planned = plan_capacity_change(None, &defaults, &bootstrap, now(1)).expect("plans");
        assert!(planned.changed);
        assert_eq!(planned.state.revision, 1);
        assert_eq!(planned.state.effective.len(), LimitId::ALL.len());
        assert!(
            planned
                .state
                .projected_limits()
                .iter()
                .all(|limit| limit.source == LimitSource::Default)
        );

        let replay = CapacityCommand::Reconcile {
            workspace_id: workspace(),
            expected_revision: 1,
        };
        let replayed = plan_capacity_change(Some(&planned.state), &defaults, &replay, now(2))
            .expect("replays");
        assert!(!replayed.changed);
        assert_eq!(replayed.state, planned.state);
    }

    #[test]
    fn override_and_removal_are_revision_and_capacity_fenced() {
        let defaults = canonical_defaults().expect("defaults");
        let initial = plan_capacity_change(
            None,
            &defaults,
            &CapacityCommand::Bootstrap {
                workspace_id: workspace(),
            },
            now(1),
        )
        .expect("bootstrap")
        .state;
        let set = CapacityCommand::SetOverride {
            workspace_id: workspace(),
            expected_revision: 1,
            capacity_fence: 9,
            approval_id: "support-approval-1".to_owned(),
            limit_id: LimitId::ApiJsonBody,
            value: LimitValue::Scalar(LimitScalarValue {
                value: DecimalU128::new(131_072),
            }),
        };
        let overridden = plan_capacity_change(Some(&initial), &defaults, &set, now(2))
            .expect("override")
            .state;
        assert_eq!(overridden.revision, 2);
        assert_eq!(overridden.capacity_fence, 9);
        assert_eq!(
            overridden
                .projected_limits()
                .iter()
                .find(|limit| limit.id == LimitId::ApiJsonBody)
                .expect("limit")
                .source,
            LimitSource::WorkspaceOverride
        );
        let replay =
            plan_capacity_change(Some(&overridden), &defaults, &set, now(99)).expect("exact retry");
        assert!(!replay.changed);
        assert_eq!(replay.state, overridden);

        let conflicting_retry = CapacityCommand::SetOverride {
            workspace_id: workspace(),
            expected_revision: 1,
            capacity_fence: 9,
            approval_id: "support-approval-1".to_owned(),
            limit_id: LimitId::ApiJsonBody,
            value: LimitValue::Scalar(LimitScalarValue {
                value: DecimalU128::new(262_144),
            }),
        };
        assert!(matches!(
            plan_capacity_change(Some(&overridden), &defaults, &conflicting_retry, now(99)),
            Err(CapacityError::Revision { .. })
        ));

        let stale = CapacityCommand::RemoveOverride {
            workspace_id: workspace(),
            expected_revision: 2,
            capacity_fence: 9,
            approval_id: "support-approval-2".to_owned(),
            limit_id: LimitId::ApiJsonBody,
        };
        assert!(matches!(
            plan_capacity_change(Some(&overridden), &defaults, &stale, now(3)),
            Err(CapacityError::CapacityFence { .. })
        ));

        let remove = CapacityCommand::RemoveOverride {
            workspace_id: workspace(),
            expected_revision: 2,
            capacity_fence: 10,
            approval_id: "support-approval-2".to_owned(),
            limit_id: LimitId::ApiJsonBody,
        };
        let removed = plan_capacity_change(Some(&overridden), &defaults, &remove, now(3))
            .expect("removes")
            .state;
        assert!(!removed.overrides.contains_key(&LimitId::ApiJsonBody));
        assert_eq!(removed.revision, 3);
    }

    #[test]
    fn shape_dimension_zero_approval_and_revision_drift_fail_closed() {
        let defaults = canonical_defaults().expect("defaults");
        let state = plan_capacity_change(
            None,
            &defaults,
            &CapacityCommand::Bootstrap {
                workspace_id: workspace(),
            },
            now(1),
        )
        .expect("bootstrap")
        .state;
        for command in [
            CapacityCommand::SetOverride {
                workspace_id: workspace(),
                expected_revision: 7,
                capacity_fence: 1,
                approval_id: "approval".to_owned(),
                limit_id: LimitId::ApiJsonBody,
                value: LimitValue::Scalar(LimitScalarValue {
                    value: DecimalU128::new(1),
                }),
            },
            CapacityCommand::SetOverride {
                workspace_id: workspace(),
                expected_revision: 1,
                capacity_fence: 1,
                approval_id: String::new(),
                limit_id: LimitId::ApiJsonBody,
                value: LimitValue::Scalar(LimitScalarValue {
                    value: DecimalU128::new(1),
                }),
            },
            CapacityCommand::SetOverride {
                workspace_id: workspace(),
                expected_revision: 1,
                capacity_fence: 1,
                approval_id: "approval".to_owned(),
                limit_id: LimitId::ApiJsonBody,
                value: LimitValue::Scalar(LimitScalarValue {
                    value: DecimalU128::ZERO,
                }),
            },
        ] {
            assert!(
                plan_capacity_change(Some(&state), &defaults, &command, now(2)).is_err(),
                "invalid command was admitted: {command:?}"
            );
        }
    }
}
