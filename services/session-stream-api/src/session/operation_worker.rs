//! Post-commit dispatch to the isolated session-operation Lambda.
//!
//! The API owns admission and the worker owns continuation. This adapter uses
//! Lambda's asynchronous invocation queue, so the ECS request holds no worker
//! connection while suspend, resume, termination or deletion progresses.

use aex_internal_contracts::operation::SessionOperationWake;
use aex_operation_domain::{Operation, WorkId};
use aex_wire::ids::{OperationId, PrefixedId as _, WorkspaceId};
use aws_sdk_lambda::primitives::Blob;
use aws_sdk_lambda::types::InvocationType;

/// Dispatches one already-committed durable operation.
#[async_trait::async_trait]
pub trait OperationWorkerInvoker: Send + Sync {
    /// Asks Lambda to begin or resume reconciliation.
    ///
    /// The wake is non-authoritative and may be delivered more than once. The
    /// worker's strong read and fenced regional-work claim own idempotency.
    async fn invoke(&self, wake: &SessionOperationWake) -> Result<(), InvokeError>;
}

/// Production asynchronous Lambda invoker, bound to one exact live alias.
#[derive(Debug, Clone)]
pub struct LambdaOperationWorkerInvoker {
    client: aws_sdk_lambda::Client,
    function_arn: String,
}

impl LambdaOperationWorkerInvoker {
    /// Binds the invoker to the configured qualified function ARN.
    #[must_use]
    pub const fn new(client: aws_sdk_lambda::Client, function_arn: String) -> Self {
        Self {
            client,
            function_arn,
        }
    }
}

#[async_trait::async_trait]
impl OperationWorkerInvoker for LambdaOperationWorkerInvoker {
    async fn invoke(&self, wake: &SessionOperationWake) -> Result<(), InvokeError> {
        let payload = serde_json::to_vec(wake).map_err(|_| InvokeError::Encode)?;
        let response = self
            .client
            .invoke()
            .function_name(&self.function_arn)
            .invocation_type(InvocationType::Event)
            .payload(Blob::new(payload))
            .send()
            .await
            .map_err(|_| InvokeError::Unavailable)?;
        if response.status_code() == 202 {
            Ok(())
        } else {
            Err(InvokeError::Rejected)
        }
    }
}

/// Derives the one work identity lifecycle admission stores for an operation.
#[must_use]
pub fn wake_for(workspace: WorkspaceId, operation: OperationId) -> SessionOperationWake {
    SessionOperationWake::new(workspace, WorkId(operation.uuid7()).to_string())
        .expect("an OperationId always derives a valid regional-work identity")
}

/// Invokes the worker for one nonterminal operation and no-ops for a completed
/// replay.
pub async fn invoke_pending(
    invoker: &dyn OperationWorkerInvoker,
    workspace: WorkspaceId,
    operation: &Operation,
) -> Result<(), InvokeError> {
    if operation.status.is_terminal() {
        return Ok(());
    }
    invoker.invoke(&wake_for(workspace, operation.id)).await
}

/// Why Lambda did not accept an asynchronous wake.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum InvokeError {
    /// The internal contract could not be encoded.
    #[error("the session operation wake could not be encoded")]
    Encode,
    /// The Lambda control plane did not accept the request.
    #[error("the session operation worker is unavailable")]
    Unavailable,
    /// Lambda returned a status other than asynchronous acceptance.
    #[error("the session operation worker rejected the asynchronous wake")]
    Rejected,
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use aex_internal_contracts::operation::SessionOperationWake;
    use aex_operation_domain::WorkId;
    use aex_operation_domain::{Operation, OperationKind, OperationScope, OperationStatus};
    use aex_wire::idempotency::IntentDigest;
    use aex_wire::ids::{OperationId, PrefixedId as _, Uuid7, WorkspaceId};

    use super::{InvokeError, OperationWorkerInvoker, invoke_pending, wake_for};

    #[derive(Default)]
    struct RecordingInvoker(Mutex<Vec<SessionOperationWake>>);

    #[async_trait::async_trait]
    impl OperationWorkerInvoker for RecordingInvoker {
        async fn invoke(&self, wake: &SessionOperationWake) -> Result<(), InvokeError> {
            self.0.lock().expect("recorder lock").push(wake.clone());
            Ok(())
        }
    }

    fn operation(workspace: WorkspaceId, status: OperationStatus) -> Operation {
        Operation {
            id: OperationId::from_uuid7(Uuid7::compose(2, [2; 10])),
            workspace,
            session: None,
            kind: OperationKind::SessionTerminate,
            status,
            intent: IntentDigest::from_bytes([3; 32]),
            scope: OperationScope::Workspace(workspace),
            progress: None,
            cursor: None,
            result: None,
            error: None,
            cancel_requested: false,
            created_at: aex_wire::types::Timestamp::from_unix_millis(1).expect("timestamp"),
            started_at: None,
            updated_at: aex_wire::types::Timestamp::from_unix_millis(1).expect("timestamp"),
            committed_at: None,
            terminal_at: status
                .is_terminal()
                .then(|| aex_wire::types::Timestamp::from_unix_millis(1).expect("timestamp")),
        }
    }

    #[test]
    fn an_operation_wake_is_exactly_tenant_and_work_bound() {
        let workspace = WorkspaceId::from_uuid7(Uuid7::compose(1, [1; 10]));
        let operation = OperationId::from_uuid7(Uuid7::compose(2, [2; 10]));
        let wake = wake_for(workspace, operation);
        assert_eq!(wake.workspace, workspace);
        assert_eq!(wake.work_id, WorkId(operation.uuid7()).to_string());
        assert_eq!(wake.schema_version.0, 1);
    }

    #[tokio::test]
    async fn only_a_nonterminal_operation_reaches_lambda() {
        let workspace = WorkspaceId::from_uuid7(Uuid7::compose(1, [1; 10]));
        let invoker = RecordingInvoker::default();
        invoke_pending(
            &invoker,
            workspace,
            &operation(workspace, OperationStatus::Queued),
        )
        .await
        .expect("queued operation wakes");
        invoke_pending(
            &invoker,
            workspace,
            &operation(workspace, OperationStatus::Succeeded),
        )
        .await
        .expect("completed replay is a no-op");
        assert_eq!(invoker.0.lock().expect("recorder lock").len(), 1);
    }
}
