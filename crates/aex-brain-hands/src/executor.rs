//! Tool-router bridge for the exact-generation Hands port.
//!
//! The global router retains no tenant state. Each prepared call carries the immutable
//! session generation, and the detached operation reference persists that generation plus
//! the result bounds needed after a process restart. A query therefore never discovers a
//! current generation or silently follows a successor with a different filesystem.

use std::sync::Arc;

use aex_brain_application::ports::{
    BoxFuture, CancelToken, DetachedStatus, DispatchTicket, HandsError, HandsOperationStart,
    HandsOperationStatus, HandsPort, HandsResult, PreparedToolCall, ProviderFailureClass,
    ProviderFailureKind, RedactedDetail, ResultBounds, ToolDispatchError, ToolOutcome,
    ToolResultBody,
};
use aex_brain_domain::effect::{DispatchProof, DispatchStage};
use aex_brain_domain::ids::{ContentHash, DetachedOperationId, Fence, HandsOperationId};
use aex_brain_domain::journal::ExecutorRoute;
use aex_brain_tool_catalog::router::ToolExecutor;
use aex_hands_protocol::operation::OperationRequest;
use aex_model_catalog::canonical::ToolResultPart;
use aex_wire::CanonicalJson;
use aex_wire::ids::{GenerationId, PrefixedId as _, ResourceName, Uuid7};

use crate::{CONTEXT_TOOL_RESULT_BYTES, MAX_RESULT_BODY_BYTES};

const DETACHED_PREFIX: &str = "hands.v1";

/// Adapts the exact-generation Hands port to the coarse tool-router executor seam.
///
/// Calls are encoded as protocol `registered_tool` operations after their arguments have
/// already passed the pinned catalog schema. The guest image owns the corresponding
/// registered implementation; this bridge never reinterprets model arguments as shell text.
pub struct HandsToolExecutor {
    hands: Arc<dyn HandsPort>,
}

impl HandsToolExecutor {
    /// Binds the exact-generation Hands port.
    #[must_use]
    pub fn new(hands: Arc<dyn HandsPort>) -> Self {
        Self { hands }
    }

    async fn completed(
        &self,
        detached: &DetachedHands,
    ) -> Result<DetachedStatus, ToolDispatchError> {
        let result = self
            .hands
            .result(
                detached.generation,
                &detached.operation,
                &ResultBounds {
                    max_bytes: detached.max_bytes,
                    max_stream_bytes: detached
                        .max_bytes
                        .min(usize::try_from(CONTEXT_TOOL_RESULT_BYTES).unwrap_or(usize::MAX)),
                    timeout_ms: detached.timeout_ms,
                },
            )
            .await
            .map_err(hands_error)?;
        incorporate_result(result, detached.max_bytes)
    }
}

impl core::fmt::Debug for HandsToolExecutor {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("HandsToolExecutor")
            .finish_non_exhaustive()
    }
}

impl ToolExecutor for HandsToolExecutor {
    fn invoke<'a>(
        &'a self,
        ticket: &'a DispatchTicket,
        call: &'a PreparedToolCall,
        cancel: &'a CancelToken,
    ) -> BoxFuture<'a, Result<ToolOutcome, ToolDispatchError>> {
        Box::pin(async move {
            if cancel.is_cancelled() {
                return Err(dispatch_error(
                    DispatchStage::PreDispatch,
                    DispatchProof::NotSent,
                    ProviderFailureKind::Cancelled,
                    "Hands call cancelled before dispatch",
                ));
            }
            if call.route.executor != ExecutorRoute::Hands {
                return Err(dispatch_error(
                    DispatchStage::PreDispatch,
                    DispatchProof::NotSent,
                    ProviderFailureKind::InvalidRequest,
                    "Hands executor received a call pinned to another route",
                ));
            }
            let max_bytes = call
                .max_result_bytes
                .min(usize::try_from(MAX_RESULT_BODY_BYTES).unwrap_or(usize::MAX));
            if max_bytes == 0 || call.route.timeout_ms == 0 {
                return Err(dispatch_error(
                    DispatchStage::PreDispatch,
                    DispatchProof::NotSent,
                    ProviderFailureKind::InvalidRequest,
                    "Hands call has a zero result or wall-clock bound",
                ));
            }
            let operation = operation_for(ticket);
            let request = OperationRequest::RegisteredTool {
                name: ResourceName::parse(call.route.name.as_str()).map_err(|_| {
                    dispatch_error(
                        DispatchStage::PreDispatch,
                        DispatchProof::NotSent,
                        ProviderFailureKind::InvalidRequest,
                        "Hands tool name is outside the registered-tool grammar",
                    )
                })?,
                manifest: aex_wire::ids::ContentHash::from_bytes(call.route.manifest_digest.0),
                args: call.input.clone(),
            };
            let request_value = serde_json::to_value(&request).map_err(|_| {
                dispatch_error(
                    DispatchStage::PreDispatch,
                    DispatchProof::NotSent,
                    ProviderFailureKind::InvalidRequest,
                    "Hands operation request could not be encoded",
                )
            })?;
            let request_canonical = CanonicalJson::from_value(&request_value).map_err(|_| {
                dispatch_error(
                    DispatchStage::PreDispatch,
                    DispatchProof::NotSent,
                    ProviderFailureKind::InvalidRequest,
                    "Hands operation request could not be canonicalized",
                )
            })?;
            let bounds = ResultBounds {
                max_bytes,
                max_stream_bytes: max_bytes
                    .min(usize::try_from(CONTEXT_TOOL_RESULT_BYTES).unwrap_or(usize::MAX)),
                timeout_ms: call.route.timeout_ms,
            };
            let start = HandsOperationStart {
                operation: operation.clone(),
                call_hash: ContentHash::of(request_canonical.as_bytes()),
                request: request_value,
                bounds,
                deadline: ticket
                    .issued_at()
                    .plus_millis(i64::from(call.route.timeout_ms)),
            };
            let accepted = self
                .hands
                .start(ticket, call.hands_generation, &start)
                .await
                .map_err(hands_error)?;
            let durable = encode_detached(
                accepted.generation,
                &accepted.operation,
                max_bytes,
                call.route.timeout_ms,
            );
            Ok(ToolOutcome::Detached {
                operation: durable,
                poll_after: accepted.poll_after,
            })
        })
    }

    fn query<'a>(
        &'a self,
        operation: &'a DetachedOperationId,
    ) -> BoxFuture<'a, Result<DetachedStatus, ToolDispatchError>> {
        Box::pin(async move {
            let detached = decode_detached(operation)?;
            match self
                .hands
                .status(detached.generation, &detached.operation)
                .await
                .map_err(hands_error)?
            {
                HandsOperationStatus::Running { poll_after } => {
                    Ok(DetachedStatus::Running { poll_after })
                }
                HandsOperationStatus::Completed { .. } => self.completed(&detached).await,
                HandsOperationStatus::Failed { reason } => Ok(DetachedStatus::Failed {
                    reason: reason.as_str().to_owned(),
                }),
                HandsOperationStatus::Cancelled => Ok(DetachedStatus::Failed {
                    reason: "Hands operation was cancelled".to_owned(),
                }),
                HandsOperationStatus::GenerationLost => Ok(DetachedStatus::Failed {
                    reason: "the exact Hands generation is no longer available".to_owned(),
                }),
            }
        })
    }

    fn cancel<'a>(
        &'a self,
        operation: &'a DetachedOperationId,
        fence: Fence,
    ) -> BoxFuture<'a, Result<(), ToolDispatchError>> {
        Box::pin(async move {
            let detached = decode_detached(operation)?;
            self.hands
                .cancel(detached.generation, &detached.operation, fence)
                .await
                .map_err(hands_error)
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct DetachedHands {
    generation: GenerationId,
    operation: HandsOperationId,
    max_bytes: usize,
    timeout_ms: u32,
}

fn incorporate_result(
    result: HandsResult,
    max_bytes: usize,
) -> Result<DetachedStatus, ToolDispatchError> {
    if result.truncated {
        return Err(dispatch_error(
            DispatchStage::Terminal,
            DispatchProof::ResponseStarted,
            ProviderFailureKind::ProtocolViolation,
            "Hands result was truncated and cannot be journalled as complete",
        ));
    }
    let value = match (result.inline, result.placed) {
        (Some(inline), None) => {
            if ContentHash::of(inline.as_bytes()) != result.checksum {
                return Err(dispatch_error(
                    DispatchStage::Terminal,
                    DispatchProof::ResponseStarted,
                    ProviderFailureKind::ProtocolViolation,
                    "Hands result disagrees with its verified body checksum",
                ));
            }
            serde_json::from_str(&inline).map_err(|_| {
                dispatch_error(
                    DispatchStage::Terminal,
                    DispatchProof::ResponseStarted,
                    ProviderFailureKind::ProtocolViolation,
                    "Hands result is not the registered tool's JSON result",
                )
            })?
        }
        (None, Some(placed)) => serde_json::to_value(placed).map_err(|_| {
            dispatch_error(
                DispatchStage::Terminal,
                DispatchProof::ResponseStarted,
                ProviderFailureKind::ProtocolViolation,
                "Hands placed result reference could not be encoded",
            )
        })?,
        _ => {
            return Err(dispatch_error(
                DispatchStage::Terminal,
                DispatchProof::ResponseStarted,
                ProviderFailureKind::ProtocolViolation,
                "Hands result must carry exactly one inline or placed body",
            ));
        }
    };
    let canonical = CanonicalJson::from_value(&value).map_err(|_| {
        dispatch_error(
            DispatchStage::Terminal,
            DispatchProof::ResponseStarted,
            ProviderFailureKind::ProtocolViolation,
            "Hands result could not be canonicalized",
        )
    })?;
    if canonical.as_bytes().len() > max_bytes {
        return Err(dispatch_error(
            DispatchStage::Terminal,
            DispatchProof::ResponseStarted,
            ProviderFailureKind::ProtocolViolation,
            "Hands result exceeds its persisted result bound",
        ));
    }
    let checksum = ContentHash::of(canonical.as_bytes());
    Ok(DetachedStatus::Completed(Box::new(ToolResultBody {
        content: vec![ToolResultPart::Json { value: canonical }],
        is_error: result.exit_code != 0,
        duration_ms: result.duration_ms,
        executed_on: ExecutorRoute::Hands,
        checksum,
    })))
}

fn operation_for(ticket: &DispatchTicket) -> HandsOperationId {
    // The protocol requires a UUIDv7-shaped identity, while correctness requires every
    // attempt of one durable effect to address the same guest operation. EffectId is the
    // authority here: its first 48 bits occupy the UUID timestamp field and its remaining
    // bits occupy the entropy field. The value is intentionally opaque, not a wall clock.
    let effect = ticket.effect().0;
    let millis = u64::from_be_bytes([
        0, 0, effect[0], effect[1], effect[2], effect[3], effect[4], effect[5],
    ]);
    let mut entropy = [0_u8; 10];
    entropy.copy_from_slice(&effect[6..]);
    HandsOperationId(Uuid7::compose(millis, entropy).to_string())
}

fn encode_detached(
    generation: GenerationId,
    operation: &HandsOperationId,
    max_bytes: usize,
    timeout_ms: u32,
) -> DetachedOperationId {
    DetachedOperationId(format!(
        "{DETACHED_PREFIX}:{generation}:{}:{max_bytes}:{timeout_ms}",
        operation.0
    ))
}

fn decode_detached(operation: &DetachedOperationId) -> Result<DetachedHands, ToolDispatchError> {
    let parts = operation.0.split(':').collect::<Vec<_>>();
    let [prefix, generation, raw_operation, max_bytes, timeout_ms] = parts.as_slice() else {
        return Err(invalid_detached());
    };
    if *prefix != DETACHED_PREFIX || Uuid7::decode_suffix(raw_operation.as_bytes()).is_err() {
        return Err(invalid_detached());
    }
    let generation = GenerationId::parse(generation).map_err(|_| invalid_detached())?;
    let max_bytes = max_bytes.parse::<usize>().map_err(|_| invalid_detached())?;
    let timeout_ms = timeout_ms.parse::<u32>().map_err(|_| invalid_detached())?;
    if max_bytes == 0
        || u64::try_from(max_bytes).unwrap_or(u64::MAX) > MAX_RESULT_BODY_BYTES
        || timeout_ms == 0
    {
        return Err(invalid_detached());
    }
    Ok(DetachedHands {
        generation,
        operation: HandsOperationId((*raw_operation).to_owned()),
        max_bytes,
        timeout_ms,
    })
}

fn invalid_detached() -> ToolDispatchError {
    dispatch_error(
        DispatchStage::PreDispatch,
        DispatchProof::NotSent,
        ProviderFailureKind::InvalidRequest,
        "detached Hands operation reference is malformed",
    )
}

fn hands_error(error: HandsError) -> ToolDispatchError {
    match error {
        HandsError::Transport {
            stage,
            proof,
            detail,
        } => ToolDispatchError {
            stage,
            proof,
            retryable: matches!(
                detail.class(),
                ProviderFailureClass::Transient | ProviderFailureClass::Overloaded
            ),
            detail,
        },
        HandsError::ResultRejected { reason, .. } => ToolDispatchError {
            stage: DispatchStage::Terminal,
            proof: DispatchProof::ResponseStarted,
            retryable: false,
            detail: reason,
        },
        HandsError::GenerationMismatch { .. } | HandsError::OperationMismatch { .. } => {
            dispatch_error(
                DispatchStage::Terminal,
                DispatchProof::ResponseStarted,
                ProviderFailureKind::ProtocolViolation,
                "Hands response did not match the exact generation or operation",
            )
        }
        HandsError::GenerationLost { .. } => ToolDispatchError {
            stage: DispatchStage::PreDispatch,
            proof: DispatchProof::NotSent,
            retryable: false,
            detail: RedactedDetail::internal(
                ProviderFailureKind::ServerError,
                "the exact Hands generation is no longer available",
            ),
        },
        HandsError::CallHashConflict { .. } => dispatch_error(
            DispatchStage::Terminal,
            DispatchProof::ResponseStarted,
            ProviderFailureKind::ProtocolViolation,
            "the Hands operation identity conflicts with another canonical call",
        ),
    }
}

fn dispatch_error(
    stage: DispatchStage,
    proof: DispatchProof,
    kind: ProviderFailureKind,
    message: &str,
) -> ToolDispatchError {
    ToolDispatchError {
        stage,
        proof,
        retryable: matches!(
            kind.class(),
            ProviderFailureClass::Transient | ProviderFailureClass::Overloaded
        ),
        detail: RedactedDetail::internal(kind, message),
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use super::{HandsToolExecutor, decode_detached, operation_for};
    use aex_brain_application::ports::{
        BoxFuture, CancelToken, ControlStateView, DetachedStatus, DispatchTicket, FenceGuard,
        HandsAccepted, HandsEndpoint, HandsError, HandsOperationStart, HandsOperationStatus,
        HandsPort, HandsResult, PreparedToolCall, ResultBounds, ToolOutcome, ToolResultBody,
        ToolRoute,
    };
    use aex_brain_domain::effect::EffectClass;
    use aex_brain_domain::ids::{
        AgentId, AgentKey, AgentRevision, CancelEpoch, ContentHash, DetachedOperationId, EffectId,
        Fence, HandsOperationId, OwnerToken, SessionId, Timestamp, ToolName,
    };
    use aex_brain_domain::journal::ExecutorRoute;
    use aex_brain_tool_catalog::router::ToolExecutor as _;
    use aex_model_catalog::canonical::ToolResultPart;
    use aex_wire::ids::{GenerationId, OrganizationId, PrefixedId as _, Uuid7, WorkspaceId};

    #[derive(Debug)]
    struct FixtureHands {
        starts: Mutex<Vec<(GenerationId, HandsOperationStart)>>,
        statuses: Mutex<Vec<(GenerationId, HandsOperationId)>>,
        cancels: Mutex<Vec<(GenerationId, HandsOperationId, Fence)>>,
        result_checksum: Mutex<Option<ContentHash>>,
        result_truncated: Mutex<bool>,
    }

    impl HandsPort for FixtureHands {
        fn ensure_generation<'a>(
            &'a self,
            _session: &'a SessionId,
            generation: GenerationId,
        ) -> BoxFuture<'a, Result<HandsEndpoint, HandsError>> {
            Box::pin(async move {
                Ok(HandsEndpoint {
                    generation,
                    address: "guest.internal".to_owned(),
                    lease_expires_at: Timestamp::from_millis(2_000_000_000_000),
                })
            })
        }

        fn start<'a>(
            &'a self,
            _ticket: &'a DispatchTicket,
            generation: GenerationId,
            start: &'a HandsOperationStart,
        ) -> BoxFuture<'a, Result<HandsAccepted, HandsError>> {
            Box::pin(async move {
                self.starts
                    .lock()
                    .expect("starts")
                    .push((generation, start.clone()));
                Ok(HandsAccepted {
                    operation: start.operation.clone(),
                    generation,
                    created: true,
                    poll_after: core::time::Duration::from_millis(25),
                })
            })
        }

        fn status<'a>(
            &'a self,
            generation: GenerationId,
            operation: &'a HandsOperationId,
        ) -> BoxFuture<'a, Result<HandsOperationStatus, HandsError>> {
            Box::pin(async move {
                self.statuses
                    .lock()
                    .expect("statuses")
                    .push((generation, operation.clone()));
                Ok(HandsOperationStatus::Completed { exit_code: 0 })
            })
        }

        fn cancel<'a>(
            &'a self,
            generation: GenerationId,
            operation: &'a HandsOperationId,
            fence: Fence,
        ) -> BoxFuture<'a, Result<(), HandsError>> {
            Box::pin(async move {
                self.cancels
                    .lock()
                    .expect("cancels")
                    .push((generation, operation.clone(), fence));
                Ok(())
            })
        }

        fn result<'a>(
            &'a self,
            generation: GenerationId,
            operation: &'a HandsOperationId,
            _bounds: &'a ResultBounds,
        ) -> BoxFuture<'a, Result<HandsResult, HandsError>> {
            Box::pin(async move {
                let inline = "{\"bytes\":3,\"path\":\"/workspace/a.txt\"}".to_owned();
                Ok(HandsResult {
                    operation: operation.clone(),
                    generation,
                    exit_code: 0,
                    inline: Some(inline.clone()),
                    placed: None,
                    truncated: *self.result_truncated.lock().expect("truncated"),
                    duration_ms: 17,
                    checksum: self
                        .result_checksum
                        .lock()
                        .expect("checksum")
                        .unwrap_or_else(|| ContentHash::of(inline.as_bytes())),
                })
            })
        }
    }

    fn generation() -> GenerationId {
        GenerationId::from_uuid7(Uuid7::compose(1, [4; 10]))
    }

    fn fixture() -> (HandsToolExecutor, Arc<FixtureHands>) {
        let hands = Arc::new(FixtureHands {
            starts: Mutex::new(Vec::new()),
            statuses: Mutex::new(Vec::new()),
            cancels: Mutex::new(Vec::new()),
            result_checksum: Mutex::new(None),
            result_truncated: Mutex::new(false),
        });
        (HandsToolExecutor::new(hands.clone()), hands)
    }

    fn ticket_at(effect: EffectId, at: Timestamp) -> DispatchTicket {
        let guard = FenceGuard::new(
            AgentKey::new(
                SessionId(uuid::Uuid::now_v7()),
                AgentId(uuid::Uuid::now_v7()),
            ),
            OwnerToken(uuid::Uuid::new_v4()),
            Fence(7),
            AgentRevision(3),
            None,
            CancelEpoch::ZERO,
            CancelToken::new(),
        );
        DispatchTicket::mint(
            &guard,
            WorkspaceId::from_uuid7(Uuid7::compose(1, [5; 10])),
            OrganizationId::from_uuid7(Uuid7::compose(1, [6; 10])),
            effect,
            1,
            at,
        )
    }

    fn ticket() -> DispatchTicket {
        ticket_at(EffectId([9; 16]), Timestamp::from_millis(1_800_000_000_000))
    }

    fn call() -> PreparedToolCall {
        PreparedToolCall {
            call: aex_brain_domain::ids::ToolCallId::new("call-1").expect("call id"),
            route: ToolRoute {
                name: ToolName::parse("read_file").expect("tool name"),
                executor: ExecutorRoute::Hands,
                class: EffectClass::NonReplayable,
                timeout_ms: 60_000,
                manifest_digest: ContentHash::of(b"manifest"),
            },
            input: aex_wire::CanonicalJson::from_value(&serde_json::json!({
                "path": "/workspace/a.txt"
            }))
            .expect("canonical input"),
            max_result_bytes: 65_536,
            hands_generation: generation(),
            control: ControlStateView::default(),
        }
    }

    #[tokio::test]
    async fn invoke_persists_exact_generation_operation_and_bounds() {
        let (executor, hands) = fixture();
        let outcome = executor
            .invoke(&ticket(), &call(), &CancelToken::new())
            .await
            .expect("accepted");
        let ToolOutcome::Detached {
            operation,
            poll_after,
        } = outcome
        else {
            panic!("Hands is detached");
        };
        assert_eq!(poll_after, core::time::Duration::from_millis(25));
        let detached = decode_detached(&operation).expect("durable reference");
        assert_eq!(detached.generation, generation());
        assert_eq!(detached.max_bytes, 65_536);
        assert_eq!(detached.timeout_ms, 60_000);
        let starts = hands.starts.lock().expect("starts");
        assert_eq!(starts.len(), 1);
        assert_eq!(starts[0].0, generation());
        assert_eq!(starts[0].1.operation, detached.operation);
        let request: aex_hands_protocol::operation::OperationRequest =
            serde_json::from_value(starts[0].1.request.clone()).expect("protocol request");
        assert!(matches!(
            request,
            aex_hands_protocol::operation::OperationRequest::RegisteredTool { name, .. }
                if name.as_str() == "read_file"
        ));
    }

    #[test]
    fn operation_identity_is_stable_across_attempt_timestamps_for_one_effect() {
        let effect = EffectId([7; 16]);
        assert_eq!(
            operation_for(&ticket_at(effect, Timestamp::from_millis(1))),
            operation_for(&ticket_at(effect, Timestamp::from_millis(9_999_999)))
        );
        assert_ne!(
            operation_for(&ticket_at(effect, Timestamp::from_millis(1))),
            operation_for(&ticket_at(EffectId([8; 16]), Timestamp::from_millis(1)))
        );
    }

    #[tokio::test]
    async fn query_uses_persisted_generation_and_canonicalizes_the_verified_result() {
        let (executor, hands) = fixture();
        let ToolOutcome::Detached { operation, .. } = executor
            .invoke(&ticket(), &call(), &CancelToken::new())
            .await
            .expect("accepted")
        else {
            panic!("Hands is detached");
        };
        let DetachedStatus::Completed(result) = executor.query(&operation).await.expect("result")
        else {
            panic!("terminal result");
        };
        let ToolResultBody {
            content,
            duration_ms,
            executed_on,
            checksum,
            ..
        } = *result;
        assert_eq!(duration_ms, 17);
        assert_eq!(executed_on, ExecutorRoute::Hands);
        let [ToolResultPart::Json { value }] = content.as_slice() else {
            panic!("canonical JSON result");
        };
        assert_eq!(value.to_value()["bytes"], 3);
        assert_eq!(checksum, ContentHash::of(value.as_bytes()));
        assert_eq!(hands.statuses.lock().expect("statuses")[0].0, generation());
    }

    #[tokio::test]
    async fn truncated_or_checksum_mismatched_results_never_enter_tool_history() {
        let (executor, hands) = fixture();
        let ToolOutcome::Detached { operation, .. } = executor
            .invoke(&ticket(), &call(), &CancelToken::new())
            .await
            .expect("accepted")
        else {
            panic!("Hands is detached");
        };
        *hands.result_truncated.lock().expect("truncated") = true;
        assert!(executor.query(&operation).await.is_err());

        *hands.result_truncated.lock().expect("truncated") = false;
        *hands.result_checksum.lock().expect("checksum") = Some(ContentHash::of(b"substituted"));
        assert!(executor.query(&operation).await.is_err());
    }

    #[tokio::test]
    async fn cancel_uses_the_persisted_generation_and_malformed_references_send_nothing() {
        let (executor, hands) = fixture();
        let ToolOutcome::Detached { operation, .. } = executor
            .invoke(&ticket(), &call(), &CancelToken::new())
            .await
            .expect("accepted")
        else {
            panic!("Hands is detached");
        };
        executor
            .cancel(&operation, Fence(11))
            .await
            .expect("cancelled");
        assert_eq!(hands.cancels.lock().expect("cancels")[0].0, generation());

        let malformed = DetachedOperationId("hands.v1:wrong".to_owned());
        assert!(executor.query(&malformed).await.is_err());
        assert_eq!(hands.statuses.lock().expect("statuses").len(), 0);
    }

    #[tokio::test]
    async fn pre_cancelled_invocation_never_materializes_or_starts_hands() {
        let (executor, hands) = fixture();
        let cancel = CancelToken::new();
        cancel.cancel();
        let error = executor
            .invoke(&ticket(), &call(), &cancel)
            .await
            .expect_err("cancelled");
        assert_eq!(
            error.proof,
            aex_brain_domain::effect::DispatchProof::NotSent
        );
        assert!(hands.starts.lock().expect("starts").is_empty());
    }
}
