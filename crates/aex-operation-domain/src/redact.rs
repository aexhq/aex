//! Purge redaction.
//!
//! When a session is purged, the operations it already produced must stop
//! carrying its content — but the envelope survives, so `GET /operations/{id}`
//! still answers and the measurement identity still reconciles against usage.
//! Stop, trash and purge results are content-free receipts and survive intact.

use crate::operation::{Operation, OperationCommit, OperationFailure, OperationResult};

/// Strips the content-bearing result and error from one operation.
///
/// Returns `None` when nothing would change, so a caller writes nothing rather
/// than churning a revision for no reason.
#[must_use]
pub fn redact_for_purge(operation: &Operation) -> Option<OperationCommit> {
    if operation.kind.result_survives_purge() {
        return None;
    }
    let content_bearing = operation
        .result
        .as_ref()
        .is_some_and(OperationResult::is_content_bearing)
        || operation
            .error
            .as_ref()
            .is_some_and(|failure| failure.detail.is_some());
    if !content_bearing {
        return None;
    }
    let mut next = operation.clone();
    next.result = next.result.map(|result| OperationResult {
        measurement: result.measurement,
        content: None,
    });
    next.error = next.error.map(|failure| OperationFailure {
        detail: None,
        ..failure
    });
    Some(OperationCommit {
        operation: next,
        latched_commit: false,
    })
}

#[cfg(test)]
mod tests {
    use aex_wire::error::ErrorCode;
    use aex_wire::idempotency::IntentDigest;
    use aex_wire::ids::{MeasurementId, OperationId, PrefixedId as _, Uuid7, WorkspaceId};
    use aex_wire::types::Timestamp;

    use super::redact_for_purge;
    use crate::operation::{
        FailureClass, Operation, OperationFailure, OperationKind, OperationResult, OperationScope,
        OperationStatus,
    };

    fn content() -> aex_wire::CanonicalJson {
        aex_wire::CanonicalJson::parse("{\"path\":\"secret.txt\"}").expect("canonical")
    }

    fn operation(kind: OperationKind) -> Operation {
        let workspace = WorkspaceId::from_uuid7(Uuid7::compose(1, [1; 10]));
        Operation {
            id: OperationId::from_uuid7(Uuid7::compose(1, [2; 10])),
            workspace,
            session: None,
            kind,
            status: OperationStatus::Succeeded,
            intent: IntentDigest::from_bytes([0; 32]),
            scope: OperationScope::Workspace(workspace),
            progress: None,
            cursor: None,
            cancel_requested: false,
            result: Some(OperationResult {
                measurement: Some(MeasurementId::from_uuid7(Uuid7::compose(1, [3; 10]))),
                content: Some(content()),
            }),
            error: None,
            created_at: Timestamp::from_unix_millis(0).expect("in range"),
            started_at: None,
            updated_at: Timestamp::from_unix_millis(0).expect("in range"),
            committed_at: None,
            terminal_at: None,
        }
    }

    #[test]
    fn redaction_keeps_the_envelope_and_the_measurement() {
        let commit = redact_for_purge(&operation(OperationKind::SessionPersist)).expect("redacts");
        let before = operation(OperationKind::SessionPersist);
        assert_eq!(commit.operation.id, before.id);
        assert_eq!(commit.operation.status, before.status);
        assert_eq!(commit.operation.intent, before.intent);
        let result = commit.operation.result.expect("kept");
        assert_eq!(
            result.measurement,
            Some(MeasurementId::from_uuid7(Uuid7::compose(1, [3; 10])))
        );
        assert_eq!(result.content, None);
    }

    #[test]
    fn content_free_receipts_survive() {
        for kind in [
            OperationKind::SessionStop,
            OperationKind::SessionTrash,
            OperationKind::SessionPurge,
        ] {
            assert_eq!(redact_for_purge(&operation(kind)), None);
        }
    }

    #[test]
    fn an_already_content_free_operation_is_not_rewritten() {
        let mut bare = operation(OperationKind::SessionPersist);
        bare.result = Some(OperationResult::receipt());
        assert_eq!(redact_for_purge(&bare), None);
    }

    #[test]
    fn an_error_detail_is_stripped_but_its_code_is_kept() {
        let mut failed = operation(OperationKind::TelemetryExport);
        failed.result = None;
        failed.status = OperationStatus::Failed;
        failed.error = Some(OperationFailure {
            code: ErrorCode::InvalidRequest,
            class: FailureClass::Terminal,
            detail: Some(content()),
        });
        let commit = redact_for_purge(&failed).expect("redacts");
        let error = commit.operation.error.expect("kept");
        assert_eq!(error.code, ErrorCode::InvalidRequest);
        assert_eq!(error.detail, None);
    }
}
