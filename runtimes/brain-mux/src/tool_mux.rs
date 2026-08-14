//! Brain client for the placed `ToolMux` service.

use std::sync::Arc;

use aex_brain_app::ports::{
    BoxFuture, CancelToken, DetachedStatus, DispatchTicket, PreparedToolCall, ProviderFailureKind,
    RedactedDetail, ToolDispatchError, ToolOutcome, ToolResultBody,
};
use aex_brain_domain::effect::{DispatchProof, DispatchStage};
use aex_brain_domain::ids::{ContentHash, DetachedOperationId, Fence, ToolName};
use aex_brain_domain::journal::ExecutorRoute;
use aex_brain_domain::mcp::FrozenMcpTransport;
use aex_brain_tool_catalog::router::ToolExecutor;
use aex_control_domain::scope::ScopeSet;
use aex_hands_protocol::operation::{GuestPath, GuestRoot};
use aex_identity_domain::assertion::{
    AssertedAccountState, AssertionClaims, Audience, EpochSlots, LocalSigner, Plane, PrincipalKind,
    issue,
};
use aex_internal_contracts::assertion::AssertionAudience;
use aex_model_catalog::{BoundedString, canonical::ToolResultPart};
use aex_tool_mux::{
    OfficialSandboxTool, SandboxConfig, ToolCallIdentity, ToolCompletion, ToolHandle,
    ToolHandleRequest, ToolRead, ToolStart, ToolStartRequest, ToolTarget,
};
use aex_wire::CanonicalJson;
use aex_wire::ids::{AgentId, MessageId, PrefixedId as _, SessionId, Uuid7};
use base64::Engine as _;

const DETACHED_PREFIX: &str = "tool-mux.v1";
const ASSERTION_HEADER: &str = "x-aex-tool-assertion";

/// Placed `ToolMux` client. It is the only non-native tool executor Brain owns.
pub struct RemoteToolMux {
    endpoint: String,
    client: reqwest::Client,
    signer: Arc<LocalSigner>,
    plane: Plane,
    region: aex_wire::types::Region,
}

impl RemoteToolMux {
    /// Binds the private service endpoint and local `AgentSession` signer.
    #[must_use]
    pub fn new(
        endpoint: &str,
        signer: Arc<LocalSigner>,
        plane: Plane,
        region: aex_wire::types::Region,
    ) -> Self {
        Self {
            endpoint: endpoint.trim_end_matches('/').to_owned(),
            client: reqwest::Client::new(),
            signer,
            plane,
            region,
        }
    }

    fn assertion<T: serde::Serialize>(
        &self,
        ticket: &DispatchTicket,
        request: &T,
    ) -> Result<String, ToolDispatchError> {
        let now = u64::try_from(ticket.issued_at().millis())
            .map_err(|_| invalid("ToolMux assertion timestamp is invalid"))?;
        let binding = aex_tool_mux::request_binding(request)
            .map_err(|_| invalid("ToolMux request binding failed"))?;
        let claims = AssertionClaims {
            issued_at_ms: now,
            expires_at_ms: now.saturating_add(30_000),
            audience: Audience {
                plane: self.plane,
                region: self.region,
                service: AssertionAudience::ToolMux,
            },
            principal_kind: PrincipalKind::AgentSession,
            principal_id: ticket.key().session.0,
            credential_binding: binding,
            organization_id: uuid::Uuid::from_bytes(*ticket.organization().uuid7().as_bytes()),
            workspace_id: uuid::Uuid::from_bytes(*ticket.workspace().uuid7().as_bytes()),
            workspace_region: self.region,
            account_state: AssertedAccountState::Active,
            scopes: ScopeSet::EMPTY,
            epochs: EpochSlots::EMPTY,
        };
        issue(self.signer.as_ref(), &claims)
            .map(|assertion| assertion.to_base64url())
            .map_err(|_| invalid("ToolMux assertion could not be issued"))
    }

    async fn post<T: serde::Serialize, R: serde::de::DeserializeOwned>(
        &self,
        path: &str,
        assertion: &str,
        request: &T,
    ) -> Result<R, ToolDispatchError> {
        let response = self
            .client
            .post(format!("{}{path}", self.endpoint))
            .header(ASSERTION_HEADER, assertion)
            .json(request)
            .send()
            .await
            .map_err(|_| ambiguous("ToolMux dispatch outcome is unknown"))?;
        if !response.status().is_success() {
            return Err(dispatched(
                "ToolMux refused or failed the dispatched request",
            ));
        }
        response
            .json()
            .await
            .map_err(|_| dispatched("ToolMux response violated its bounded contract"))
    }
}

impl core::fmt::Debug for RemoteToolMux {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("RemoteToolMux")
            .field("endpoint", &self.endpoint)
            .finish_non_exhaustive()
    }
}

impl ToolExecutor for RemoteToolMux {
    fn supports(&self, tool: &ToolName) -> bool {
        matches!(
            tool.as_str(),
            "read_file" | "edit_file" | "write_file" | "bash" | "storage_persist" | "mcp_call"
        )
    }

    fn invoke<'a>(
        &'a self,
        ticket: &'a DispatchTicket,
        call: &'a PreparedToolCall,
        cancel: &'a CancelToken,
    ) -> BoxFuture<'a, Result<ToolOutcome, ToolDispatchError>> {
        Box::pin(async move {
            if cancel.is_cancelled() {
                return Err(cancelled("ToolMux call was cancelled before dispatch"));
            }
            let identity = identity(ticket)?;
            let target = target(call)?;
            let request = ToolStartRequest {
                identity: identity.clone(),
                sandbox: SandboxConfig {
                    enabled: call.hands_generation.is_some(),
                    generation: call.hands_generation,
                },
                target,
                arguments: arguments(call),
                deadline_ms: ticket
                    .issued_at()
                    .millis()
                    .saturating_add(i64::from(call.route.timeout_ms)),
                max_result_bytes: call.max_result_bytes,
                timeout_ms: call.route.timeout_ms,
            };
            let assertion = self.assertion(ticket, &request)?;
            match self
                .post::<_, ToolStart>("/internal/tools/start", &assertion, &request)
                .await?
            {
                ToolStart::Completed { result } => completion(result).map(ToolOutcome::Completed),
                ToolStart::Accepted { handle } => Ok(ToolOutcome::Detached {
                    operation: encode_detached(&identity, &handle)?,
                    poll_after: core::time::Duration::from_millis(250),
                }),
            }
        })
    }

    fn query<'a>(
        &'a self,
        operation: &'a DetachedOperationId,
    ) -> BoxFuture<'a, Result<DetachedStatus, ToolDispatchError>> {
        Box::pin(async move {
            let request = decode_detached(operation)?;
            // Detached reads are authorized by the exact request identity. The
            // signer input is reconstructed from the durable handle, so a
            // dropped activation never needs a bearer token.
            let ticket = detached_ticket(&request.identity)?;
            let assertion = self.assertion(&ticket, &request)?;
            match self
                .post::<_, ToolRead>("/internal/tools/read", &assertion, &request)
                .await?
            {
                ToolRead::Pending => Ok(DetachedStatus::Running {
                    poll_after: core::time::Duration::from_millis(250),
                }),
                ToolRead::Completed { result } => {
                    completion(result).map(|body| DetachedStatus::Completed(Box::new(body)))
                }
            }
        })
    }

    fn cancel<'a>(
        &'a self,
        operation: &'a DetachedOperationId,
        _fence: Fence,
    ) -> BoxFuture<'a, Result<(), ToolDispatchError>> {
        Box::pin(async move {
            let request = decode_detached(operation)?;
            let ticket = detached_ticket(&request.identity)?;
            let assertion = self.assertion(&ticket, &request)?;
            let response = self
                .client
                .post(format!("{}/internal/tools/cancel", self.endpoint))
                .header(ASSERTION_HEADER, assertion)
                .json(&request)
                .send()
                .await
                .map_err(|_| ambiguous("ToolMux cancellation outcome is unknown"))?;
            if response.status().is_success() {
                Ok(())
            } else {
                Err(dispatched("ToolMux cancellation failed"))
            }
        })
    }
}

fn target(call: &PreparedToolCall) -> Result<ToolTarget, ToolDispatchError> {
    match call.route.name.as_str() {
        "read_file" => Ok(ToolTarget::OfficialSandbox {
            tool: OfficialSandboxTool::Read,
        }),
        "edit_file" => Ok(ToolTarget::OfficialSandbox {
            tool: OfficialSandboxTool::Edit,
        }),
        "write_file" => Ok(ToolTarget::OfficialSandbox {
            tool: OfficialSandboxTool::Write,
        }),
        "bash" => Ok(ToolTarget::OfficialSandbox {
            tool: OfficialSandboxTool::Bash,
        }),
        "storage_persist" => {
            let input = call.input.to_value();
            let source = input
                .get("path")
                .and_then(serde_json::Value::as_str)
                .ok_or_else(|| invalid("storage.persist requires path"))?;
            Ok(ToolTarget::StoragePersist {
                source: GuestPath::parse(&GuestRoot::workspace(), source)
                    .map_err(|_| invalid("storage.persist path is outside /workspace"))?,
                logical_name: input
                    .get("name")
                    .and_then(serde_json::Value::as_str)
                    .ok_or_else(|| invalid("storage.persist requires name"))?
                    .to_owned(),
                media_type: input
                    .get("mediaType")
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_owned),
            })
        }
        "mcp_call" => mcp_target(call),
        _ => Err(invalid("ToolMux received an unsupported tool")),
    }
}

fn mcp_target(call: &PreparedToolCall) -> Result<ToolTarget, ToolDispatchError> {
    let input = call.input.to_value();
    let server_name = input
        .get("server")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| invalid("mcp_call requires server"))?;
    let tool = input
        .get("tool")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| invalid("mcp_call requires tool"))?
        .to_owned();
    let server = call
        .mcp_servers
        .iter()
        .find(|server| server.name.as_str() == server_name)
        .ok_or_else(|| invalid("mcp_call named a server outside this session"))?;
    match &server.transport {
        FrozenMcpTransport::RemoteHttp { endpoint, headers } => Ok(ToolTarget::RemoteMcp {
            server: server.name.clone(),
            endpoint: endpoint.clone(),
            headers: headers.clone(),
            tool,
        }),
        FrozenMcpTransport::SandboxProcess {
            command,
            args,
            environment,
            working_directory,
        } => Ok(ToolTarget::SandboxMcp {
            server: server.name.clone(),
            command: command.clone(),
            args: args.clone(),
            environment: environment.clone(),
            working_directory: working_directory.clone(),
            tool,
        }),
    }
}

fn arguments(call: &PreparedToolCall) -> serde_json::Value {
    let input = call.input.to_value();
    if call.route.name.as_str() == "mcp_call" {
        input
            .get("arguments")
            .cloned()
            .unwrap_or_else(|| serde_json::json!({}))
    } else {
        input
    }
}

fn completion(result: ToolCompletion) -> Result<ToolResultBody, ToolDispatchError> {
    let (part, bytes) = if result.truncated {
        let value = CanonicalJson::from_value(&serde_json::json!({
            "preview": result.preview,
            "previewTruncated": true,
            "outputFile": result.output_file,
            "error": result.error,
        }))
        .map_err(|_| dispatched("ToolMux local output reference is invalid"))?;
        let bytes = value.as_bytes().to_vec();
        (ToolResultPart::Json { value }, bytes)
    } else if let Ok(value) = CanonicalJson::parse(&result.preview) {
        let bytes = value.as_bytes().to_vec();
        (ToolResultPart::Json { value }, bytes)
    } else {
        let bytes = result.preview.as_bytes().to_vec();
        let text = BoundedString::new(result.preview)
            .map_err(|_| dispatched("ToolMux preview exceeded the model result bound"))?;
        (ToolResultPart::Text { text }, bytes)
    };
    Ok(ToolResultBody {
        content: vec![part],
        is_error: result.error.is_some(),
        duration_ms: 0,
        executed_on: ExecutorRoute::ToolMux,
        checksum: ContentHash::of(&bytes),
    })
}

#[derive(serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct DetachedRequest {
    identity: ToolCallIdentity,
    handle: ToolHandle,
}

fn encode_detached(
    identity: &ToolCallIdentity,
    handle: &ToolHandle,
) -> Result<DetachedOperationId, ToolDispatchError> {
    let bytes = serde_json::to_vec(&DetachedRequest {
        identity: identity.clone(),
        handle: handle.clone(),
    })
    .map_err(|_| dispatched("ToolMux detached handle is not encodable"))?;
    Ok(DetachedOperationId(format!(
        "{DETACHED_PREFIX}:{}",
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
    )))
}

fn decode_detached(
    operation: &DetachedOperationId,
) -> Result<ToolHandleRequest, ToolDispatchError> {
    let encoded = operation
        .0
        .strip_prefix(&format!("{DETACHED_PREFIX}:"))
        .ok_or_else(|| invalid("ToolMux detached handle is malformed"))?;
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(encoded)
        .map_err(|_| invalid("ToolMux detached handle is malformed"))?;
    let detached: DetachedRequest = serde_json::from_slice(&bytes)
        .map_err(|_| invalid("ToolMux detached handle is malformed"))?;
    Ok(ToolHandleRequest {
        identity: detached.identity,
        handle: detached.handle,
    })
}

fn identity(ticket: &DispatchTicket) -> Result<ToolCallIdentity, ToolDispatchError> {
    let key = ticket.key();
    let mut message = ticket.effect().0;
    message[0] ^= 1;
    message[6] = 0x70 | (message[6] & 0x0f);
    message[8] = 0x80 | (message[8] & 0x3f);
    Ok(ToolCallIdentity {
        organization: ticket.organization(),
        workspace: ticket.workspace(),
        session: SessionId::from_uuid7(
            Uuid7::from_bytes(*key.session.0.as_bytes())
                .map_err(|_| invalid("session identity is not UUIDv7"))?,
        ),
        agent: AgentId::from_uuid7(
            Uuid7::from_bytes(*key.agent.0.as_bytes())
                .map_err(|_| invalid("agent identity is not UUIDv7"))?,
        ),
        message: MessageId::from_uuid7(
            Uuid7::from_bytes(message).map_err(|_| invalid("effect identity is not UUIDv7"))?,
        ),
        batch: 0,
        call: hex::encode(ticket.effect().0),
        attempt: u32::from(ticket.attempt()),
    })
}

fn detached_ticket(identity: &ToolCallIdentity) -> Result<DispatchTicket, ToolDispatchError> {
    use aex_brain_app::ports::FenceGuard;
    use aex_brain_domain::ids::{
        AgentId as BrainAgentId, AgentKey, AgentRevision, CancelEpoch, EffectId, OwnerToken,
        SessionId as BrainSessionId, Timestamp,
    };
    let effect = hex::decode(&identity.call)
        .ok()
        .and_then(|bytes| <[u8; 16]>::try_from(bytes.as_slice()).ok())
        .ok_or_else(|| invalid("ToolMux detached effect identity is malformed"))?;
    let guard = FenceGuard::new(
        AgentKey::new(
            BrainSessionId(uuid::Uuid::from_bytes(*identity.session.uuid7().as_bytes())),
            BrainAgentId(uuid::Uuid::from_bytes(*identity.agent.uuid7().as_bytes())),
        ),
        OwnerToken(uuid::Uuid::from_bytes(effect)),
        Fence(0),
        AgentRevision(0),
        None,
        CancelEpoch::ZERO,
        CancelToken::new(),
    );
    Ok(DispatchTicket::mint(
        &guard,
        identity.workspace,
        identity.organization,
        EffectId(effect),
        u16::try_from(identity.attempt).map_err(|_| invalid("ToolMux attempt is malformed"))?,
        Timestamp::from_millis(now_millis()),
    ))
}

fn now_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .and_then(|duration| i64::try_from(duration.as_millis()).ok())
        .unwrap_or(0)
}

fn invalid(message: &str) -> ToolDispatchError {
    error(
        DispatchStage::PreDispatch,
        DispatchProof::NotSent,
        ProviderFailureKind::InvalidRequest,
        false,
        message,
    )
}

fn cancelled(message: &str) -> ToolDispatchError {
    error(
        DispatchStage::PreDispatch,
        DispatchProof::NotSent,
        ProviderFailureKind::Cancelled,
        false,
        message,
    )
}

fn ambiguous(message: &str) -> ToolDispatchError {
    error(
        DispatchStage::Dispatched,
        DispatchProof::PossiblySent,
        ProviderFailureKind::Transport,
        true,
        message,
    )
}

fn dispatched(message: &str) -> ToolDispatchError {
    error(
        DispatchStage::Terminal,
        DispatchProof::ResponseStarted,
        ProviderFailureKind::ServerError,
        false,
        message,
    )
}

fn error(
    stage: DispatchStage,
    proof: DispatchProof,
    kind: ProviderFailureKind,
    retryable: bool,
    message: &str,
) -> ToolDispatchError {
    ToolDispatchError {
        stage,
        proof,
        retryable,
        detail: RedactedDetail::internal(kind, message),
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;
    use aex_identity_domain::assertion::{
        AssertionSigner as _, KeyId, VerificationKey, VerificationKeySet,
    };
    use aex_tool_mux::{
        ExecutorOutput, FullOutput, GuestPort, ReadyHand, RuntimePort, StoragePersistPort,
        TelemetryEnvelope, TelemetryPort, TelemetryPressure, ToolMux, ToolMuxFuture,
    };
    use aex_wire::ids::{GenerationId, OrganizationId, WorkspaceId};
    use zeroize::Zeroizing;

    struct ContractPorts;

    impl RuntimePort for ContractPorts {
        fn start_waiter<'a>(
            &'a self,
            _session: SessionId,
            _hand: aex_runtime_control::HandId,
            _generation: GenerationId,
            _call: &'a ToolCallIdentity,
        ) -> ToolMuxFuture<'a, Result<(), String>> {
            Box::pin(async { Ok(()) })
        }

        fn poll_waiter<'a>(
            &'a self,
            session: SessionId,
            hand: aex_runtime_control::HandId,
            generation: GenerationId,
            _call: &'a ToolCallIdentity,
        ) -> ToolMuxFuture<'a, Result<Option<ReadyHand>, String>> {
            Box::pin(async move {
                assert_eq!(hand, aex_runtime_control::HandId::for_session(session));
                Ok(Some(ReadyHand {
                    hand,
                    generation,
                    fence: aex_hands_protocol::rpc::Fence(9),
                }))
            })
        }

        fn cancel_waiter<'a>(
            &'a self,
            _generation: GenerationId,
            _call: &'a ToolCallIdentity,
        ) -> ToolMuxFuture<'a, Result<Option<ReadyHand>, String>> {
            Box::pin(async { Ok(None) })
        }

        fn settle_waiter<'a>(
            &'a self,
            _ready: ReadyHand,
            _call: &'a ToolCallIdentity,
        ) -> ToolMuxFuture<'a, Result<(), String>> {
            Box::pin(async { Ok(()) })
        }
    }

    impl GuestPort for ContractPorts {
        fn hello(&self, _ready: ReadyHand) -> ToolMuxFuture<'_, Result<(), String>> {
            Box::pin(async { Ok(()) })
        }

        fn start<'a>(
            &'a self,
            _ready: ReadyHand,
            target: &'a ToolTarget,
            arguments: &'a serde_json::Value,
            _call: &'a ToolCallIdentity,
            _deadline_ms: i64,
            _max_result_bytes: usize,
            _timeout_ms: u32,
        ) -> ToolMuxFuture<'a, Result<aex_hands_protocol::rpc::HandsOperationId, String>> {
            Box::pin(async move {
                let ToolTarget::RemoteMcp {
                    endpoint,
                    headers,
                    server,
                    tool,
                } = target
                else {
                    return Err("unexpected target".to_owned());
                };
                assert_eq!(endpoint, "https://mcp.internal");
                assert!(headers.is_empty());
                assert_eq!(server.as_str(), "fixture");
                assert_eq!(tool, "echo");
                assert_eq!(arguments, &serde_json::json!({"value": 7}));
                Ok(aex_hands_protocol::rpc::HandsOperationId(Uuid7::compose(
                    8, [8; 10],
                )))
            })
        }

        fn read(
            &self,
            _ready: ReadyHand,
            _operation: aex_hands_protocol::rpc::HandsOperationId,
            _max_result_bytes: usize,
            _timeout_ms: u32,
        ) -> ToolMuxFuture<'_, Result<Option<ExecutorOutput>, String>> {
            Box::pin(async {
                let body = br#"{"echo":7}"#.to_vec();
                Ok(Some(ExecutorOutput {
                    preview: body.clone(),
                    full: FullOutput::Inline(body),
                    is_error: false,
                }))
            })
        }

        fn cancel<'a>(
            &'a self,
            _ready: ReadyHand,
            _call: &'a ToolCallIdentity,
        ) -> ToolMuxFuture<'a, Result<(), String>> {
            Box::pin(async { Ok(()) })
        }
    }

    impl StoragePersistPort for ContractPorts {
        fn start_persist<'a>(
            &'a self,
            _ready: ReadyHand,
            _source: &'a GuestPath,
            _logical_name: &'a str,
            _media_type: Option<&'a str>,
            _call: &'a ToolCallIdentity,
        ) -> ToolMuxFuture<'a, Result<(), String>> {
            Box::pin(async { Err("not used".to_owned()) })
        }

        fn read_persist<'a>(
            &'a self,
            _ready: ReadyHand,
            _call: &'a ToolCallIdentity,
        ) -> ToolMuxFuture<'a, Result<Option<ExecutorOutput>, String>> {
            Box::pin(async { Err("not used".to_owned()) })
        }

        fn cancel_persist<'a>(
            &'a self,
            _ready: ReadyHand,
            _call: &'a ToolCallIdentity,
        ) -> ToolMuxFuture<'a, Result<(), String>> {
            Box::pin(async { Err("not used".to_owned()) })
        }
    }

    impl TelemetryPort for ContractPorts {
        fn try_emit(&self, _event: TelemetryEnvelope) -> Result<(), TelemetryPressure> {
            Ok(())
        }
    }

    fn call() -> ToolCallIdentity {
        ToolCallIdentity {
            organization: OrganizationId::from_uuid7(Uuid7::compose(1, [1; 10])),
            workspace: WorkspaceId::from_uuid7(Uuid7::compose(2, [2; 10])),
            session: SessionId::from_uuid7(Uuid7::compose(3, [3; 10])),
            agent: AgentId::from_uuid7(Uuid7::compose(4, [4; 10])),
            message: MessageId::from_uuid7(Uuid7::compose(5, [5; 10])),
            batch: 0,
            call: hex::encode([7; 16]),
            attempt: 1,
        }
    }

    #[test]
    fn detached_handle_round_trips_without_bearer_material() {
        let identity = call();
        let handle = ToolHandle::Sandbox {
            hand: aex_runtime_control::HandId::for_session(identity.session),
            generation: GenerationId::from_uuid7(Uuid7::compose(6, [6; 10])),
            target: Box::new(ToolTarget::OfficialSandbox {
                tool: aex_tool_mux::OfficialSandboxTool::Read,
            }),
            arguments: serde_json::json!({"path": "/workspace/input.txt"}),
            deadline_ms: 60_000,
            max_result_bytes: 1_048_576,
            timeout_ms: 60_000,
        };
        let encoded = encode_detached(&identity, &handle).expect("encode");
        let decoded = decode_detached(&encoded).expect("decode");
        assert_eq!(decoded.identity, identity);
        assert_eq!(decoded.handle, handle);
        assert!(!encoded.0.contains("token"));
    }

    #[test]
    fn transport_failure_is_ambiguously_dispatched() {
        let error = ambiguous("bounded");
        assert_eq!(error.stage, DispatchStage::Dispatched);
        assert_eq!(error.proof, DispatchProof::PossiblySent);
        assert!(error.retryable);
    }

    #[tokio::test]
    async fn placed_client_and_service_share_body_bound_contract() {
        let seed = Zeroizing::new([7_u8; 32]);
        let signer = Arc::new(LocalSigner::new(
            KeyId::new(uuid::Uuid::from_u128(11)),
            &seed,
        ));
        let keys = VerificationKeySet::new(vec![VerificationKey {
            kid: signer.kid(),
            public_key: signer.public_key(),
            not_after_ms: u64::MAX,
        }])
        .expect("verification keys");
        let ports = Arc::new(ContractPorts);
        let mux = Arc::new(ToolMux::new(
            ports.clone(),
            ports.clone(),
            ports.clone(),
            ports,
        ));
        let app = tool_mux::router(tool_mux::App::new(
            mux,
            Arc::new(tool_mux::auth::AssertionAuthorizer::new(
                keys,
                Plane::Dev,
                aex_wire::types::Region::EuWest1,
            )),
        ));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("private listener");
        let address = listener.local_addr().expect("listener address");
        let server = tokio::spawn(async move {
            axum::serve(listener, app)
                .await
                .expect("fixture service stopped cleanly");
        });
        let client = RemoteToolMux::new(
            &format!("http://{address}"),
            signer,
            Plane::Dev,
            aex_wire::types::Region::EuWest1,
        );
        let identity = call();
        let ticket = detached_ticket(&identity).expect("ticket");
        let request = ToolStartRequest {
            identity,
            sandbox: SandboxConfig {
                enabled: true,
                generation: Some(GenerationId::from_uuid7(Uuid7::compose(6, [6; 10]))),
            },
            target: ToolTarget::RemoteMcp {
                server: aex_wire::ids::ResourceName::parse("fixture").expect("server"),
                endpoint: "https://mcp.internal".to_owned(),
                headers: BTreeMap::new(),
                tool: "echo".to_owned(),
            },
            arguments: serde_json::json!({"value": 7}),
            deadline_ms: now_millis() + 30_000,
            max_result_bytes: 1_048_576,
            timeout_ms: 30_000,
        };
        let assertion = client.assertion(&ticket, &request).expect("assertion");
        let response: ToolStart = client
            .post("/internal/tools/start", &assertion, &request)
            .await
            .expect("placed contract round trip");
        let ToolStart::Accepted { handle } = response else {
            panic!("remote MCP starts detached")
        };
        let read = ToolHandleRequest {
            identity: request.identity.clone(),
            handle,
        };
        let ticket = detached_ticket(&read.identity).expect("detached ticket");
        let assertion = client.assertion(&ticket, &read).expect("read assertion");
        let ToolRead::Completed { result } = client
            .post::<_, ToolRead>("/internal/tools/read", &assertion, &read)
            .await
            .expect("detached result round trip")
        else {
            panic!("fixture result is ready")
        };
        assert_eq!(result.preview, r#"{"echo":7}"#);
        assert!(result.output_file.is_none());
        server.abort();
        let _ = server.await;
    }

    #[tokio::test]
    async fn actual_connect_failure_is_ambiguously_dispatched() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("reserve address");
        let address = listener.local_addr().expect("address");
        drop(listener);
        let signer = Arc::new(LocalSigner::new(
            KeyId::new(uuid::Uuid::from_u128(12)),
            &Zeroizing::new([8; 32]),
        ));
        let client = RemoteToolMux::new(
            &format!("http://{address}"),
            signer,
            Plane::Dev,
            aex_wire::types::Region::EuWest1,
        );
        let error = client
            .post::<_, ToolStart>("/internal/tools/start", "invalid", &serde_json::json!({}))
            .await
            .expect_err("closed listener makes dispatch ambiguous");
        assert_eq!(error.stage, DispatchStage::Dispatched);
        assert_eq!(error.proof, DispatchProof::PossiblySent);
        assert!(error.retryable);
    }
}
