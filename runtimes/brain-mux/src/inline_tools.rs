//! Typed, process-local Brain-control tools.
//!
//! This executor is a closed set. It never opens a socket, reads a filesystem,
//! or discovers state. Reads come from the exact folded control view carried by
//! [`PreparedToolCall`]; writes become durable only when the activation settles
//! the matching result.

use aex_brain_application::ports::{
    BoxFuture, CancelToken, DetachedStatus, DispatchTicket, PreparedToolCall, ProviderFailureKind,
    RedactedDetail, ToolDispatchError, ToolOutcome, ToolResultBody,
};
use aex_brain_domain::effect::{DispatchProof, DispatchStage};
use aex_brain_domain::ids::{ContentHash, DetachedOperationId, Fence, ToolName};
use aex_brain_domain::journal::ExecutorRoute;
use aex_brain_tool_catalog::control::{apply_todo_write, render_todo_read};
use aex_brain_tool_catalog::router::ToolExecutor;
use aex_model_catalog::canonical::ToolResultPart;
use aex_wire::CanonicalJson;
use std::time::Instant;

/// Production executor for the currently earned pure control tools.
#[derive(Debug, Clone, Copy, Default)]
pub struct BrainControlExecutor;

impl ToolExecutor for BrainControlExecutor {
    fn supports(&self, tool: &ToolName) -> bool {
        matches!(tool.as_str(), "todo_read" | "todo_write")
    }

    fn invoke<'a>(
        &'a self,
        _ticket: &'a DispatchTicket,
        call: &'a PreparedToolCall,
        cancel: &'a CancelToken,
    ) -> BoxFuture<'a, Result<ToolOutcome, ToolDispatchError>> {
        Box::pin(async move {
            if cancel.is_cancelled() {
                return Err(refusal(
                    ProviderFailureKind::Cancelled,
                    "Brain-control call was cancelled before execution",
                ));
            }
            if call.route.executor != ExecutorRoute::BrainInline {
                return Err(refusal(
                    ProviderFailureKind::InvalidRequest,
                    "Brain-control executor received a call pinned to another route",
                ));
            }

            let started = Instant::now();
            let result = match call.route.name.as_str() {
                "todo_read" => {
                    if call
                        .input
                        .to_value()
                        .as_object()
                        .is_none_or(|object| !object.is_empty())
                    {
                        return Err(refusal(
                            ProviderFailureKind::InvalidRequest,
                            "todo_read requires an empty argument object",
                        ));
                    }
                    serde_json::to_value(
                        render_todo_read(call.control.todo_state.as_ref()).map_err(|_| {
                            refusal(
                                ProviderFailureKind::ProtocolViolation,
                                "folded todo state is not a valid todo document",
                            )
                        })?,
                    )
                    .map_err(|_| {
                        refusal(
                            ProviderFailureKind::ProtocolViolation,
                            "todo_read result could not be encoded",
                        )
                    })?
                }
                "todo_write" => {
                    let (_, result) = apply_todo_write(&call.input).map_err(|_| {
                        refusal(
                            ProviderFailureKind::InvalidRequest,
                            "todo_write arguments violate the pinned control contract",
                        )
                    })?;
                    serde_json::to_value(result).map_err(|_| {
                        refusal(
                            ProviderFailureKind::ProtocolViolation,
                            "todo_write result could not be encoded",
                        )
                    })?
                }
                _ => {
                    return Err(refusal(
                        ProviderFailureKind::InvalidRequest,
                        "Brain-control executor received a tool outside its closed set",
                    ));
                }
            };
            let canonical = CanonicalJson::from_value(&result).map_err(|_| {
                refusal(
                    ProviderFailureKind::ProtocolViolation,
                    "Brain-control result could not be canonicalized",
                )
            })?;
            if canonical.as_bytes().len() > call.max_result_bytes {
                return Err(refusal(
                    ProviderFailureKind::InvalidRequest,
                    "Brain-control result exceeds the caller's exact result bound",
                ));
            }
            let checksum = ContentHash::of(canonical.as_bytes());
            Ok(ToolOutcome::Completed(ToolResultBody {
                content: vec![ToolResultPart::Json { value: canonical }],
                is_error: false,
                duration_ms: u32::try_from(started.elapsed().as_millis()).unwrap_or(u32::MAX),
                executed_on: ExecutorRoute::BrainInline,
                checksum,
            }))
        })
    }

    fn query<'a>(
        &'a self,
        _operation: &'a DetachedOperationId,
    ) -> BoxFuture<'a, Result<DetachedStatus, ToolDispatchError>> {
        Box::pin(async {
            Err(refusal(
                ProviderFailureKind::InvalidRequest,
                "pure Brain-control tools never create detached operations",
            ))
        })
    }

    fn cancel<'a>(
        &'a self,
        _operation: &'a DetachedOperationId,
        _fence: Fence,
    ) -> BoxFuture<'a, Result<(), ToolDispatchError>> {
        Box::pin(async {
            Err(refusal(
                ProviderFailureKind::InvalidRequest,
                "pure Brain-control tools have no detached operation to cancel",
            ))
        })
    }
}

fn refusal(kind: ProviderFailureKind, message: &'static str) -> ToolDispatchError {
    ToolDispatchError {
        stage: DispatchStage::PreDispatch,
        proof: DispatchProof::NotSent,
        retryable: false,
        detail: RedactedDetail::internal(kind, message),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aex_brain_application::ports::{ControlStateView, FenceGuard, ToolRoute};
    use aex_brain_domain::effect::EffectClass;
    use aex_brain_domain::ids::{
        AgentId, AgentKey, AgentRevision, CancelEpoch, EffectId, OwnerToken, SessionId, Timestamp,
        ToolCallId,
    };
    use aex_wire::ids::{GenerationId, OrganizationId, PrefixedId as _, Uuid7, WorkspaceId};
    use uuid::Uuid;

    fn block_on<F: core::future::Future>(future: F) -> F::Output {
        let mut future = Box::pin(future);
        let mut context = core::task::Context::from_waker(core::task::Waker::noop());
        match future.as_mut().poll(&mut context) {
            core::task::Poll::Ready(value) => value,
            core::task::Poll::Pending => panic!("inline executor must complete synchronously"),
        }
    }

    fn ticket() -> DispatchTicket {
        let guard = FenceGuard::new(
            AgentKey::new(SessionId(Uuid::from_u128(1)), AgentId(Uuid::from_u128(2))),
            OwnerToken(Uuid::from_u128(3)),
            Fence(4),
            AgentRevision(5),
            None,
            CancelEpoch::ZERO,
            CancelToken::new(),
        );
        DispatchTicket::mint(
            &guard,
            WorkspaceId::from_uuid7(Uuid7::compose(1, [6; 10])),
            OrganizationId::from_uuid7(Uuid7::compose(1, [7; 10])),
            EffectId([8; 16]),
            1,
            Timestamp::from_millis(4),
        )
    }

    fn call(
        name: &str,
        input: CanonicalJson,
        todo_state: Option<CanonicalJson>,
    ) -> PreparedToolCall {
        PreparedToolCall {
            call: ToolCallId::new("call-1").expect("call id"),
            route: ToolRoute {
                name: ToolName::parse(name).expect("tool name"),
                executor: ExecutorRoute::BrainInline,
                class: EffectClass::Pure,
                timeout_ms: 50,
                manifest_digest: ContentHash::of(b"tool catalog"),
            },
            input,
            max_result_bytes: 65_536,
            hands_generation: GenerationId::from_uuid7(Uuid7::compose(1, [9; 10])),
            control: ControlStateView {
                todo_state,
                assistant_turns: 0,
                depth: 0,
            },
        }
    }

    #[test]
    fn capability_is_closed_to_todo_tools() {
        let executor = BrainControlExecutor;
        assert!(executor.supports(&ToolName::parse("todo_read").expect("name")));
        assert!(executor.supports(&ToolName::parse("todo_write").expect("name")));
        for forbidden in ["wait", "grep", "run_command", "web_fetch"] {
            assert!(!executor.supports(&ToolName::parse(forbidden).expect("name")));
        }
    }

    #[test]
    fn read_is_a_pure_projection_of_the_folded_successful_write() {
        let state = CanonicalJson::parse(
            r#"{"todos":[{"activeForm":"Testing","content":"Test it","status":"in_progress"}]}"#,
        )
        .expect("todo state");
        let read = call(
            "todo_read",
            CanonicalJson::parse("{}").expect("empty input"),
            Some(state),
        );
        let outcome = block_on(BrainControlExecutor.invoke(&ticket(), &read, &CancelToken::new()))
            .expect("read result");
        let ToolOutcome::Completed(result) = outcome else {
            panic!("todo_read is inline")
        };
        let [ToolResultPart::Json { value }] = result.content.as_slice() else {
            panic!("one canonical JSON result")
        };
        assert_eq!(
            value.to_value()["counts"]["in_progress"],
            serde_json::json!(1)
        );
    }

    #[test]
    fn shell_names_are_refused_before_any_dispatch() {
        let shell = call(
            "grep",
            CanonicalJson::parse(r#"{"pattern":"x"}"#).expect("input"),
            None,
        );
        let error = block_on(BrainControlExecutor.invoke(&ticket(), &shell, &CancelToken::new()))
            .expect_err("shell must remain in Hands");
        assert_eq!(error.stage, DispatchStage::PreDispatch);
        assert_eq!(error.proof, DispatchProof::NotSent);
    }
}
