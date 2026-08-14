//! Production exact-generation runtime waiter and Hands guest adapters.

use std::sync::Arc;

use aex_brain_app::ports::{
    CancelToken, DispatchTicket, FenceGuard, HandsOperationStart, HandsOperationStatus, HandsPort,
    HandsResult, ResultBounds,
};
use aex_brain_domain::ids::{
    AgentId as BrainAgentId, AgentKey, AgentRevision, CancelEpoch, ContentHash as BrainHash,
    EffectId, Fence as BrainFence, HandsOperationId as BrainOperationId, OwnerToken,
    SessionId as BrainSessionId, Timestamp as BrainTimestamp,
};
use aex_hands_protocol::operation::{
    EnvName, EnvValue, GuestPath, GuestRoot, OperationRequest, SANDBOX_MCP_REQUEST_VAR,
    SandboxMcpCall,
};
use aex_hands_protocol::rpc::HandsOperationId;
use aex_runtime_control::HandId;
use aex_tool_mux::{
    ExecutorOutput, FullOutput, GuestPort, ReadyHand, RuntimePort, SandboxConfig, ToolCallIdentity,
    ToolMuxFuture, ToolTarget,
};
use aex_wire::CanonicalJson;
use aex_wire::ids::{ContentHash, GenerationId, PrefixedId as _, SessionId, Uuid7};
use sha2::{Digest as _, Sha256};

use crate::mcp::McpSecretReader;

const GUEST_HELPER: &str = "/proc/self/exe";

/// Runtime Control port over the one production Hands lifecycle authority.
pub struct ProductionRuntimeAdapter {
    live: Arc<dyn aex_brain_hands::LiveFileBackend>,
}

impl ProductionRuntimeAdapter {
    /// Binds exact-generation lifecycle operations.
    #[must_use]
    pub const fn new(live: Arc<dyn aex_brain_hands::LiveFileBackend>) -> Self {
        Self { live }
    }
}

impl RuntimePort for ProductionRuntimeAdapter {
    fn eager_prepare<'a>(
        &'a self,
        session: SessionId,
        sandbox: SandboxConfig,
    ) -> ToolMuxFuture<'a, Result<Vec<aex_tool_mux::PreparationProgress>, String>> {
        Box::pin(async move {
            let generation = sandbox
                .generation
                .ok_or_else(|| "enabled sandbox has no exact generation".to_owned())?;
            self.live
                .ensure_ready(session, generation)
                .await
                .map_err(|_| "sandbox eager readiness failed".to_owned())?;
            self.live
                .suspend_ready(session, generation)
                .await
                .map_err(|_| "sandbox eager suspension failed".to_owned())?;
            Ok(vec![
                aex_tool_mux::PreparationProgress::Ready,
                aex_tool_mux::PreparationProgress::Suspended,
            ])
        })
    }

    fn wait_ready<'a>(
        &'a self,
        session: SessionId,
        hand: HandId,
        generation: GenerationId,
        call: &'a ToolCallIdentity,
    ) -> ToolMuxFuture<'a, Result<ReadyHand, String>> {
        Box::pin(async move {
            if hand != HandId::for_session(session) || call.session != session {
                return Err("tool waiter identity does not own the Hand".to_owned());
            }
            let operation = operation_id(call);
            let fence = self
                .live
                .hold_tool_waiter(session, generation, operation)
                .await
                .map_err(|_| "exact-generation tool waiter admission failed".to_owned())?;
            Ok(ReadyHand {
                hand,
                generation,
                fence,
            })
        })
    }

    fn settle_waiter<'a>(
        &'a self,
        ready: ReadyHand,
        call: &'a ToolCallIdentity,
    ) -> ToolMuxFuture<'a, Result<(), String>> {
        Box::pin(async move {
            if ready.hand != HandId::for_session(call.session) {
                return Err("tool waiter settlement names a foreign Hand".to_owned());
            }
            self.live
                .settle_tool_waiter(ready.generation, operation_id(call))
                .await
                .map_err(|_| "exact-generation tool waiter settlement failed".to_owned())
        })
    }
}

/// Credential-free exact-generation guest operations.
pub struct ProductionGuestAdapter {
    hands: Arc<dyn HandsPort>,
    secrets: Arc<dyn McpSecretReader>,
}

impl ProductionGuestAdapter {
    /// Binds Hands and MCP custody.
    #[must_use]
    pub const fn new(hands: Arc<dyn HandsPort>, secrets: Arc<dyn McpSecretReader>) -> Self {
        Self { hands, secrets }
    }

    async fn operation_request(
        &self,
        target: &ToolTarget,
        arguments: &serde_json::Value,
        call: &ToolCallIdentity,
    ) -> Result<OperationRequest, String> {
        match target {
            ToolTarget::OfficialSandbox { tool } => {
                let name = match tool {
                    aex_tool_mux::OfficialSandboxTool::Read => "read_file",
                    aex_tool_mux::OfficialSandboxTool::Edit => "edit_file",
                    aex_tool_mux::OfficialSandboxTool::Write => "write_file",
                    aex_tool_mux::OfficialSandboxTool::Bash => "bash",
                };
                aex_brain_hands::HandsTool::parse(name)
                    .expect("official tool is closed")
                    .encode(&GuestRoot::workspace(), arguments)
                    .map(|encoded| encoded.request)
                    .map_err(|error| error.to_string())
            }
            ToolTarget::SandboxMcp {
                command,
                args,
                environment,
                working_directory,
                tool,
                ..
            } => {
                let mut revealed = std::collections::BTreeMap::new();
                for (name, secret) in environment {
                    let value = self.secrets.reveal(secret, call).await?;
                    revealed.insert(name.clone(), EnvValue::new(value.to_string()));
                }
                let cwd = GuestPath::parse(
                    &GuestRoot::workspace(),
                    working_directory.as_deref().unwrap_or("/workspace"),
                )
                .map_err(|_| "sandbox MCP working directory is invalid".to_owned())?;
                let request = SandboxMcpCall {
                    command: command.clone(),
                    args: args.clone(),
                    environment: revealed,
                    working_directory: cwd.clone(),
                    tool: tool.clone(),
                    arguments: CanonicalJson::from_value(arguments)
                        .map_err(|_| "sandbox MCP arguments are not canonical".to_owned())?,
                };
                Ok(OperationRequest::Exec {
                    argv: vec![GUEST_HELPER.to_owned(), "mcp-call".to_owned()],
                    cwd,
                    env: vec![(
                        EnvName(SANDBOX_MCP_REQUEST_VAR.to_owned()),
                        EnvValue::new(
                            serde_json::to_string(&request)
                                .map_err(|_| "sandbox MCP request is not encodable".to_owned())?,
                        ),
                    )],
                    stdin: None,
                })
            }
            _ => Err("guest adapter received a non-guest target".to_owned()),
        }
    }
}

impl GuestPort for ProductionGuestAdapter {
    fn hello<'a>(&'a self, ready: ReadyHand) -> ToolMuxFuture<'a, Result<(), String>> {
        Box::pin(async move {
            let endpoint = self
                .hands
                .ensure_generation(&brain_session(ready.hand.session()), ready.generation)
                .await
                .map_err(|_| "exact-generation guest hello failed".to_owned())?;
            if endpoint.generation != ready.generation {
                return Err("guest hello returned a foreign generation".to_owned());
            }
            Ok(())
        })
    }

    fn start<'a>(
        &'a self,
        ready: ReadyHand,
        target: &'a ToolTarget,
        arguments: &'a serde_json::Value,
        call: &'a ToolCallIdentity,
    ) -> ToolMuxFuture<'a, Result<Result<ExecutorOutput, HandsOperationId>, String>> {
        Box::pin(async move {
            let request = self.operation_request(target, arguments, call).await?;
            let value = serde_json::to_value(&request)
                .map_err(|_| "guest operation is not encodable".to_owned())?;
            let canonical = CanonicalJson::from_value(&value)
                .map_err(|_| "guest operation is not canonical".to_owned())?;
            let operation = operation_id(call);
            let accepted = self
                .hands
                .start(
                    &dispatch_ticket(call, ready.fence.0)?,
                    ready.generation,
                    &HandsOperationStart {
                        operation: BrainOperationId(operation.0.to_string()),
                        call_hash: BrainHash::of(canonical.as_bytes()),
                        request: value,
                        bounds: ResultBounds {
                            max_bytes: max_result_bytes(call),
                            max_stream_bytes: aex_tool_mux::MAX_LIVE_PREVIEW_BYTES,
                            timeout_ms: timeout_ms(call),
                        },
                        deadline: BrainTimestamp::from_millis(deadline_ms(call)),
                    },
                )
                .await
                .map_err(|_| "exact-generation guest dispatch failed".to_owned())?;
            match accepted.result {
                Some(result) => Ok(Ok(output_from_result(*result)?)),
                None => Ok(Err(operation)),
            }
        })
    }

    fn read<'a>(
        &'a self,
        ready: ReadyHand,
        operation: HandsOperationId,
        max_result_bytes: usize,
        timeout_ms: u32,
    ) -> ToolMuxFuture<'a, Result<Option<ExecutorOutput>, String>> {
        Box::pin(async move {
            let brain = BrainOperationId(operation.0.to_string());
            match self
                .hands
                .status(ready.generation, &brain)
                .await
                .map_err(|_| "guest result status failed".to_owned())?
            {
                HandsOperationStatus::Running { .. } => Ok(None),
                HandsOperationStatus::Completed { .. } => self
                    .hands
                    .result(
                        ready.generation,
                        &brain,
                        &ResultBounds {
                            max_bytes: max_result_bytes,
                            max_stream_bytes: max_result_bytes
                                .min(aex_tool_mux::MAX_LIVE_PREVIEW_BYTES),
                            timeout_ms,
                        },
                    )
                    .await
                    .map_err(|_| "guest result pull failed".to_owned())
                    .and_then(output_from_result)
                    .map(Some),
                HandsOperationStatus::Failed { reason } => Ok(Some(error_output(reason.as_str()))),
                HandsOperationStatus::Cancelled => Ok(Some(error_output("operation cancelled"))),
                HandsOperationStatus::GenerationLost => {
                    Err("exact sandbox generation was lost".to_owned())
                }
            }
        })
    }

    fn cancel<'a>(
        &'a self,
        ready: ReadyHand,
        operation: HandsOperationId,
    ) -> ToolMuxFuture<'a, Result<(), String>> {
        Box::pin(async move {
            self.hands
                .cancel(
                    ready.generation,
                    &BrainOperationId(operation.0.to_string()),
                    BrainFence(ready.fence.0),
                )
                .await
                .map_err(|_| "guest cancellation failed".to_owned())
        })
    }
}

fn output_from_result(result: HandsResult) -> Result<ExecutorOutput, String> {
    if result.truncated {
        return Err("guest result exceeded its declared complete-result bound".to_owned());
    }
    match (result.inline, result.placed, result.sandbox_file) {
        (Some(body), None, None) => Ok(ExecutorOutput {
            preview: body.as_bytes()[..body.len().min(aex_tool_mux::MAX_LIVE_PREVIEW_BYTES)]
                .to_vec(),
            full: FullOutput::Inline(body.into_bytes()),
            is_error: result.exit_code != 0,
        }),
        (None, None, Some(file)) => Ok(ExecutorOutput {
            preview: file.preview.into_bytes(),
            full: FullOutput::SandboxFile {
                path: GuestPath::parse(&GuestRoot::workspace(), &file.path)
                    .map_err(|_| "guest large-result path is invalid".to_owned())?,
                bytes: file.byte_len,
                hash: ContentHash::from_bytes(result.checksum.0),
            },
            is_error: result.exit_code != 0,
        }),
        (None, Some(placed), None) => {
            let body = serde_json::to_vec(&placed)
                .map_err(|_| "guest placed result is not encodable".to_owned())?;
            Ok(ExecutorOutput {
                preview: body.clone(),
                full: FullOutput::Inline(body),
                is_error: result.exit_code != 0,
            })
        }
        _ => Err("guest result body is incoherent".to_owned()),
    }
}

fn error_output(message: &str) -> ExecutorOutput {
    let body = serde_json::to_vec(&serde_json::json!({"error": message}))
        .unwrap_or_else(|_| b"{\"error\":\"guest failure\"}".to_vec());
    ExecutorOutput {
        preview: body.clone(),
        full: FullOutput::Inline(body),
        is_error: true,
    }
}

fn operation_id(call: &ToolCallIdentity) -> HandsOperationId {
    let digest = identity_digest(call);
    let mut entropy = [0_u8; 10];
    entropy.copy_from_slice(&digest[..10]);
    HandsOperationId(Uuid7::compose(call.session.uuid7().unix_millis(), entropy))
}

fn dispatch_ticket(call: &ToolCallIdentity, fence: u64) -> Result<DispatchTicket, String> {
    let digest = identity_digest(call);
    let mut effect = [0_u8; 16];
    effect.copy_from_slice(&digest[..16]);
    let owner = uuid::Uuid::from_bytes(effect);
    let guard = FenceGuard::new(
        AgentKey::new(
            brain_session(call.session),
            BrainAgentId(uuid::Uuid::from_bytes(*call.agent.uuid7().as_bytes())),
        ),
        OwnerToken(owner),
        BrainFence(fence),
        AgentRevision(0),
        None,
        CancelEpoch::ZERO,
        CancelToken::new(),
    );
    Ok(DispatchTicket::mint(
        &guard,
        call.workspace,
        call.organization,
        EffectId(effect),
        u16::try_from(call.attempt).map_err(|_| "tool attempt exceeds ticket range".to_owned())?,
        BrainTimestamp::from_millis(now_millis()),
    ))
}

fn identity_digest(call: &ToolCallIdentity) -> [u8; 32] {
    Sha256::digest(serde_json::to_vec(call).unwrap_or_default()).into()
}

fn brain_session(session: SessionId) -> BrainSessionId {
    BrainSessionId(uuid::Uuid::from_bytes(*session.uuid7().as_bytes()))
}

// ToolStartRequest owns these bounds; Hands start receives them through a
// task-local request context installed by the HTTP adapter.
tokio::task_local! {
    static CALL_BOUNDS: (i64, u32, usize);
}

pub(crate) async fn with_call_bounds<T>(
    deadline: i64,
    timeout: u32,
    max_result_bytes: usize,
    future: impl core::future::Future<Output = T>,
) -> T {
    CALL_BOUNDS
        .scope((deadline, timeout, max_result_bytes), future)
        .await
}

fn deadline_ms(_call: &ToolCallIdentity) -> i64 {
    CALL_BOUNDS
        .try_with(|value| value.0)
        .unwrap_or_else(|_| now_millis().saturating_add(60_000))
}

fn timeout_ms(_call: &ToolCallIdentity) -> u32 {
    CALL_BOUNDS.try_with(|value| value.1).unwrap_or(60_000)
}

fn max_result_bytes(_call: &ToolCallIdentity) -> usize {
    CALL_BOUNDS
        .try_with(|value| value.2)
        .unwrap_or(aex_hands_protocol::rpc::MAX_GUEST_BODY_BYTES)
}

fn now_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .and_then(|duration| i64::try_from(duration.as_millis()).ok())
        .unwrap_or(0)
}
