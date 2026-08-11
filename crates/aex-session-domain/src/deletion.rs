//! Irreversible session deletion.
//!
//! The public product has no trash or restore window. [`begin_delete`] is the
//! single destructive fence: it advances the deletion epoch exactly once,
//! closes all ordinary admission and binds the cleanup to one durable
//! operation. Completion requires content-free evidence from every owner and
//! leaves only the minimal tombstone plus aggregate accounting/audit facts.

pub use aex_operation_domain::{DeletionEpoch, DeletionGuard, DeletionState};

use aex_wire::ids::{OperationId, SessionId, WorkspaceId};
use aex_wire::types::Timestamp;

use crate::session::WorkAdmission;

/// The minimal marker retained after user-content deletion.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionTombstone {
    /// The deleted session.
    pub session: SessionId,
    /// Its owning workspace. Registered workspace files are independent and remain.
    pub workspace: WorkspaceId,
    /// The operation that crossed the irreversible fence.
    pub deleted_by: OperationId,
    /// The deletion epoch retained for stale-command fencing.
    pub epoch: DeletionEpoch,
    /// When all required cleanup evidence committed.
    pub deleted_at: Timestamp,
}

/// Independent durable facts required before deletion may complete.
///
/// These flags deliberately do not imply an ordering. The operation can retry
/// each owner independently, but it may publish a tombstone only after all
/// required facts are durable.
#[allow(
    clippy::struct_excessive_bools,
    reason = "each flag is an independent fact from a distinct cleanup owner"
)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeleteEvidence {
    /// The exact provider generation is terminated (or was never materialized).
    pub generation_terminated: bool,
    /// Session-head user content is removed.
    pub session_content_removed: bool,
    /// Message payload rows and indexes are removed.
    pub messages_removed: bool,
    /// Brain journal/control rows containing user content are removed.
    pub brain_user_content_removed: bool,
    /// Session-scoped observations and telemetry are removed.
    pub observations_removed: bool,
    /// Session export objects and grants are removed or irrevocably fenced.
    pub export_objects_removed: bool,
    /// Aggregate billing facts required for accounting remain available.
    pub billing_aggregate_retained: bool,
    /// Content-free audit evidence required for compliance remains available.
    pub audit_fact_retained: bool,
}

impl DeleteEvidence {
    /// The first missing completion predicate, in canonical order.
    #[must_use]
    pub fn missing(&self) -> Option<&'static str> {
        [
            (self.generation_terminated, "generation_terminated"),
            (self.session_content_removed, "session_content_removed"),
            (self.messages_removed, "messages_removed"),
            (
                self.brain_user_content_removed,
                "brain_user_content_removed",
            ),
            (self.observations_removed, "observations_removed"),
            (self.export_objects_removed, "export_objects_removed"),
            (
                self.billing_aggregate_retained,
                "billing_aggregate_retained",
            ),
            (self.audit_fact_retained, "audit_fact_retained"),
        ]
        .into_iter()
        .find_map(|(present, name)| (!present).then_some(name))
    }
}

/// The guard and admission state landed at the destructive fence.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DeleteCommit {
    /// The guard after deletion was claimed.
    pub guard: DeletionGuard,
    /// The closed admission state.
    pub admission: WorkAdmission,
}

/// Why irreversible deletion was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum DeletionRejection {
    /// Another operation already owns the deletion.
    #[error("session deletion operation {0} is already in progress")]
    InProgress(OperationId),
    /// The session is already deleted.
    #[error("session was deleted by operation {0}")]
    Deleted(OperationId),
    /// Completion was attempted from a non-deleting state.
    #[error("session deletion state is {0:?}")]
    NotDeleting(DeletionState),
    /// One cleanup owner has not yet made its evidence durable.
    #[error("session deletion evidence is missing `{item}`")]
    EvidenceIncomplete {
        /// The first missing predicate.
        item: &'static str,
    },
}

/// Crosses the irreversible deletion fence.
///
/// Repeating the same operation is idempotent. A different operation cannot
/// replace the elected owner.
///
/// # Errors
///
/// Returns [`DeletionRejection`] when another operation owns the fence or the
/// session is already deleted.
pub fn begin_delete(
    guard: &DeletionGuard,
    operation: OperationId,
) -> Result<DeleteCommit, DeletionRejection> {
    match guard.state {
        DeletionState::Live => Ok(DeleteCommit {
            guard: DeletionGuard {
                state: DeletionState::Deleting,
                epoch: guard.epoch.next(),
                delete_operation: Some(operation),
                ..*guard
            },
            admission: WorkAdmission::Deleting,
        }),
        DeletionState::Deleting => match guard.delete_operation {
            Some(owner) if owner == operation => Ok(DeleteCommit {
                guard: *guard,
                admission: WorkAdmission::Deleting,
            }),
            Some(owner) => Err(DeletionRejection::InProgress(owner)),
            None => Err(DeletionRejection::InProgress(operation)),
        },
        DeletionState::Deleted => Err(DeletionRejection::Deleted(
            guard.delete_operation.unwrap_or(operation),
        )),
    }
}

/// Completes deletion after every required fact is durable.
///
/// # Errors
///
/// Returns [`DeletionRejection::EvidenceIncomplete`] for the first missing
/// fact and [`DeletionRejection::NotDeleting`] unless the destructive fence was
/// previously crossed.
pub fn complete_delete(
    guard: &DeletionGuard,
    workspace: WorkspaceId,
    evidence: &DeleteEvidence,
    now: Timestamp,
) -> Result<SessionTombstone, DeletionRejection> {
    if guard.state != DeletionState::Deleting {
        return Err(DeletionRejection::NotDeleting(guard.state));
    }
    if let Some(item) = evidence.missing() {
        return Err(DeletionRejection::EvidenceIncomplete { item });
    }
    let deleted_by = guard
        .delete_operation
        .ok_or(DeletionRejection::NotDeleting(guard.state))?;
    Ok(SessionTombstone {
        session: guard.session,
        workspace,
        deleted_by,
        epoch: guard.epoch,
        deleted_at: now,
    })
}

#[cfg(test)]
mod tests {
    use aex_wire::ids::{OperationId, PrefixedId, SessionId, Uuid7, WorkspaceId};
    use aex_wire::types::Timestamp;

    use super::{
        DeleteEvidence, DeletionGuard, DeletionRejection, DeletionState, begin_delete,
        complete_delete,
    };
    use crate::session::WorkAdmission;

    fn id<T: PrefixedId>(tag: u8) -> T {
        T::from_uuid7(Uuid7::compose(1, [tag; 10]))
    }

    fn complete_evidence() -> DeleteEvidence {
        DeleteEvidence {
            generation_terminated: true,
            session_content_removed: true,
            messages_removed: true,
            brain_user_content_removed: true,
            observations_removed: true,
            export_objects_removed: true,
            billing_aggregate_retained: true,
            audit_fact_retained: true,
        }
    }

    #[test]
    fn delete_advances_once_and_the_same_operation_replays() {
        let live = DeletionGuard::live(id::<SessionId>(1));
        let first = begin_delete(&live, id::<OperationId>(2)).expect("claims");
        assert_eq!(first.guard.state, DeletionState::Deleting);
        assert_eq!(first.guard.epoch, live.epoch.next());
        assert_eq!(first.admission, WorkAdmission::Deleting);
        assert_eq!(begin_delete(&first.guard, id::<OperationId>(2)), Ok(first));
        assert_eq!(
            begin_delete(&first.guard, id::<OperationId>(3)),
            Err(DeletionRejection::InProgress(id::<OperationId>(2)))
        );
    }

    #[test]
    fn completion_requires_every_owner_and_yields_only_a_minimal_tombstone() {
        let deleting = begin_delete(
            &DeletionGuard::live(id::<SessionId>(1)),
            id::<OperationId>(2),
        )
        .expect("claims")
        .guard;
        let workspace = id::<WorkspaceId>(3);
        let now = Timestamp::from_unix_millis(10).expect("in range");
        let mut evidence = complete_evidence();
        evidence.observations_removed = false;
        assert_eq!(
            complete_delete(&deleting, workspace, &evidence, now),
            Err(DeletionRejection::EvidenceIncomplete {
                item: "observations_removed"
            })
        );
        let tombstone = complete_delete(&deleting, workspace, &complete_evidence(), now)
            .expect("complete evidence");
        assert_eq!(tombstone.session, id::<SessionId>(1));
        assert_eq!(tombstone.workspace, workspace);
        assert_eq!(tombstone.deleted_by, id::<OperationId>(2));
        assert_eq!(tombstone.deleted_at, now);
    }
}
