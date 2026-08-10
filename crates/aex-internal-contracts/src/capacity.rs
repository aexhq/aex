//! The one command central control may send the regional capacity authority.
//!
//! # Why the command shape is duplicated here rather than imported
//!
//! `regional-capacity-controller` owns the full `CapacityCommand` vocabulary —
//! bootstrap, reconcile and the two override arms — inside
//! `aex-capacity-dynamodb`, together with the store that commits them. Central
//! control **may not link that crate**: the capacity producer is a separate
//! Cargo feature and a disjoint IAM keyspace precisely so that central cannot
//! write a limit row, and a dependency edge would undo the first half of that
//! while the second half silently failed at runtime instead.
//!
//! So central names the one command it is allowed to send, and nothing else.
//! The duplication is deliberate and it is fenced: the controller's own suite
//! asserts that [`CapacityBootstrap`] encodes byte-identically to
//! `CapacityCommand::Bootstrap`, so a rename on either side is a failing test
//! rather than a `Refused` at three in the morning.
//!
//! # Why the outcome is decoded loosely
//!
//! An `Applied` answer carries the complete authority state, including typed
//! limit values whose vocabulary is the regional plane's. Central has no use for
//! any of it and must not gain the ability to parse it, so this decoder reads
//! the discriminator and the one boolean it acts on and ignores the rest.

use aex_wire::ids::WorkspaceId;
use serde::{Deserialize, Serialize};

/// Materialise the complete effective-limit set for a newly placed workspace.
///
/// Idempotent by construction: an exact replay answers
/// [`CapacityOutcome::Applied`] with `changed: false`, and a workspace that was
/// already bootstrapped and has since been reconciled or overridden answers
/// [`CapacityOutcome::Refused`] with [`ALREADY_EXISTS`] — which is also a
/// satisfied precondition, because the authority row and the projection rows
/// land in one transaction.
/// The field spelling is `workspace_id`, not the `camelCase` the rest of this
/// crate uses. That is not a slip: this envelope's shape is dictated by the
/// decoder on the other side, and matching the receiver is the whole job.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum CapacityBootstrap {
    /// The one arm. A tagged variant rather than a bare struct, so the encoding
    /// carries the discriminator the controller matches on.
    Bootstrap {
        /// Whose effective set to materialise.
        workspace_id: WorkspaceId,
    },
}

impl CapacityBootstrap {
    /// The command for one workspace.
    #[must_use]
    pub const fn new(workspace_id: WorkspaceId) -> Self {
        Self::Bootstrap { workspace_id }
    }
}

/// The controller's bounded answer, narrowed to what central acts on.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum CapacityOutcome {
    /// A revision committed, or an exact replay resolved against one that had.
    Applied {
        /// False for an exact durable replay. Either way the set exists.
        changed: bool,
    },
    /// A deterministic policy refusal. No write occurred.
    Refused {
        /// The controller's stable closed reason code.
        code: String,
        /// A bounded human diagnostic.
        message: String,
    },
}

/// The refusal that means "this workspace already has a complete effective set".
///
/// A bootstrap reaches it whenever the durable state exists and its last command
/// was something else — a reconcile after a defaults release, or a support
/// override. It is a **satisfied** precondition, not a failure: the thing the
/// caller needed to be true is true, and it was made true by somebody else.
pub const ALREADY_EXISTS: &str = "already_exists";

impl CapacityOutcome {
    /// Whether the complete effective set is durable after this answer.
    ///
    /// The one question a provisioning caller asks. Everything else the
    /// controller can say means the set may not exist, and a caller that
    /// continued on one of those would publish a placement for a workspace whose
    /// every request answers `401`.
    #[must_use]
    pub fn set_is_materialised(&self) -> bool {
        match self {
            Self::Applied { .. } => true,
            Self::Refused { code, .. } => code == ALREADY_EXISTS,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{ALREADY_EXISTS, CapacityBootstrap, CapacityOutcome};
    use aex_wire::ids::{PrefixedId as _, Uuid7, WorkspaceId};

    fn workspace() -> WorkspaceId {
        WorkspaceId::from_uuid7(Uuid7::compose(1_754_051_696_789, [7; 10]))
    }

    #[test]
    fn the_command_encodes_as_the_controllers_tagged_bootstrap() {
        let encoded =
            serde_json::to_value(CapacityBootstrap::new(workspace())).expect("the command encodes");
        assert_eq!(
            encoded,
            serde_json::json!({ "kind": "bootstrap", "workspace_id": workspace().to_string() }),
            "the encoding drifted from the controller's command vocabulary"
        );
    }

    #[test]
    fn an_applied_answer_decodes_without_parsing_the_regional_state() {
        // The real answer carries the whole authority record. Central must be
        // able to read the two fields it acts on without gaining the ability to
        // parse a typed limit value.
        let answer: CapacityOutcome = serde_json::from_value(serde_json::json!({
            "status": "applied",
            "changed": false,
            "state": { "revision": 3, "effective": { "api.json_body": { "kind": "scalar" } } },
        }))
        .expect("an applied answer decodes");
        assert_eq!(answer, CapacityOutcome::Applied { changed: false });
        assert!(answer.set_is_materialised());
    }

    #[test]
    fn only_an_already_existing_set_makes_a_refusal_a_satisfied_precondition() {
        let existing = CapacityOutcome::Refused {
            code: ALREADY_EXISTS.to_owned(),
            message: "workspace capacity already exists".to_owned(),
        };
        assert!(existing.set_is_materialised());
        for code in [
            "revision_conflict",
            "defaults_regression",
            "defaults_digest_conflict",
            "invalid_override",
            "missing",
            "revision_exhausted",
            "invalid_approval",
            "capacity_fence_conflict",
        ] {
            let refusal = CapacityOutcome::Refused {
                code: code.to_owned(),
                message: String::new(),
            };
            assert!(
                !refusal.set_is_materialised(),
                "`{code}` was treated as proof of a complete effective set"
            );
        }
    }
}
