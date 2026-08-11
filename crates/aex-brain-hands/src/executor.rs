//! Tool-router bridge for the exact-generation Hands port.
//!
//! The global router retains no tenant state. Each prepared call carries the immutable
//! session generation, and the detached operation reference persists that generation plus
//! the result bounds needed after a process restart. A query therefore never discovers a
//! current generation or silently follows a successor with a different filesystem.

use std::sync::Arc;

use aex_brain_app::ports::{
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
use aex_model_catalog::BoundedString;
use aex_model_catalog::canonical::ToolResultPart;
use aex_wire::CanonicalJson;
use aex_wire::ids::{GenerationId, PrefixedId as _, ResourceName, Uuid7};

use crate::encode::{EncodedOperation, HandsTool, ToolEncodingError};
use crate::{CONTEXT_TOOL_RESULT_BYTES, MAX_RESULT_BODY_BYTES};

const DETACHED_PREFIX: &str = "hands.v1";

/// Adapts the exact-generation Hands port to the coarse tool-router executor seam.
///
/// A call whose name is a Hands-routed catalogue row becomes the **guest operation
/// that does the thing** — `ls` becomes `ListDir`, a command becomes `Exec` — via
/// [`crate::encode::HandsTool`], which owns the whole map and is exhaustive over it.
/// Anything outside that closed set is still encoded as a protocol `registered_tool`
/// operation, which the guest refuses; [`ToolExecutor::supports`] therefore reports
/// it as unimplemented so readiness never advertises it.
///
/// One reinterpretation happens and is deliberate: `bash`'s `command` is a
/// shell line by its own catalogue schema, so it is run by a shell
/// ([`crate::encode::COMMAND_SHELL`]). Its `argv` form is passed through untouched.
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
            .map(|body| DetachedStatus::Completed(Box::new(body)))
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
    fn supports(&self, tool: &aex_brain_domain::ids::ToolName) -> bool {
        // Exactly the rows this bridge encodes into an operation the guest runs.
        // A row the guest cannot complete stays unsupported and is therefore
        // never advertised: offering a tool that can only answer
        // capability_unavailable is the silent failure the owner rules out.
        HandsTool::parse(tool.as_str()).is_some_and(HandsTool::is_served)
    }

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
            let encoded = encode_call(call)?;
            // A wall bound the arguments named may only narrow the route's own.
            let timeout_ms = encoded
                .max_wall_ms
                .and_then(|wall| u32::try_from(wall).ok())
                .map_or(call.route.timeout_ms, |wall| {
                    call.route.timeout_ms.min(wall)
                });
            if timeout_ms == 0 {
                return Err(dispatch_error(
                    DispatchStage::PreDispatch,
                    DispatchProof::NotSent,
                    ProviderFailureKind::InvalidRequest,
                    "Hands call has a zero result or wall-clock bound",
                ));
            }
            let request_value = serde_json::to_value(&encoded.request).map_err(|_| {
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
                timeout_ms,
            };
            let start = HandsOperationStart {
                operation: operation.clone(),
                call_hash: ContentHash::of(request_canonical.as_bytes()),
                request: request_value,
                bounds,
                deadline: ticket.issued_at().plus_millis(i64::from(timeout_ms)),
            };
            let accepted = self
                .hands
                .start(ticket, call.hands_generation, &start)
                .await
                .map_err(hands_error)?;
            // Attached delivery: the guest answered on the connection the start
            // held, so the result is already here and there is no detached
            // operation to persist, no poll interval to wait out and no second
            // round trip to make. The incorporation is the same one the pull
            // performs — same verified checksum, same bound, same body.
            if let Some(result) = accepted.result {
                return incorporate_result(*result, max_bytes).map(ToolOutcome::Completed);
            }
            let durable = encode_detached(
                accepted.generation,
                &accepted.operation,
                max_bytes,
                timeout_ms,
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

/// Turns one prepared model call into the operation the guest will actually run.
///
/// A Hands-routed catalogue row becomes its structured operation. A name outside
/// that closed set keeps the previous behaviour — a `registered_tool` operation the
/// guest refuses — which is reachable only for a custom registered executor, since
/// [`ToolExecutor::supports`] keeps every unmapped built-in off the advertised
/// surface.
fn encode_call(call: &PreparedToolCall) -> Result<EncodedOperation, ToolDispatchError> {
    let Some(tool) = HandsTool::parse(call.route.name.as_str()) else {
        return Ok(EncodedOperation::instant(
            OperationRequest::RegisteredTool {
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
            },
        ));
    };
    tool.encode(
        &aex_runtime_control::generation::guest_root(),
        &call.input.to_value(),
    )
    .map_err(|error| encoding_error(&error))
}

/// Reports an encoding refusal as a pre-dispatch failure, with its own reason.
///
/// Nothing was sent, so the model can correct the call and try again; the reason
/// names the exact argument rather than a generic rejection.
fn encoding_error(error: &ToolEncodingError) -> ToolDispatchError {
    dispatch_error(
        DispatchStage::PreDispatch,
        DispatchProof::NotSent,
        ProviderFailureKind::InvalidRequest,
        &error.to_string(),
    )
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
) -> Result<ToolResultBody, ToolDispatchError> {
    if result.truncated {
        return Err(dispatch_error(
            DispatchStage::Terminal,
            DispatchProof::ResponseStarted,
            ProviderFailureKind::ProtocolViolation,
            "Hands result was truncated and cannot be journalled as complete",
        ));
    }
    let (part, bytes) = match (result.inline, result.placed) {
        (Some(inline), None) => {
            if ContentHash::of(inline.as_bytes()) != result.checksum {
                return Err(dispatch_error(
                    DispatchStage::Terminal,
                    DispatchProof::ResponseStarted,
                    ProviderFailureKind::ProtocolViolation,
                    "Hands result disagrees with its verified body checksum",
                ));
            }
            // The structured guest operations answer in text: a listing is
            // lines, a stat is one row, an exec is its captured output. A
            // registered tool answers in JSON. Both are carried verbatim,
            // because reshaping a body to fit the other kind would put words in
            // the guest's mouth.
            match serde_json::from_str::<serde_json::Value>(&inline) {
                Ok(value) => json_part(&value)?,
                Err(_) => text_part(inline)?,
            }
        }
        (None, Some(placed)) => {
            let value = serde_json::to_value(placed).map_err(|_| {
                dispatch_error(
                    DispatchStage::Terminal,
                    DispatchProof::ResponseStarted,
                    ProviderFailureKind::ProtocolViolation,
                    "Hands placed result reference could not be encoded",
                )
            })?;
            json_part(&value)?
        }
        _ => {
            return Err(dispatch_error(
                DispatchStage::Terminal,
                DispatchProof::ResponseStarted,
                ProviderFailureKind::ProtocolViolation,
                "Hands result must carry exactly one inline or placed body",
            ));
        }
    };
    if bytes.len() > max_bytes {
        return Err(dispatch_error(
            DispatchStage::Terminal,
            DispatchProof::ResponseStarted,
            ProviderFailureKind::ProtocolViolation,
            "Hands result exceeds its persisted result bound",
        ));
    }
    let checksum = ContentHash::of(&bytes);
    Ok(ToolResultBody {
        content: vec![part],
        is_error: result.exit_code != 0,
        duration_ms: result.duration_ms,
        executed_on: ExecutorRoute::Hands,
        checksum,
    })
}

/// A canonical-JSON result part and the bytes its checksum is over.
fn json_part(value: &serde_json::Value) -> Result<(ToolResultPart, Vec<u8>), ToolDispatchError> {
    let canonical = CanonicalJson::from_value(value).map_err(|_| {
        dispatch_error(
            DispatchStage::Terminal,
            DispatchProof::ResponseStarted,
            ProviderFailureKind::ProtocolViolation,
            "Hands result could not be canonicalized",
        )
    })?;
    let bytes = canonical.as_bytes().to_vec();
    Ok((ToolResultPart::Json { value: canonical }, bytes))
}

/// A text result part and the bytes its checksum is over.
fn text_part(text: String) -> Result<(ToolResultPart, Vec<u8>), ToolDispatchError> {
    let bytes = text.as_bytes().to_vec();
    let text = BoundedString::new(text).map_err(|_| {
        dispatch_error(
            DispatchStage::Terminal,
            DispatchProof::ResponseStarted,
            ProviderFailureKind::ProtocolViolation,
            "Hands result exceeds the provider-neutral tool-result text bound",
        )
    })?;
    Ok((ToolResultPart::Text { text }, bytes))
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

    mod attached;

    use super::{HandsToolExecutor, decode_detached, operation_for};
    use aex_brain_app::ports::{
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
        result_body: Mutex<String>,
        /// What an attached start answers with on the connection it held.
        attached: Mutex<Option<HandsResult>>,
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
                    result: self
                        .attached
                        .lock()
                        .expect("attached")
                        .clone()
                        .map(Box::new),
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
                let inline = self.result_body.lock().expect("body").clone();
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
            result_body: Mutex::new("{\"bytes\":3,\"path\":\"/workspace/a.txt\"}".to_owned()),
            attached: Mutex::new(None),
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
                concurrency_weight: 1,
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
        // This assertion used to pin the defect: it required `read_file` to
        // arrive as a `RegisteredTool`, which is the one operation the guest
        // refuses. The read now arrives as the read.
        let request: aex_hands_protocol::operation::OperationRequest =
            serde_json::from_value(starts[0].1.request.clone()).expect("protocol request");
        let aex_hands_protocol::operation::OperationRequest::ReadFile { path, range } = request
        else {
            panic!("a file read is a ReadFile operation");
        };
        assert_eq!(path.as_str(), "/workspace/a.txt");
        assert_eq!(range, None);
    }

    #[tokio::test]
    async fn the_owners_ls_reaches_the_guests_list_dir_operation() {
        let (executor, hands) = fixture();
        let mut listing = call();
        listing.route.name = ToolName::parse("list_dir").expect("tool name");
        listing.input =
            aex_wire::CanonicalJson::from_value(&serde_json::json!({})).expect("canonical input");
        executor
            .invoke(&ticket(), &listing, &CancelToken::new())
            .await
            .expect("accepted");
        let starts = hands.starts.lock().expect("starts");
        let request: aex_hands_protocol::operation::OperationRequest =
            serde_json::from_value(starts[0].1.request.clone()).expect("protocol request");
        let aex_hands_protocol::operation::OperationRequest::ListDir { path, .. } = request else {
            panic!("`ls` is a ListDir operation");
        };
        assert_eq!(path.as_str(), "/workspace");
    }

    #[tokio::test]
    async fn a_command_reaches_the_one_guest_primitive_that_starts_a_process() {
        let (executor, hands) = fixture();
        let mut command = call();
        command.route.name = ToolName::parse("bash").expect("tool name");
        command.route.timeout_ms = 600_000;
        command.input =
            aex_wire::CanonicalJson::from_value(&serde_json::json!({"command": "ls -la"}))
                .expect("canonical input");
        let ToolOutcome::Detached { operation, .. } = executor
            .invoke(&ticket(), &command, &CancelToken::new())
            .await
            .expect("accepted")
        else {
            panic!("Hands is detached");
        };
        let starts = hands.starts.lock().expect("starts");
        let request: aex_hands_protocol::operation::OperationRequest =
            serde_json::from_value(starts[0].1.request.clone()).expect("protocol request");
        let aex_hands_protocol::operation::OperationRequest::Exec { argv, cwd, .. } = request
        else {
            panic!("a command is an Exec operation");
        };
        assert_eq!(argv, vec!["bash", "-lc", "ls -la"]);
        assert_eq!(cwd.as_str(), "/workspace");
        assert_eq!(
            decode_detached(&operation)
                .expect("durable reference")
                .timeout_ms,
            600_000
        );
    }

    #[tokio::test]
    async fn a_named_timeout_narrows_the_persisted_bound_and_is_never_widened_by_it() {
        let (executor, hands) = fixture();
        let mut command = call();
        command.route.name = ToolName::parse("bash").expect("tool name");
        command.route.timeout_ms = 600_000;
        command.input = aex_wire::CanonicalJson::from_value(
            &serde_json::json!({"command": "sleep 1", "timeoutMs": 5_000}),
        )
        .expect("canonical input");
        let ToolOutcome::Detached { operation, .. } = executor
            .invoke(&ticket(), &command, &CancelToken::new())
            .await
            .expect("accepted")
        else {
            panic!("Hands is detached");
        };
        assert_eq!(
            decode_detached(&operation)
                .expect("durable reference")
                .timeout_ms,
            5_000,
            "a caller that asked for five seconds must not silently get ten minutes"
        );
        let starts = hands.starts.lock().expect("starts");
        assert_eq!(starts[0].1.bounds.timeout_ms, 5_000);
    }

    #[tokio::test]
    async fn an_argument_the_wire_cannot_carry_sends_nothing_and_says_why() {
        let (executor, hands) = fixture();
        let mut read = call();
        read.input = aex_wire::CanonicalJson::from_value(
            &serde_json::json!({"path": "/workspace/a.txt", "startLine": 4}),
        )
        .expect("canonical input");
        let error = executor
            .invoke(&ticket(), &read, &CancelToken::new())
            .await
            .expect_err("an unrepresentable argument is refused");
        assert_eq!(
            error.proof,
            aex_brain_domain::effect::DispatchProof::NotSent
        );
        assert!(hands.starts.lock().expect("starts").is_empty());
    }

    #[test]
    fn only_the_rows_the_guest_can_complete_are_reported_as_supported() {
        let (executor, _) = fixture();
        for served in ["read_file", "list_dir", "glob", "grep", "bash", "git"] {
            assert!(
                executor.supports(&ToolName::parse(served).expect("tool name")),
                "{served} is encoded into an operation the guest runs"
            );
        }
        for unserved in ["write_file", "run_code", "browser_launch", "web_fetch"] {
            assert!(
                !executor.supports(&ToolName::parse(unserved).expect("tool name")),
                "{unserved} must never be advertised: it could only fail"
            );
        }
    }

    #[tokio::test]
    async fn a_text_body_from_a_structured_operation_reaches_the_model_intact() {
        let (executor, hands) = fixture();
        let listing = "dir 4096 src/\nfile 12 a.txt";
        *hands.result_body.lock().expect("body") = listing.to_owned();
        let mut call = call();
        call.route.name = ToolName::parse("list_dir").expect("tool name");
        call.input =
            aex_wire::CanonicalJson::from_value(&serde_json::json!({})).expect("canonical input");
        let ToolOutcome::Detached { operation, .. } = executor
            .invoke(&ticket(), &call, &CancelToken::new())
            .await
            .expect("accepted")
        else {
            panic!("Hands is detached");
        };
        let DetachedStatus::Completed(result) = executor.query(&operation).await.expect("result")
        else {
            panic!("terminal result");
        };
        let [ToolResultPart::Text { text }] = result.content.as_slice() else {
            panic!("a guest listing is text, not JSON");
        };
        assert_eq!(text.as_str(), listing);
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
        {
            let cancels = hands.cancels.lock().expect("cancels");
            assert_eq!(cancels[0].0, generation());
            assert_eq!(cancels[0].2, Fence(11));
        }

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
