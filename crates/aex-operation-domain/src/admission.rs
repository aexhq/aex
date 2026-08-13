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
    Execution, Operation, OperationKind, OperationResult, OperationScope, OperationStatus,
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
    /// The execution this **request** resolved to, when the caller classified
    /// it per request rather than per kind (D-2).
    ///
    /// `None` means "the kind's default". A declaration may only escalate
    /// `Inline` to `Continued`, never reverse a kind's durability requirement.
    pub execution: Option<Execution>,
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
    /// The same operation id names another resource.
    WrongScope,
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
            Self::WrongWorkspace | Self::WrongScope => ErrorCode::NotFound,
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
    /// Irreversible deletion already holds the session.
    DeletionInProgress {
        /// The operation that claimed it.
        operation: OperationId,
    },
    /// The session is gone except for its tombstone.
    SessionDeleted {
        /// The operation that deleted it.
        operation: OperationId,
    },
}

/// Admits an operation.
///
/// Ordering matters and is fixed: identity is resolved first, so the same
/// identity always replays its original outcome even during and after deletion;
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
        if existing.session != request.session || existing.scope != request.scope {
            return AdmissionOutcome::Conflict(ConflictCode::WrongScope);
        }
        if existing.intent != request.intent {
            return AdmissionOutcome::Conflict(ConflictCode::OperationIdempotencyConflict);
        }
        return AdmissionOutcome::Replay(Box::new(existing.clone()));
    }

    if let Some(guard) = guard {
        match guard.state {
            DeletionState::Deleting => {
                if let Some(operation) = guard.delete_operation {
                    return AdmissionOutcome::DeletionInProgress { operation };
                }
            }
            DeletionState::Deleted => {
                if let Some(operation) = guard.delete_operation {
                    return AdmissionOutcome::SessionDeleted { operation };
                }
            }
            DeletionState::Live => {}
        }
    }

    // A per-request classification may only escalate. `max` over the
    // `Inline < Continued` ordering makes that structural rather than reviewed:
    // a caller that hands back `Inline` for a kind whose default is `Continued`
    // still gets `Continued`.
    let resolved = request
        .execution
        .map_or(request.kind.execution(), |declared| {
            declared.max(request.kind.execution())
        });
    let inline = resolved == Execution::Inline;
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
            execution: None,
        }
    }

    fn deleting() -> DeletionGuard {
        DeletionGuard {
            session: SessionId::from_uuid7(Uuid7::compose(1, [3; 10])),
            state: DeletionState::Deleting,
            epoch: DeletionEpoch(1),
            delete_operation: Some(OperationId::from_uuid7(Uuid7::compose(1, [9; 10]))),
        }
    }

    #[test]
    fn a_per_request_classification_cannot_de_escalate_a_session_command() {
        let mut reduced = request(OperationKind::SessionCancel);
        reduced.execution = Some(crate::operation::Execution::Inline);
        let AdmissionOutcome::Inserted(created) = admit(None, None, &reduced, moment(1)) else {
            panic!("expected an insert");
        };
        assert_eq!(created.status, OperationStatus::Queued);
    }

    #[test]
    fn a_continued_kind_is_created_queued() {
        let AdmissionOutcome::Inserted(created) =
            admit(None, None, &request(OperationKind::ContentGc), moment(1))
        else {
            panic!("expected an insert");
        };
        assert_eq!(created.status, OperationStatus::Queued);
        assert_eq!(created.committed_at, None);
    }

    #[test]
    fn the_same_identity_always_replays_even_during_deletion() {
        let existing = match admit(
            None,
            None,
            &request(OperationKind::SessionCancel),
            moment(1),
        ) {
            AdmissionOutcome::Inserted(created) => *created,
            other => panic!("expected an insert, got {other:?}"),
        };
        assert_eq!(
            admit(
                Some(&existing),
                Some(&deleting()),
                &request(OperationKind::SessionCancel),
                moment(2)
            ),
            AdmissionOutcome::Replay(Box::new(existing))
        );
    }

    #[test]
    fn a_new_identity_during_deletion_is_deletion_in_progress() {
        assert_eq!(
            admit(
                None,
                Some(&deleting()),
                &request(OperationKind::SessionCancel),
                moment(2)
            ),
            AdmissionOutcome::DeletionInProgress {
                operation: OperationId::from_uuid7(Uuid7::compose(1, [9; 10]))
            }
        );
    }

    #[test]
    fn a_reused_identity_with_a_different_intent_conflicts() {
        let existing = match admit(
            None,
            None,
            &request(OperationKind::SessionCancel),
            moment(1),
        ) {
            AdmissionOutcome::Inserted(created) => *created,
            other => panic!("expected an insert, got {other:?}"),
        };
        let mut different = request(OperationKind::SessionCancel);
        different.intent = IntentDigest::from_bytes([7; 32]);
        assert_eq!(
            admit(Some(&existing), None, &different, moment(2)),
            AdmissionOutcome::Conflict(ConflictCode::OperationIdempotencyConflict)
        );
    }

    #[test]
    fn an_operation_identity_never_replays_across_sessions() {
        let existing = match admit(
            None,
            None,
            &request(OperationKind::SessionCancel),
            moment(1),
        ) {
            AdmissionOutcome::Inserted(created) => *created,
            other => panic!("expected an insert, got {other:?}"),
        };
        let mut foreign = request(OperationKind::SessionCancel);
        let foreign_session = SessionId::from_uuid7(Uuid7::compose(2, [8; 10]));
        foreign.session = Some(foreign_session);
        foreign.scope = OperationScope::Session(foreign_session);
        assert_eq!(
            admit(Some(&existing), None, &foreign, moment(2)),
            AdmissionOutcome::Conflict(ConflictCode::WrongScope)
        );
    }
}
