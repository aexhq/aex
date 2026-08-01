//! Deletion denial that survives a metadata restore.
//!
//! `unwrap_allowed` is a pure predicate over `(OwnerEdge, DenialProjection,
//! central_frontier)`. Its point is the frontier check: a restored `DynamoDB`
//! snapshot carries whatever owner edges it carried when it was taken, so owner
//! edges alone can never authorize an unwrap. The projection must have replayed
//! through the current central frontier, and no restored data can make that
//! true on its own.

use std::collections::BTreeSet;

use aex_wire::ids::{SessionId, WorkspaceId};
use aex_wire::types::Timestamp;

use crate::pin::OwnerEdge;

/// The monotone position of the central deletion-denial ledger.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct DenialEpoch(pub u64);

impl DenialEpoch {
    /// The epoch a fresh plane starts at.
    pub const INITIAL: Self = Self(0);
}

/// Whose content is denied.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum DenialSubject {
    /// One session's content.
    Session(SessionId),
    /// A whole workspace's content.
    Workspace(WorkspaceId),
}

/// One content-free denial fact.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct DeletionDenial {
    /// Whose content is denied.
    pub subject: DenialSubject,
    /// The ledger position the fact was written at.
    pub epoch: DenialEpoch,
    /// When the fact was recorded.
    pub recorded_at: Timestamp,
}

/// The regional replay of the central denial ledger.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DenialProjection {
    replayed_through: DenialEpoch,
    denied: BTreeSet<DenialSubject>,
}

impl DenialProjection {
    /// An empty projection at the initial epoch.
    #[must_use]
    pub fn new() -> Self {
        Self {
            replayed_through: DenialEpoch::INITIAL,
            denied: BTreeSet::new(),
        }
    }

    /// How far the projection has replayed.
    #[must_use]
    pub const fn replayed_through(&self) -> DenialEpoch {
        self.replayed_through
    }

    /// Whether a subject is denied.
    #[must_use]
    pub fn is_denied(&self, subject: &DenialSubject) -> bool {
        self.denied.contains(subject)
    }

    /// Every denied subject, in canonical order.
    pub fn denied(&self) -> impl Iterator<Item = &DenialSubject> {
        self.denied.iter()
    }

    /// Replays one fact.
    ///
    /// A fact at or below the replayed position is ignored — replay is
    /// idempotent — and no fact ever removes a subject: denial is monotone in
    /// both the epoch and the subject set.
    pub fn apply(&mut self, fact: DeletionDenial) {
        self.denied.insert(fact.subject);
        if fact.epoch > self.replayed_through {
            self.replayed_through = fact.epoch;
        }
    }

    /// Records that the projection has caught up to a position with no further
    /// facts. Never moves backwards.
    pub fn advance_to(&mut self, epoch: DenialEpoch) {
        if epoch > self.replayed_through {
            self.replayed_through = epoch;
        }
    }
}

/// Why an unwrap was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum UnwrapDenied {
    /// The subject is on the denial ledger.
    #[error("content for {0:?} is denied")]
    SubjectDenied(DenialSubject),
    /// The local projection has not replayed through the central frontier.
    #[error("denial projection replayed through {replayed_through:?}, needs {required:?}")]
    ProjectionBehind {
        /// How far the projection has replayed.
        replayed_through: DenialEpoch,
        /// The central frontier it must reach.
        required: DenialEpoch,
    },
    /// No owner edge reaches the object.
    #[error("no owner edge authorizes this unwrap")]
    OwnerEdgeAbsent,
}

/// Whether an owner edge may be unwrapped.
///
/// # Errors
///
/// Returns [`UnwrapDenied`] naming the first failing check. The frontier check
/// is the only way to pass and cannot be satisfied by restored data alone.
pub fn unwrap_allowed(
    edge: Option<&OwnerEdge>,
    projection: &DenialProjection,
    central_frontier: DenialEpoch,
) -> Result<(), UnwrapDenied> {
    if projection.replayed_through < central_frontier {
        return Err(UnwrapDenied::ProjectionBehind {
            replayed_through: projection.replayed_through,
            required: central_frontier,
        });
    }
    let edge = edge.ok_or(UnwrapDenied::OwnerEdgeAbsent)?;
    let workspace = DenialSubject::Workspace(edge.workspace);
    if projection.is_denied(&workspace) {
        return Err(UnwrapDenied::SubjectDenied(workspace));
    }
    if let crate::pin::PinSubject::Session(session) = edge.subject {
        let subject = DenialSubject::Session(session);
        if projection.is_denied(&subject) {
            return Err(UnwrapDenied::SubjectDenied(subject));
        }
    }
    if let crate::pin::Pin::Root { session, .. } = edge.pin {
        let subject = DenialSubject::Session(session);
        if projection.is_denied(&subject) {
            return Err(UnwrapDenied::SubjectDenied(subject));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use aex_wire::ids::{PrefixedId as _, SessionId, Uuid7, WorkspaceId};
    use aex_wire::types::Timestamp;

    use super::{
        DeletionDenial, DenialEpoch, DenialProjection, DenialSubject, UnwrapDenied, unwrap_allowed,
    };
    use crate::pin::{OwnerEdge, Pin, PinSubject, RootKind};
    use crate::tree::ContentRoot;

    fn workspace() -> WorkspaceId {
        WorkspaceId::from_uuid7(Uuid7::compose(1, [1; 10]))
    }

    fn session() -> SessionId {
        SessionId::from_uuid7(Uuid7::compose(1, [2; 10]))
    }

    fn edge() -> OwnerEdge {
        OwnerEdge {
            workspace: workspace(),
            subject: PinSubject::Session(session()),
            pin: Pin::Root {
                session: session(),
                kind: RootKind::Persisted,
                root: ContentRoot {
                    digest: [0; 32],
                    entries: 0,
                    logical_bytes: 0,
                },
            },
        }
    }

    #[test]
    fn a_behind_projection_denies_whatever_edges_exist() {
        let projection = DenialProjection::new();
        assert_eq!(
            unwrap_allowed(Some(&edge()), &projection, DenialEpoch(7)),
            Err(UnwrapDenied::ProjectionBehind {
                replayed_through: DenialEpoch::INITIAL,
                required: DenialEpoch(7),
            })
        );
    }

    #[test]
    fn a_caught_up_projection_allows_an_undenied_edge() {
        let mut projection = DenialProjection::new();
        projection.advance_to(DenialEpoch(7));
        assert_eq!(
            unwrap_allowed(Some(&edge()), &projection, DenialEpoch(7)),
            Ok(())
        );
        assert_eq!(
            unwrap_allowed(None, &projection, DenialEpoch(7)),
            Err(UnwrapDenied::OwnerEdgeAbsent)
        );
    }

    #[test]
    fn a_denied_subject_is_never_undenied() {
        let mut projection = DenialProjection::new();
        projection.apply(DeletionDenial {
            subject: DenialSubject::Session(session()),
            epoch: DenialEpoch(7),
            recorded_at: Timestamp::from_unix_millis(0).expect("in range"),
        });
        assert_eq!(projection.replayed_through(), DenialEpoch(7));
        assert_eq!(
            unwrap_allowed(Some(&edge()), &projection, DenialEpoch(7)),
            Err(UnwrapDenied::SubjectDenied(DenialSubject::Session(
                session()
            )))
        );
        projection.advance_to(DenialEpoch(9));
        assert_eq!(
            unwrap_allowed(Some(&edge()), &projection, DenialEpoch(9)),
            Err(UnwrapDenied::SubjectDenied(DenialSubject::Session(
                session()
            )))
        );
    }
}
