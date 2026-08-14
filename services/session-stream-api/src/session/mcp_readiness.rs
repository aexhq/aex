//! Bounded MCP qualification between exact-generation setup and public 201.

use std::collections::BTreeMap;
use std::num::{NonZeroU64, NonZeroUsize};
use std::sync::Arc;
use std::time::Duration;

use aex_brain_domain::mcp::{FrozenMcpServer, FrozenMcpTransport};
use aex_brain_mcp::pool::{ConnectionPool, ServerRevision, TenantScope, screened_client};
use aex_hands_protocol::operation::{EnvValue, GuestPath, GuestRoot, SandboxMcpQualification};
use aex_hands_protocol::rpc::HandsOperationId;
use aex_wire::ids::{
    ContentHash, GenerationId, OrganizationId, PrefixedId as _, SessionId, WorkspaceId,
};
use aex_wire::types::Timestamp;
use sha2::{Digest as _, Sha256};

use super::provider_key::SessionMcpSecretReader;

const QUALIFICATION_TIMEOUT: Duration = Duration::from_secs(30);

/// Create-time qualification boundary. An error makes session publication
/// fail and the existing exact-generation compensation path terminate runtime.
#[async_trait::async_trait]
pub trait SessionMcpQualifier: Send + Sync + 'static {
    /// Qualifies every frozen server in request order under one bounded timeout
    /// per server.
    async fn qualify(
        &self,
        organization: OrganizationId,
        workspace: WorkspaceId,
        session: SessionId,
        generation: Option<GenerationId>,
        servers: &[FrozenMcpServer],
        now: Timestamp,
    ) -> Result<(), String>;
}

/// Production remote-rmcp and exact-generation sandbox qualifier.
pub struct ProductionMcpQualifier {
    secrets: Arc<dyn SessionMcpSecretReader>,
    live: Arc<dyn aex_brain_hands::LiveFileBackend>,
    pool: ConnectionPool,
}

impl ProductionMcpQualifier {
    /// Binds the same secret and Hands authorities used by session admission.
    #[must_use]
    pub fn new(
        secrets: Arc<dyn SessionMcpSecretReader>,
        live: Arc<dyn aex_brain_hands::LiveFileBackend>,
    ) -> Self {
        Self {
            secrets,
            live,
            pool: ConnectionPool::new(NonZeroUsize::new(64).expect("positive")),
        }
    }

    async fn reveal(
        &self,
        organization: OrganizationId,
        workspace: WorkspaceId,
        refs: &BTreeMap<String, aex_wire::ids::ResourceName>,
        now: Timestamp,
    ) -> Result<BTreeMap<String, zeroize::Zeroizing<String>>, String> {
        let mut values = BTreeMap::new();
        for (key, secret) in refs {
            let value = self
                .secrets
                .reveal_mcp_secret(organization, workspace, secret, now)
                .await
                .map_err(|_| "MCP transport secret is unavailable".to_owned())?;
            values.insert(key.clone(), value);
        }
        Ok(values)
    }

    async fn qualify_one(
        &self,
        organization: OrganizationId,
        workspace: WorkspaceId,
        session: SessionId,
        generation: Option<GenerationId>,
        server: &FrozenMcpServer,
        now: Timestamp,
    ) -> Result<(), String> {
        match &server.transport {
            FrozenMcpTransport::RemoteHttp { endpoint, headers } => {
                let headers = self.reveal(organization, workspace, headers, now).await?;
                let revision = ServerRevision {
                    server: server.name.clone(),
                    revision: NonZeroU64::new(1).expect("positive"),
                    secret_generation: NonZeroU64::new(1).expect("positive"),
                    manifest_digest: ContentHash::of(
                        &serde_json::to_vec(server)
                            .map_err(|_| "MCP config is not encodable".to_owned())?,
                    ),
                };
                let client = screened_client(
                    &self.pool,
                    &aex_brain_managed_web::egress::SystemDnsResolver,
                    TenantScope {
                        organization,
                        workspace,
                    },
                    revision,
                    endpoint,
                )
                .await
                .map_err(|_| "remote MCP endpoint failed egress screening".to_owned())?;
                aex_brain_mcp::execute::qualify_streamable_http(
                    client,
                    headers
                        .into_iter()
                        .map(|(name, value)| (name, value.to_string())),
                )
                .await
                .map_err(|_| "remote MCP qualification failed".to_owned())?;
                Ok(())
            }
            FrozenMcpTransport::SandboxProcess {
                command,
                args,
                environment,
                working_directory,
            } => {
                let generation = generation
                    .ok_or_else(|| "sandbox MCP requires an enabled sandbox".to_owned())?;
                let environment = self
                    .reveal(organization, workspace, environment, now)
                    .await?
                    .into_iter()
                    .map(|(name, value)| (name, EnvValue::new(value.to_string())))
                    .collect();
                let working_directory = GuestPath::parse(
                    &GuestRoot::workspace(),
                    working_directory.as_deref().unwrap_or("/workspace"),
                )
                .map_err(|_| "sandbox MCP working directory is invalid".to_owned())?;
                self.live
                    .qualify_sandbox_mcp(
                        session,
                        generation,
                        activity_id(generation, server),
                        &SandboxMcpQualification {
                            command: command.clone(),
                            args: args.clone(),
                            environment,
                            working_directory,
                        },
                    )
                    .await
                    .map_err(|_| "sandbox MCP qualification failed".to_owned())?;
                Ok(())
            }
        }
    }
}

#[async_trait::async_trait]
impl SessionMcpQualifier for ProductionMcpQualifier {
    async fn qualify(
        &self,
        organization: OrganizationId,
        workspace: WorkspaceId,
        session: SessionId,
        generation: Option<GenerationId>,
        servers: &[FrozenMcpServer],
        now: Timestamp,
    ) -> Result<(), String> {
        for server in servers {
            tracing::info!(
                target: "aex::session_telemetry",
                event = "sandbox_progress",
                progress = "qualifying_sandbox_mcp",
                session = %session,
                server = %server.name,
            );
            tokio::time::timeout(
                QUALIFICATION_TIMEOUT,
                self.qualify_one(organization, workspace, session, generation, server, now),
            )
            .await
            .map_err(|_| "MCP qualification timed out".to_owned())??;
        }
        Ok(())
    }
}

fn activity_id(generation: GenerationId, server: &FrozenMcpServer) -> HandsOperationId {
    let mut hasher = Sha256::new();
    hasher.update(b"aex.session-create.mcp-qualification.v1\0");
    hasher.update(generation.uuid7().as_bytes());
    hasher.update(server.name.as_str().as_bytes());
    let digest: [u8; 32] = hasher.finalize().into();
    let mut entropy = [0_u8; 10];
    entropy.copy_from_slice(&digest[..10]);
    HandsOperationId(aex_wire::ids::Uuid7::compose(
        generation.uuid7().unix_millis(),
        entropy,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    use aex_brain_app::ports::{BoxFuture, HandsError, ProviderFailureKind, RedactedDetail};
    use aex_brain_domain::effect::{DispatchProof, DispatchStage};
    use aex_brain_hands::{LiveFileReply, LiveGenerationReady};
    use aex_hands_protocol::files::FileRequest;

    struct FakeSecrets {
        value: String,
    }

    impl SessionMcpSecretReader for FakeSecrets {
        fn reveal_mcp_secret<'a>(
            &'a self,
            _organization: OrganizationId,
            _workspace: WorkspaceId,
            _name: &'a aex_secret_domain::SecretName,
            _now: Timestamp,
        ) -> BoxFuture<'a, Result<zeroize::Zeroizing<String>, aex_session_app::PortError>> {
            Box::pin(async move { Ok(zeroize::Zeroizing::new(self.value.clone())) })
        }
    }

    #[derive(Default)]
    struct FakeLive {
        qualification: Mutex<Option<(SessionId, GenerationId, SandboxMcpQualification)>>,
    }

    fn unused_live_error() -> HandsError {
        HandsError::Transport {
            stage: DispatchStage::PreDispatch,
            proof: DispatchProof::NotSent,
            detail: RedactedDetail::internal(
                ProviderFailureKind::ServerError,
                "unused fake live method",
            ),
        }
    }

    impl aex_brain_hands::LiveFileBackend for FakeLive {
        fn ensure_ready(
            &self,
            _session: SessionId,
            _generation: GenerationId,
        ) -> BoxFuture<'_, Result<LiveGenerationReady, HandsError>> {
            Box::pin(async { Err(unused_live_error()) })
        }

        fn qualify_sandbox_mcp<'a>(
            &'a self,
            session: SessionId,
            generation: GenerationId,
            _activity: HandsOperationId,
            request: &'a SandboxMcpQualification,
        ) -> BoxFuture<'a, Result<Vec<String>, HandsError>> {
            Box::pin(async move {
                self.qualification.lock().expect("qualification").replace((
                    session,
                    generation,
                    request.clone(),
                ));
                Ok(vec!["read".to_owned()])
            })
        }

        fn call<'a>(
            &'a self,
            _session: SessionId,
            _generation: GenerationId,
            _activity: HandsOperationId,
            _requests: &'a [FileRequest],
        ) -> BoxFuture<'a, Result<LiveFileReply, HandsError>> {
            Box::pin(async { Err(unused_live_error()) })
        }

        fn abort_unpublished(
            &self,
            _session: SessionId,
            _generation: GenerationId,
        ) -> BoxFuture<'_, Result<(), HandsError>> {
            Box::pin(async { Ok(()) })
        }
    }

    #[test]
    fn qualification_activity_is_stable_per_exact_generation_and_server() {
        let generation = GenerationId::from_uuid7(aex_wire::ids::Uuid7::compose(1, [1; 10]));
        let server = FrozenMcpServer {
            name: aex_wire::ids::ResourceName::parse("repo").expect("name"),
            transport: FrozenMcpTransport::RemoteHttp {
                endpoint: "https://example.com/mcp".to_owned(),
                headers: BTreeMap::new(),
            },
        };
        assert_eq!(
            activity_id(generation, &server),
            activity_id(generation, &server)
        );
    }

    #[tokio::test]
    async fn production_sandbox_qualification_reveals_only_at_the_exact_guest_handshake() {
        let live = Arc::new(FakeLive::default());
        let qualifier = ProductionMcpQualifier::new(
            Arc::new(FakeSecrets {
                value: "customer-token".to_owned(),
            }),
            live.clone(),
        );
        let organization = OrganizationId::from_uuid7(aex_wire::ids::Uuid7::compose(1, [1; 10]));
        let workspace = WorkspaceId::from_uuid7(aex_wire::ids::Uuid7::compose(2, [2; 10]));
        let session = SessionId::from_uuid7(aex_wire::ids::Uuid7::compose(3, [3; 10]));
        let generation = GenerationId::from_uuid7(aex_wire::ids::Uuid7::compose(4, [4; 10]));
        let secret = aex_wire::ids::ResourceName::parse("mcp-secret").expect("secret name");
        qualifier
            .qualify(
                organization,
                workspace,
                session,
                Some(generation),
                &[FrozenMcpServer {
                    name: aex_wire::ids::ResourceName::parse("repo").expect("server name"),
                    transport: FrozenMcpTransport::SandboxProcess {
                        command: "server".to_owned(),
                        args: vec!["--stdio".to_owned()],
                        environment: BTreeMap::from([("TOKEN".to_owned(), secret)]),
                        working_directory: Some("/workspace/project".to_owned()),
                    },
                }],
                Timestamp::from_unix_millis(1).expect("timestamp"),
            )
            .await
            .expect("qualification succeeds");

        let (found_session, found_generation, request) = live
            .qualification
            .lock()
            .expect("qualification")
            .clone()
            .expect("the guest handshake ran");
        assert_eq!(found_session, session);
        assert_eq!(found_generation, generation);
        assert_eq!(request.command, "server");
        assert_eq!(request.environment["TOKEN"].expose(), "customer-token");
        assert_eq!(request.working_directory.as_str(), "/workspace/project");
    }

    #[tokio::test]
    async fn sandbox_process_is_refused_when_session_has_no_generation() {
        let qualifier = ProductionMcpQualifier::new(
            Arc::new(FakeSecrets {
                value: String::new(),
            }),
            Arc::new(FakeLive::default()),
        );
        let server = FrozenMcpServer {
            name: aex_wire::ids::ResourceName::parse("repo").expect("server name"),
            transport: FrozenMcpTransport::SandboxProcess {
                command: "server".to_owned(),
                args: Vec::new(),
                environment: BTreeMap::new(),
                working_directory: None,
            },
        };
        let result = qualifier
            .qualify(
                OrganizationId::from_uuid7(aex_wire::ids::Uuid7::compose(1, [1; 10])),
                WorkspaceId::from_uuid7(aex_wire::ids::Uuid7::compose(2, [2; 10])),
                SessionId::from_uuid7(aex_wire::ids::Uuid7::compose(3, [3; 10])),
                None,
                &[server],
                Timestamp::from_unix_millis(1).expect("timestamp"),
            )
            .await;
        assert_eq!(
            result.expect_err("sandbox MCP without a generation is refused"),
            "sandbox MCP requires an enabled sandbox"
        );
    }
}
