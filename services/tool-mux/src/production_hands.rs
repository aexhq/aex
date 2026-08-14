//! Production exact-generation runtime waiter and Hands guest adapters.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

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
    SandboxMcpCall, SandboxMcpTransport,
};
use aex_hands_protocol::rpc::HandsOperationId;
use aex_runtime_control::HandId;
use aex_tool_mux::{
    ExecutorOutput, FullOutput, GuestPort, ReadyHand, RuntimePort, ToolCallIdentity, ToolMuxFuture,
    ToolTarget,
};
use aex_wire::CanonicalJson;
use aex_wire::ids::{ContentHash, GenerationId, PrefixedId as _, SessionId, Uuid7};
use sha2::{Digest as _, Sha256};

use crate::mcp::McpSecretReader;
use crate::preparation::SandboxPreparationPort;

const GUEST_HELPER: &str = "/proc/self/exe";

/// Runtime Control port over the one production Hands lifecycle authority.
pub struct ProductionRuntimeAdapter {
    preparation: Arc<dyn SandboxPreparationPort>,
    waiters: Arc<Mutex<BTreeMap<HandsOperationId, WaiterState>>>,
}

#[derive(Debug, Clone, Copy)]
enum WaiterState {
    Running { recovered: bool },
    CancelRequested { recovered: bool },
    Ready(ReadyHand),
    Failed,
}

impl ProductionRuntimeAdapter {
    /// Binds exact-generation lifecycle operations.
    #[must_use]
    pub fn new(preparation: Arc<dyn SandboxPreparationPort>) -> Self {
        Self {
            preparation,
            waiters: Arc::new(Mutex::new(BTreeMap::new())),
        }
    }

    fn schedule(
        &self,
        session: SessionId,
        hand: HandId,
        generation: GenerationId,
        call: &ToolCallIdentity,
        recovered: bool,
    ) -> Result<(), String> {
        if hand != HandId::for_session(session) || call.session != session {
            return Err("tool waiter identity does not own the Hand".to_owned());
        }
        let operation = operation_id(call);
        {
            let mut waiters = self
                .waiters
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if waiters.contains_key(&operation) {
                return Ok(());
            }
            waiters.insert(operation, WaiterState::Running { recovered });
        }
        let preparation = Arc::clone(&self.preparation);
        let waiters = Arc::clone(&self.waiters);
        let call = call.clone();
        tokio::spawn(async move {
            let outcome = preparation
                .prepare_for_tool(call.workspace, session, generation, &call)
                .await;
            let settle = {
                let mut states = waiters
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                let cancelled = matches!(
                    states.get(&operation),
                    Some(WaiterState::CancelRequested { .. })
                );
                match outcome {
                    Ok(ready) if cancelled => {
                        states.remove(&operation);
                        Some(ready)
                    }
                    Ok(ready) => {
                        states.insert(operation, WaiterState::Ready(ready));
                        None
                    }
                    Err(_) => {
                        preparation.preparation_failed(session, generation, &call.call);
                        states.insert(operation, WaiterState::Failed);
                        None
                    }
                }
            };
            if let Some(ready) = settle {
                let _ = preparation.settle_tool(ready, &call).await;
            }
        });
        Ok(())
    }
}

impl RuntimePort for ProductionRuntimeAdapter {
    fn start_waiter<'a>(
        &'a self,
        session: SessionId,
        hand: HandId,
        generation: GenerationId,
        call: &'a ToolCallIdentity,
    ) -> ToolMuxFuture<'a, Result<(), String>> {
        Box::pin(async move { self.schedule(session, hand, generation, call, false) })
    }

    fn poll_waiter<'a>(
        &'a self,
        session: SessionId,
        hand: HandId,
        generation: GenerationId,
        call: &'a ToolCallIdentity,
    ) -> ToolMuxFuture<'a, Result<Option<ReadyHand>, String>> {
        Box::pin(async move {
            if hand != HandId::for_session(session) || call.session != session {
                return Err("tool waiter identity does not own the Hand".to_owned());
            }
            let operation = operation_id(call);
            let state = self
                .waiters
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .get(&operation)
                .copied();
            match state {
                Some(WaiterState::Running { .. }) => Ok(None),
                Some(WaiterState::Ready(ready))
                    if ready.hand == hand && ready.generation == generation =>
                {
                    Ok(Some(ready))
                }
                Some(WaiterState::Ready(_)) => {
                    Err("tool waiter produced a foreign generation".to_owned())
                }
                Some(WaiterState::CancelRequested { .. }) => {
                    Err("tool waiter was cancelled".to_owned())
                }
                Some(WaiterState::Failed) => Err("sandbox preparation failed".to_owned()),
                None => {
                    self.schedule(session, hand, generation, call, true)?;
                    Ok(None)
                }
            }
        })
    }

    fn cancel_waiter<'a>(
        &'a self,
        generation: GenerationId,
        call: &'a ToolCallIdentity,
    ) -> ToolMuxFuture<'a, Result<Option<ReadyHand>, String>> {
        Box::pin(async move {
            let operation = operation_id(call);
            let (ready, recover) = {
                let mut waiters = self
                    .waiters
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                match waiters.get(&operation).copied() {
                    Some(WaiterState::Running { recovered: false }) => {
                        waiters
                            .insert(operation, WaiterState::CancelRequested { recovered: false });
                        (None, false)
                    }
                    Some(WaiterState::Running { recovered: true }) => {
                        waiters.insert(operation, WaiterState::CancelRequested { recovered: true });
                        (None, true)
                    }
                    Some(WaiterState::Ready(ready)) => {
                        waiters.remove(&operation);
                        (Some(ready), false)
                    }
                    Some(WaiterState::CancelRequested { recovered }) => (None, recovered),
                    Some(WaiterState::Failed) => {
                        return Err("sandbox preparation failed".to_owned());
                    }
                    None => (None, true),
                }
            };
            if let Some(ready) = ready {
                return Ok(Some(ready));
            }
            if !recover {
                return Ok(None);
            }
            // Process-local state cannot prove whether the deterministic guest
            // operation was already dispatched before a restart. Recover the
            // exact retained generation and let ToolMux cancel that operation
            // directly; never acknowledge cancellation based only on an empty
            // waiter map.
            let ready = self
                .preparation
                .prepare_for_tool(call.workspace, call.session, generation, call)
                .await?;
            Ok(Some(ready))
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
            self.preparation.settle_tool(ready, call).await?;
            self.waiters
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .remove(&operation_id(call));
            Ok(())
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
            ToolTarget::RemoteMcp {
                endpoint,
                headers,
                tool,
                ..
            } => {
                let mut revealed = std::collections::BTreeMap::new();
                for (name, secret) in headers {
                    let value = self.secrets.reveal(secret, call).await?;
                    revealed.insert(name.clone(), EnvValue::new(value.to_string()));
                }
                mcp_operation(
                    SandboxMcpTransport::StreamableHttp {
                        endpoint: endpoint.clone(),
                        headers: revealed,
                    },
                    tool,
                    arguments,
                )
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
                mcp_operation(
                    SandboxMcpTransport::ChildProcess {
                        command: command.clone(),
                        args: args.clone(),
                        environment: revealed,
                        working_directory: cwd,
                    },
                    tool,
                    arguments,
                )
            }
            ToolTarget::StoragePersist { .. } => {
                Err("guest adapter received a non-guest target".to_owned())
            }
        }
    }
}

fn mcp_operation(
    transport: SandboxMcpTransport,
    tool: &str,
    arguments: &serde_json::Value,
) -> Result<OperationRequest, String> {
    let request = SandboxMcpCall {
        transport,
        tool: tool.to_owned(),
        arguments: CanonicalJson::from_value(arguments)
            .map_err(|_| "MCP arguments are not canonical".to_owned())?,
    };
    Ok(OperationRequest::Exec {
        argv: vec![GUEST_HELPER.to_owned(), "mcp-call".to_owned()],
        cwd: GuestPath::parse(&GuestRoot::workspace(), "/workspace")
            .map_err(|_| "workspace path is invalid".to_owned())?,
        env: vec![(
            EnvName(SANDBOX_MCP_REQUEST_VAR.to_owned()),
            EnvValue::new(
                serde_json::to_string(&request)
                    .map_err(|_| "MCP request is not encodable".to_owned())?,
            ),
        )],
        stdin: None,
    })
}

impl GuestPort for ProductionGuestAdapter {
    fn hello(&self, ready: ReadyHand) -> ToolMuxFuture<'_, Result<(), String>> {
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
        deadline_ms: i64,
        max_result_bytes: usize,
        timeout_ms: u32,
    ) -> ToolMuxFuture<'a, Result<HandsOperationId, String>> {
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
                            max_bytes: max_result_bytes,
                            max_stream_bytes: aex_tool_mux::MAX_LIVE_PREVIEW_BYTES,
                            timeout_ms,
                        },
                        deadline: BrainTimestamp::from_millis(deadline_ms),
                    },
                )
                .await
                .map_err(|_| "exact-generation guest dispatch failed".to_owned())?;
            if accepted.operation.0 != operation.0.to_string()
                || accepted.generation != ready.generation
            {
                return Err("guest accepted a foreign detached operation".to_owned());
            }
            Ok(operation)
        })
    }

    fn read(
        &self,
        ready: ReadyHand,
        operation: HandsOperationId,
        max_result_bytes: usize,
        timeout_ms: u32,
    ) -> ToolMuxFuture<'_, Result<Option<ExecutorOutput>, String>> {
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
        call: &'a ToolCallIdentity,
    ) -> ToolMuxFuture<'a, Result<(), String>> {
        Box::pin(async move {
            let operation = operation_id(call);
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
    match (result.inline, result.sandbox_file) {
        (Some(body), None) => Ok(ExecutorOutput {
            preview: body.as_bytes()[..body.len().min(aex_tool_mux::MAX_LIVE_PREVIEW_BYTES)]
                .to_vec(),
            full: FullOutput::Inline(body.into_bytes()),
            is_error: result.exit_code != 0,
        }),
        (None, Some(file)) => Ok(ExecutorOutput {
            preview: file.preview.into_bytes(),
            full: FullOutput::SandboxFile {
                path: GuestPath::parse(&GuestRoot::workspace(), &file.path)
                    .map_err(|_| "guest large-result path is invalid".to_owned())?,
                bytes: file.byte_len,
                hash: ContentHash::from_bytes(result.checksum.0),
            },
            is_error: result.exit_code != 0,
        }),
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

fn now_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .and_then(|duration| i64::try_from(duration.as_millis()).ok())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    use aex_hands_protocol::rpc::Fence;
    use aex_wire::ids::{AgentId, MessageId, OrganizationId, WorkspaceId};

    use super::*;

    struct PreparationFake {
        starts: AtomicUsize,
        released: AtomicBool,
        gate: tokio::sync::Notify,
    }

    impl PreparationFake {
        fn new(released: bool) -> Self {
            Self {
                starts: AtomicUsize::new(0),
                released: AtomicBool::new(released),
                gate: tokio::sync::Notify::new(),
            }
        }
    }

    impl SandboxPreparationPort for PreparationFake {
        fn prepare_for_tool<'a>(
            &'a self,
            _workspace: WorkspaceId,
            session: SessionId,
            generation: GenerationId,
            _call: &'a ToolCallIdentity,
        ) -> ToolMuxFuture<'a, Result<ReadyHand, String>> {
            Box::pin(async move {
                self.starts.fetch_add(1, Ordering::SeqCst);
                while !self.released.load(Ordering::SeqCst) {
                    self.gate.notified().await;
                }
                Ok(ReadyHand {
                    hand: HandId::for_session(session),
                    generation,
                    fence: Fence(7),
                })
            })
        }

        fn settle_tool<'a>(
            &'a self,
            _ready: ReadyHand,
            _call: &'a ToolCallIdentity,
        ) -> ToolMuxFuture<'a, Result<(), String>> {
            Box::pin(async { Ok(()) })
        }

        fn preparation_failed(&self, _session: SessionId, _generation: GenerationId, _call: &str) {}
    }

    fn id<T: aex_wire::ids::PrefixedId>(seed: u8) -> T {
        T::from_uuid7(Uuid7::compose(u64::from(seed), [seed; 10]))
    }

    fn call() -> ToolCallIdentity {
        ToolCallIdentity {
            organization: id::<OrganizationId>(1),
            workspace: id::<WorkspaceId>(2),
            session: id::<SessionId>(3),
            agent: id::<AgentId>(4),
            message: id::<MessageId>(5),
            batch: 0,
            call: "detached-call".to_owned(),
            attempt: 1,
        }
    }

    async fn wait_ready(
        adapter: &ProductionRuntimeAdapter,
        identity: &ToolCallIdentity,
        generation: GenerationId,
    ) -> ReadyHand {
        for _ in 0..100 {
            if let Some(ready) = adapter
                .poll_waiter(
                    identity.session,
                    HandId::for_session(identity.session),
                    generation,
                    identity,
                )
                .await
                .expect("poll")
            {
                return ready;
            }
            tokio::task::yield_now().await;
        }
        panic!("preparation did not finish")
    }

    #[tokio::test]
    async fn start_returns_before_the_first_generation_is_ready() {
        let preparation = Arc::new(PreparationFake::new(false));
        let adapter = ProductionRuntimeAdapter::new(preparation.clone());
        let identity = call();
        let generation = id::<GenerationId>(6);
        tokio::time::timeout(
            std::time::Duration::from_millis(50),
            adapter.start_waiter(
                identity.session,
                HandId::for_session(identity.session),
                generation,
                &identity,
            ),
        )
        .await
        .expect("start is detached")
        .expect("scheduled");
        assert!(
            adapter
                .poll_waiter(
                    identity.session,
                    HandId::for_session(identity.session),
                    generation,
                    &identity,
                )
                .await
                .expect("poll")
                .is_none()
        );
        preparation.released.store(true, Ordering::SeqCst);
        preparation.gate.notify_waiters();
        assert_eq!(
            wait_ready(&adapter, &identity, generation).await.generation,
            generation
        );
    }

    #[tokio::test]
    async fn missing_process_state_is_reconstructed_by_read_polling() {
        let preparation = Arc::new(PreparationFake::new(true));
        let adapter = ProductionRuntimeAdapter::new(preparation.clone());
        let identity = call();
        let generation = id::<GenerationId>(6);
        assert!(
            adapter
                .poll_waiter(
                    identity.session,
                    HandId::for_session(identity.session),
                    generation,
                    &identity,
                )
                .await
                .expect("reconstruction scheduled")
                .is_none()
        );
        assert_eq!(
            wait_ready(&adapter, &identity, generation).await.generation,
            generation
        );
        assert_eq!(preparation.starts.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn missing_process_state_is_reconstructed_before_cancellation_acknowledges() {
        let preparation = Arc::new(PreparationFake::new(true));
        let adapter = ProductionRuntimeAdapter::new(preparation.clone());
        let identity = call();
        let generation = id::<GenerationId>(6);
        let ready = adapter
            .cancel_waiter(generation, &identity)
            .await
            .expect("restart cancellation recovers")
            .expect("guest cancellation must follow");
        assert_eq!(ready.generation, generation);
        assert_eq!(preparation.starts.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn original_preparation_cancellation_does_not_start_a_guest_operation() {
        let preparation = Arc::new(PreparationFake::new(false));
        let adapter = ProductionRuntimeAdapter::new(preparation);
        let identity = call();
        let generation = id::<GenerationId>(6);
        adapter
            .start_waiter(
                identity.session,
                HandId::for_session(identity.session),
                generation,
                &identity,
            )
            .await
            .expect("waiter starts");
        assert!(
            adapter
                .cancel_waiter(generation, &identity)
                .await
                .expect("pre-dispatch cancellation")
                .is_none()
        );
    }
}
