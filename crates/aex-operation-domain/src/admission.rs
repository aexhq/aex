//! Operation admission.
//!
//! `admit` is total over `(existing, guard, request)`: every combination maps to
//! exactly one [`AdmissionOutcome`], so there is no path where a caller gets an
//! ambiguous answer about whether its operation exists.

use aex_wire::error::ErrorCode;
use aex_wire::idempotency::IntentDigest;
use aex_wire::ids::{OperationId, SessionId, WorkspaceId};
use aex_wire::types::Timestamp;

use crate::deletion::{DeletionGuard, DeletionState};
use crate::operation::{
    Operation, OperationKind, OperationResult, OperationScope, OperationStatus,
};

/// What an admission asks for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdmitRequest {
    /// The caller-minted identity.
    pub id: OperationId,
    /// The owning workspace.
    pub workspace: WorkspaceId,
    /// The session, when the operation names one.
    pub session: Option<SessionId>,
    /// What the operation does.
    pub kind: OperationKind,
    /// What was asked for.
    pub intent: IntentDigest,
    /// What it acts on.
    pub scope: OperationScope,
    /// The result an inline operation is created already carrying (D-07).
    pub inline_result: Option<OperationResult>,
}

/// Which conflict a mismatched replay is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ConflictCode {
    /// The same operation id was reused for a different intent.
    OperationIdempotencyConflict,
    /// The same operation id names a different workspace.
    WrongWorkspace,
    /// The same operation id names a different kind.
    WrongKind,
}

impl ConflictCode {
    /// The stable public code.
    #[must_use]
    pub const fn code(self) -> ErrorCode {
        match self {
            Self::OperationIdempotencyConflict | Self::WrongKind => {
                ErrorCode::OperationIdempotencyConflict
            }
            // A foreign workspace is never told the record exists.
            Self::WrongWorkspace => ErrorCode::NotFound,
        }
    }
}

/// What admission decided.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AdmissionOutcome {
    /// A new operation was created.
    Inserted(Box<Operation>),
    /// The same identity and intent replays the original.
    Replay(Box<Operation>),
    /// The same identity was reused for a different intent.
    Conflict(ConflictCode),
    /// A purge already holds the session.
    DeletionInProgress {
        /// The operation that claimed it.
        operation: OperationId,
    },
    /// The session is gone.
    SessionPurged {
        /// The operation that purged it.
        operation: OperationId,
    },
}

/// Admits an operation.
///
/// Ordering matters and is fixed: identity is resolved first, so the same
/// identity always replays its original outcome even during and after a purge;
/// only a **new** identity meets the deletion fence.
#[must_use]
pub fn admit(
    existing: Option<&Operation>,
    guard: Option<&DeletionGuard>,
    request: &AdmitRequest,
    now: Timestamp,
) -> AdmissionOutcome {
    if let Some(existing) = existing {
        if existing.workspace != request.workspace {
            return AdmissionOutcome::Conflict(ConflictCode::WrongWorkspace);
        }
        if existing.kind != request.kind {
            return AdmissionOutcome::Conflict(ConflictCode::WrongKind);
        }
        if existing.intent != request.intent {
            return AdmissionOutcome::Conflict(ConflictCode::OperationIdempotencyConflict);
        }
        return AdmissionOutcome::Replay(Box::new(existing.clone()));
    }

    if let Some(guard) = guard
        && !request.kind.claims_session_deletion()
    {
        match guard.state {
            DeletionState::Purging => {
                if let Some(operation) = guard.purge_operation {
                    return AdmissionOutcome::DeletionInProgress { operation };
                }
            }
            DeletionState::Purged => {
                if let Some(operation) = guard.purge_operation {
                    return AdmissionOutcome::SessionPurged { operation };
                }
            }
            DeletionState::Live | DeletionState::Trashed => {}
        }
    }

    let inline = request.kind.execution() == crate::operation::Execution::Inline;
    let status = if inline {
        OperationStatus::Succeeded
    } else {
        OperationStatus::Queued
    };
    AdmissionOutcome::Inserted(Box::new(Operation {
        id: request.id,
        workspace: request.workspace,
        session: request.session,
        kind: request.kind,
        status,
        intent: request.intent,
        scope: request.scope,
        progress: None,
        cursor: None,
        cancel_requested: false,
        result: if inline {
            Some(
                request
                    .inline_result
                    .clone()
                    .unwrap_or_else(OperationResult::receipt),
            )
        } else {
            None
        },
        error: None,
        created_at: now,
        started_at: if inline { Some(now) } else { None },
        updated_at: now,
        committed_at: if inline { Some(now) } else { None },
        terminal_at: if inline { Some(now) } else { None },
    }))
}

#[cfg(test)]
mod tests {
    use aex_wire::idempotency::IntentDigest;
    use aex_wire::ids::{OperationId, PrefixedId as _, SessionId, Uuid7, WorkspaceId};
    use aex_wire::types::Timestamp;

    use super::{AdmissionOutcome, AdmitRequest, ConflictCode, admit};
    use crate::deletion::{DeletionEpoch, DeletionGuard, DeletionState};
    use crate::operation::{OperationKind, OperationScope, OperationStatus};

    fn moment(millis: i64) -> Timestamp {
        Timestamp::from_unix_millis(millis).expect("in range")
    }

    fn request(kind: OperationKind) -> AdmitRequest {
        let workspace = WorkspaceId::from_uuid7(Uuid7::compose(1, [1; 10]));
        let session = SessionId::from_uuid7(Uuid7::compose(1, [3; 10]));
        AdmitRequest {
            id: OperationId::from_uuid7(Uuid7::compose(1, [2; 10])),
            workspace,
            session: Some(session),
            kind,
            intent: IntentDigest::from_bytes([1; 32]),
            scope: OperationScope::Session(session),
            inline_result: None,
        }
    }

    fn purging() -> DeletionGuard {
        DeletionGuard {
            session: SessionId::from_uuid7(Uuid7::compose(1, [3; 10])),
            state: DeletionState::Purging,
            epoch: DeletionEpoch(2),
            trashed_at: Some(moment(1)),
            recovery_deadline: None,
            purge_operation: Some(OperationId::from_uuid7(Uuid7::compose(1, [9; 10]))),
        }
    }

    #[test]
    fn an_inline_kind_is_created_already_succeeded() {
        let AdmissionOutcome::Inserted(created) =
            admit(None, None, &request(OperationKind::SessionStop), moment(1))
        else {
            panic!("expected an insert");
        };
        assert_eq!(created.status, OperationStatus::Succeeded);
        assert_eq!(created.committed_at, Some(moment(1)));
        assert!(created.result.is_some());
    }

    #[test]
    fn a_continued_kind_is_created_queued() {
        let AdmissionOutcome::Inserted(created) = admit(
            None,
            None,
            &request(OperationKind::TelemetryExport),
            moment(1),
        ) else {
            panic!("expected an insert");
        };
        assert_eq!(created.status, OperationStatus::Queued);
        assert_eq!(created.committed_at, None);
    }

    #[test]
    fn the_same_identity_always_replays_even_during_a_purge() {
        let existing = match admit(None, None, &request(OperationKind::SessionStop), moment(1)) {
            AdmissionOutcome::Inserted(created) => *created,
            other => panic!("expected an insert, got {other:?}"),
        };
        assert_eq!(
            admit(
                Some(&existing),
                Some(&purging()),
                &request(OperationKind::SessionStop),
                moment(2)
            ),
            AdmissionOutcome::Replay(Box::new(existing))
        );
    }

    #[test]
    fn a_new_identity_during_a_purge_is_deletion_in_progress() {
        assert_eq!(
            admit(
                None,
                Some(&purging()),
                &request(OperationKind::SessionPersist),
                moment(2)
            ),
            AdmissionOutcome::DeletionInProgress {
                operation: OperationId::from_uuid7(Uuid7::compose(1, [9; 10]))
            }
        );
    }

    #[test]
    fn a_reused_identity_with_a_different_intent_conflicts() {
        let existing = match admit(None, None, &request(OperationKind::SessionStop), moment(1)) {
            AdmissionOutcome::Inserted(created) => *created,
            other => panic!("expected an insert, got {other:?}"),
        };
        let mut different = request(OperationKind::SessionStop);
        different.intent = IntentDigest::from_bytes([7; 32]);
        assert_eq!(
            admit(Some(&existing), None, &different, moment(2)),
            AdmissionOutcome::Conflict(ConflictCode::OperationIdempotencyConflict)
        );
    }
}
