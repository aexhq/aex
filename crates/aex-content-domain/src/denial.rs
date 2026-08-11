//! Workspace deletion denial that survives a metadata restore.
//!
//! The monotone ledger is independent of restorable content metadata. A
//! restored registry descriptor or pin cannot move this projection's frontier
//! or remove a denied workspace.

use std::collections::BTreeSet;

use aex_wire::ids::WorkspaceId;
use aex_wire::types::Timestamp;

/// The monotone position of the central deletion-denial ledger.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct DenialEpoch(pub u64);

impl DenialEpoch {
    /// The epoch a fresh plane starts at.
    pub const INITIAL: Self = Self(0);
}

/// One content-free denial fact.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct DeletionDenial {
    /// Whose registered content is denied.
    pub workspace: WorkspaceId,
    /// The ledger position the fact was written at.
    pub epoch: DenialEpoch,
    /// When the fact was recorded.
    pub recorded_at: Timestamp,
}

/// The regional replay of the central denial ledger.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DenialProjection {
    replayed_through: DenialEpoch,
    denied: BTreeSet<WorkspaceId>,
}

impl DenialProjection {
    /// An empty projection at the initial epoch.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// How far the projection has replayed.
    #[must_use]
    pub const fn replayed_through(&self) -> DenialEpoch {
        self.replayed_through
    }

    /// Whether a workspace is denied.
    #[must_use]
    pub fn is_denied(&self, workspace: &WorkspaceId) -> bool {
        self.denied.contains(workspace)
    }

    /// Every denied workspace, in canonical order.
    pub fn denied(&self) -> impl Iterator<Item = &WorkspaceId> {
        self.denied.iter()
    }

    /// Replays one monotone fact.
    pub fn apply(&mut self, fact: DeletionDenial) {
        self.denied.insert(fact.workspace);
        if fact.epoch > self.replayed_through {
            self.replayed_through = fact.epoch;
        }
    }

    /// Records that the projection caught up through an empty position.
    pub fn advance_to(&mut self, epoch: DenialEpoch) {
        if epoch > self.replayed_through {
            self.replayed_through = epoch;
        }
    }
}

/// Why an unwrap was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum UnwrapDenied {
    /// The workspace is on the denial ledger.
    #[error("content for workspace {0} is denied")]
    WorkspaceDenied(WorkspaceId),
    /// The local projection has not replayed through the central frontier.
    #[error("denial projection replayed through {replayed_through:?}, needs {required:?}")]
    ProjectionBehind {
        /// How far the projection has replayed.
        replayed_through: DenialEpoch,
        /// The central frontier it must reach.
        required: DenialEpoch,
    },
}

/// Whether registered content in `workspace` may be unwrapped.
///
/// # Errors
///
/// Returns [`UnwrapDenied`] when replay is behind or the workspace is denied.
pub fn unwrap_allowed(
    workspace: WorkspaceId,
    projection: &DenialProjection,
    central_frontier: DenialEpoch,
) -> Result<(), UnwrapDenied> {
    if projection.replayed_through < central_frontier {
        return Err(UnwrapDenied::ProjectionBehind {
            replayed_through: projection.replayed_through,
            required: central_frontier,
        });
    }
    if projection.is_denied(&workspace) {
        return Err(UnwrapDenied::WorkspaceDenied(workspace));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use aex_wire::ids::{PrefixedId as _, Uuid7, WorkspaceId};
    use aex_wire::types::Timestamp;

    use super::{DeletionDenial, DenialEpoch, DenialProjection, UnwrapDenied, unwrap_allowed};

    fn workspace() -> WorkspaceId {
        WorkspaceId::from_uuid7(Uuid7::compose(1, [1; 10]))
    }

    #[test]
    fn replay_frontier_and_workspace_denial_both_fail_closed() {
        let mut projection = DenialProjection::new();
        assert!(matches!(
            unwrap_allowed(workspace(), &projection, DenialEpoch(1)),
            Err(UnwrapDenied::ProjectionBehind { .. })
        ));
        projection.apply(DeletionDenial {
            workspace: workspace(),
            epoch: DenialEpoch(1),
            recorded_at: Timestamp::from_unix_millis(0).expect("in range"),
        });
        assert_eq!(
            unwrap_allowed(workspace(), &projection, DenialEpoch(1)),
            Err(UnwrapDenied::WorkspaceDenied(workspace()))
        );
        projection.advance_to(DenialEpoch(2));
        assert_eq!(
            unwrap_allowed(workspace(), &projection, DenialEpoch(2)),
            Err(UnwrapDenied::WorkspaceDenied(workspace()))
        );
    }
}
