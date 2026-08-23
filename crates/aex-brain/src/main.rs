//! Aex's downstream Brain composition.
//!
//! Brain owns the engine and protocols; this binary composes Aex's selected extensions and outer
//! configuration. Production is the default; `BRAIN_MODE=local` explicitly selects durable local
//! adapters and unsafe host execution.

mod customer_delivery;

use std::collections::HashMap;
use std::sync::Arc;

use aws_microvm_controller::AwsMicrovmEnvironment;
use brain::adapter::ToolExecutor;
use brain::config::ServerToolPolicy;
use brain::customer::CustomerTransportConfig;
use brain::environment::{EnvironmentAdapter, EnvironmentRegistry};
use brain::session::{Brain, BrainConfig, BrainServices, ProviderFactory};
use brain_aws::{AwsPersistenceConfig, AwsRuntimePorts};
use brain_protocol::session::{ExternalToolCompletion, ExternalToolEffect, ExternalToolScope};
use brain_providers::external::HttpExternalToolExecutor;
use brain_server::api::{AppState, Tenancy, serve};
use brain_standalone::durable_local_parts;

const CAPABILITIES: [&str; 4] = [
    "brain.output",
    "brain.web.search",
    "brain.web.fetch",
    "brain.subagents",
];

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| {
                "brain=info,brain_aws=info,aws_microvm_controller=info,aex_brain=info".into()
            }),
        )
        .init();

    let mode = std::env::var("BRAIN_MODE").unwrap_or_else(|_| "production".into());
    if !matches!(mode.as_str(), "production" | "local") {
        anyhow::bail!("unsupported BRAIN_MODE={mode}; use production or local");
    }
    let token = required("AEX_BRAIN_TOKEN")?;
    let address = std::env::var("AEX_BRAIN_LISTEN")
        .unwrap_or_else(|_| "127.0.0.1:8700".into())
        .parse()?;
    let customer_transport = configured_customer_transport(&mode, address)?;
    let (config, external) = configured_runtime()?;
    let brain = match mode.as_str() {
        "production" => compose_production(config, external, customer_transport).await?,
        "local" => {
            let data_dir =
                std::env::var("BRAIN_DATA_DIR").unwrap_or_else(|_| "./brain-data".into());
            let brain = compose_local(config, external, customer_transport, data_dir, None, None)?;
            tracing::warn!(
                capabilities = ?CAPABILITIES,
                "LOCAL MODE: durable SQLite/custody/storage with unsandboxed host Tool execution; session network policy is not enforced"
            );
            brain
        }
        _ => unreachable!("BRAIN_MODE was validated above"),
    };
    // The Aex control plane fronts every request and always stamps x-brain-tenant-id;
    // hosted mode refuses a header-less request rather than booking tenant "local".
    let tenancy = if mode == "production" {
        Tenancy::Required
    } else {
        Tenancy::Implicit("local".into())
    };
    serve(
        AppState {
            brain,
            token,
            tenancy,
        },
        address,
    )
    .await
}

/// Build the exact Aex policy and its private control-plane executor in every runtime mode.
/// Generic `BRAIN_EXTERNAL_TOOL_*` variables cannot widen or redirect this capability set.
fn configured_runtime() -> anyhow::Result<(BrainConfig, Arc<dyn ToolExecutor>)> {
    let executor_token = required("AEX_EXTERNAL_TOOL_EXECUTOR_TOKEN")?;
    let executor_url = std::env::var("AEX_CONTROL_INTERNAL_URL")
        .unwrap_or_else(|_| "http://127.0.0.1:8601/internal/v1/tools/call".into());
    validate_executor_url(&executor_url)?;
    // Load and validate host process policy before sealing Aex's fixed capability registry.
    // In particular, malformed retention ceilings must fail startup instead of silently falling
    // back to defaults and exposing unbounded DynamoDB retention.
    let mut config = BrainConfig::from_env().map_err(anyhow::Error::msg)?;
    config.external_executor_url = None;
    config.external_executor_token = None;
    config.external_executor_capabilities.clear();
    config.official_capabilities = official_capabilities();
    let external: Arc<dyn ToolExecutor> = Arc::new(
        HttpExternalToolExecutor::new(
            executor_url,
            Some(executor_token),
            config.external_call_timeout,
            CAPABILITIES.map(str::to_owned),
        )
        .map_err(anyhow::Error::msg)?,
    );
    Ok((config, external))
}

async fn compose_production(
    config: BrainConfig,
    external: Arc<dyn ToolExecutor>,
    customer_transport: CustomerTransportConfig,
) -> anyhow::Result<Arc<Brain>> {
    let persistence = AwsPersistenceConfig::from_env().map_err(anyhow::Error::msg)?;
    validate_production_region(&persistence.region)?;
    let environment = AwsMicrovmEnvironment::from_env().await?;
    let adapter = EnvironmentAdapter {
        execution: environment.clone(),
        preparation: environment.clone(),
        files: Some(environment.clone()),
    };
    let environments = EnvironmentRegistry::new([("@aexhq/env-aws-microvm".to_owned(), adapter)])?;
    let customer_delivery =
        Arc::new(customer_delivery::ApiGatewayCustomerDelivery::from_env().await?);
    let loop_registry = configured_loop_registry(required("BRAIN_LOOP_STORE_DIR")?.into())?;
    let brain = brain_aws::compose(
        config,
        persistence,
        AwsRuntimePorts {
            environments,
            external_executor: Some(external),
            customer_delivery: Some(customer_delivery),
            customer_transport: Some(customer_transport),
            agentloop_registry: Some(loop_registry),
            // The hosted default: guarded live transport with private addresses denied.
            provider_factory: None,
        },
    )
    .await
    .map_err(anyhow::Error::msg)?;
    environment
        .attach_secret_delivery(brain.clone())
        .map_err(|error| {
            anyhow::anyhow!(
                "AWS MicroVM Environment secret delivery: {}",
                error.message.as_str()
            )
        })?;
    tracing::info!(capabilities = ?CAPABILITIES, "Aex hosted Brain composition ready");
    Ok(brain)
}

fn validate_production_region(region: &str) -> anyhow::Result<()> {
    if region != "us-east-1" {
        anyhow::bail!("Aex MVP production is pinned to AWS_REGION=us-east-1; got {region}");
    }
    Ok(())
}

fn compose_local(
    config: BrainConfig,
    external: Arc<dyn ToolExecutor>,
    customer_transport: CustomerTransportConfig,
    data_dir: impl Into<std::path::PathBuf>,
    provider_factory: Option<ProviderFactory>,
    agentloop_registry: Option<Arc<dyn brain::agentloop::AgentloopRegistry>>,
) -> anyhow::Result<Arc<Brain>> {
    let allow_private = config.outbound_allow_private;
    let data_dir = data_dir.into();
    let parts = durable_local_parts(&data_dir).map_err(anyhow::Error::msg)?;
    let local_environment = parts.local_environment.clone();
    let loop_registry = match agentloop_registry {
        Some(registry) => registry,
        None => configured_loop_registry(data_dir.join("loops"))?,
    };
    let environments = EnvironmentRegistry::new([(
        "brain.local".to_owned(),
        EnvironmentAdapter {
            execution: parts.environment.clone(),
            preparation: parts.session_preparation.clone(),
            files: Some(parts.sandbox_files.clone()),
        },
    )])?;
    let brain = Brain::with_parts_and_services(
        config,
        parts.journal,
        parts.custody,
        external,
        BrainServices {
            session_storage: Some(parts.session_storage),
            bundle_storage: Some(parts.bundle_storage),
            environments,
            customer_delivery: None,
            customer_transport: Some(customer_transport),
            compactor: None,
            agentloop_registry: Some(loop_registry),
        },
        provider_factory.unwrap_or_else(|| brain_providers::default_factory(allow_private)),
    );
    local_environment
        .attach_secret_delivery(brain.clone())
        .map_err(|error| {
            anyhow::anyhow!(
                "local Environment secret delivery: {}",
                error.message.as_str()
            )
        })?;
    Ok(brain)
}

fn configured_loop_registry(
    store_dir: std::path::PathBuf,
) -> anyhow::Result<Arc<dyn brain::agentloop::AgentloopRegistry>> {
    let toolchain_dir = required("BRAIN_LOOPHOST_TOOLCHAIN_DIR")?;
    Ok(Arc::new(brain_loophost::registry::LoophostRegistry::new(
        store_dir,
        toolchain_dir,
    )?))
}

fn configured_customer_transport(
    mode: &str,
    listen: std::net::SocketAddr,
) -> anyhow::Result<CustomerTransportConfig> {
    let local_origin = || {
        let host = if listen.is_ipv6() {
            format!("[::1]:{}", listen.port())
        } else {
            format!("127.0.0.1:{}", listen.port())
        };
        (
            format!("ws://{host}/v1/customer-environment/socket"),
            format!("http://{host}"),
        )
    };
    let (websocket_default, observation_default) = local_origin();
    let websocket_url = match std::env::var("AEX_CUSTOMER_ENVIRONMENT_WEBSOCKET_URL") {
        Ok(value) => value,
        Err(_) if mode == "local" => websocket_default,
        Err(_) => anyhow::bail!("AEX_CUSTOMER_ENVIRONMENT_WEBSOCKET_URL is not set"),
    };
    let observation_base_url = match std::env::var("AEX_CUSTOMER_ENVIRONMENT_OBSERVATION_BASE_URL")
    {
        Ok(value) => value,
        Err(_) if mode == "local" => observation_default,
        Err(_) => anyhow::bail!("AEX_CUSTOMER_ENVIRONMENT_OBSERVATION_BASE_URL is not set"),
    };
    CustomerTransportConfig::new(websocket_url, observation_base_url).map_err(anyhow::Error::msg)
}

fn official_capabilities() -> HashMap<String, ServerToolPolicy> {
    [
        (
            "brain.output",
            ServerToolPolicy {
                capability: "brain.output".into(),
                scope: ExternalToolScope::Root,
                completion: ExternalToolCompletion::ReturnDirect,
                effect: ExternalToolEffect::ReplaySafe,
                max_input_bytes: brain_protocol::MAX_EXTERNAL_TOOL_INPUT_BYTES,
            },
        ),
        (
            "brain.web.search",
            ServerToolPolicy {
                capability: "brain.web.search".into(),
                scope: ExternalToolScope::All,
                completion: ExternalToolCompletion::Continue,
                effect: ExternalToolEffect::ReplaySafe,
                max_input_bytes: 8 * 1024,
            },
        ),
        (
            "brain.web.fetch",
            ServerToolPolicy {
                capability: "brain.web.fetch".into(),
                scope: ExternalToolScope::All,
                completion: ExternalToolCompletion::Continue,
                effect: ExternalToolEffect::ReplaySafe,
                max_input_bytes: 8 * 1024,
            },
        ),
        (
            // The former brain.subagents intrinsic: the same child-session verbs, spoken over
            // Brain's session-scoped children API by ordinary Aex service code.
            "brain.subagents",
            ServerToolPolicy {
                capability: "brain.subagents".into(),
                scope: ExternalToolScope::All,
                completion: ExternalToolCompletion::Continue,
                effect: ExternalToolEffect::ReplaySafe,
                max_input_bytes: brain_protocol::MAX_EXTERNAL_TOOL_INPUT_BYTES,
            },
        ),
    ]
    .into_iter()
    .map(|(capability, policy)| (capability.to_owned(), policy))
    .collect()
}

fn required(name: &str) -> anyhow::Result<String> {
    let value = std::env::var(name).map_err(|_| anyhow::anyhow!("{name} is not set"))?;
    if value.trim().is_empty() {
        anyhow::bail!("{name} cannot be empty");
    }
    Ok(value)
}

fn validate_executor_url(value: &str) -> anyhow::Result<()> {
    let url = reqwest::Url::parse(value)
        .map_err(|error| anyhow::anyhow!("AEX_CONTROL_INTERNAL_URL: {error}"))?;
    if !matches!(url.scheme(), "http" | "https") {
        anyhow::bail!("AEX_CONTROL_INTERNAL_URL must use HTTP or HTTPS");
    }
    if !url.username().is_empty() || url.password().is_some() {
        anyhow::bail!("AEX_CONTROL_INTERNAL_URL must not contain credentials");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::path::PathBuf;
    use std::process::Stdio;
    use std::sync::Mutex;
    use std::time::{Duration, Instant};

    use base64::Engine;
    use brain::Result;
    use brain::agentloop::{Agentloop, AgentloopRegistry, SequentialAgentloop};
    use brain::journal::AgentloopSelectorDoc;
    use brain::journal::Record;
    use brain::provider::fake::{FakeProvider, Scripted};
    use brain_protocol::session::{
        CreateSessionRequest, ExternalToolCallRequest, ExternalToolCallResponse,
        MessageRequestContent, MessageRequestContentString,
    };
    use serde_json::json;
    use sha2::{Digest, Sha256};
    use tokio::io::{AsyncBufReadExt, BufReader};
    use tokio_util::sync::CancellationToken;

    struct TestLoopRegistry;

    impl AgentloopRegistry for TestLoopRegistry {
        fn resolve(&self, _selector: &AgentloopSelectorDoc) -> Result<Arc<dyn Agentloop>> {
            Ok(Arc::new(SequentialAgentloop))
        }

        fn admit_custom(
            &self,
            source_bundle_sha256: &str,
            toolchain: &str,
            bundle: &[u8],
        ) -> Result<AgentloopSelectorDoc> {
            Ok(AgentloopSelectorDoc {
                source_bundle_sha256: source_bundle_sha256.into(),
                source_bundle_bytes: bundle.len() as u64,
                toolchain: toolchain.into(),
            })
        }
    }

    fn test_loop_registry() -> Arc<dyn AgentloopRegistry> {
        Arc::new(TestLoopRegistry)
    }

    fn test_loop() -> serde_json::Value {
        let bundle = b"integration test loop";
        json!({
            "source_bundle_sha256": hex::encode(Sha256::digest(bundle)),
            "toolchain": "test-loop",
            "bundle_base64": base64::engine::general_purpose::STANDARD.encode(bundle),
        })
    }

    const CUSTOMER_RUNNER: &str = r#"
const base = process.env.SMOKE_BASE_URL;
const token = process.env.SMOKE_BRAIN_TOKEN;
const tenant = process.env.SMOKE_TENANT_ID;
const clientId = process.env.SMOKE_CLIENT_ID;
const registration = process.env.SMOKE_REGISTRATION;
const toolName = process.env.SMOKE_TOOL_NAME;
const contractDigest = process.env.SMOKE_CONTRACT_DIGEST;
const grantResponse = await fetch(`${base}/internal/v1/customer-environment/grants`, {
  method: 'POST',
  headers: {
    authorization: `Bearer ${token}`,
    'content-type': 'application/json',
    'x-brain-tenant-id': tenant,
  },
  body: JSON.stringify({ client_id: clientId }),
});
if (!grantResponse.ok) throw new Error(`grant failed: ${grantResponse.status} ${await grantResponse.text()}`);
const grant = await grantResponse.json();
const proofBytes = await crypto.subtle.digest(
  'SHA-256',
  new TextEncoder().encode(`brain.customer-environment.frame-proof\0${grant.protocol}`),
);
const proof = [...new Uint8Array(proofBytes)]
  .map((byte) => byte.toString(16).padStart(2, '0'))
  .join('');
const socket = new WebSocket(grant.url, grant.protocol);
const observe = async (body) => {
  const response = await fetch(grant.observation_url, {
    method: 'POST',
    headers: { authorization: `Bearer ${grant.observation_token}`, 'content-type': 'application/json' },
    body: JSON.stringify(body),
  });
  if (!response.ok) throw new Error(`observation failed: ${response.status} ${await response.text()}`);
};
socket.addEventListener('open', () => socket.send(JSON.stringify({
  type: 'register', client_id: clientId, process_id: 'aex-local-smoke-node', proof,
})));
socket.addEventListener('message', (event) => {
  void (async () => {
    const frame = JSON.parse(String(event.data));
    if (frame.type === 'ready') {
      socket.send(JSON.stringify({
        type: 'register_tools', epoch: frame.epoch, batch_id: 'smoke-registration',
        proof,
        registrations: [{ registration, name: toolName, contract_digest: contractDigest }],
      }));
    } else if (frame.type === 'registered' && frame.batch_id === 'smoke-registration') {
      process.stdout.write('READY\n');
    } else if (frame.type === 'offer') {
      await observe({
        type: 'receipt', epoch: frame.epoch, operation_id: frame.operation_id,
        request_digest: frame.request_digest, replayed: false,
      });
      const output = { client_value: frame.input.value * 3, runtime: 'node-app' };
      await observe({
        type: 'terminal', epoch: frame.epoch, operation_id: frame.operation_id,
        request_digest: frame.request_digest, ok: true, output,
      });
    }
  })().catch((error) => { process.stderr.write(`${error.stack ?? error}\n`); process.exitCode = 1; });
});
"#;

    struct TempDir(PathBuf);

    impl TempDir {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "aex-brain-local-smoke-{}-{}",
                std::process::id(),
                brain::wall_ms()
            ));
            std::fs::create_dir_all(&path).expect("create temp data dir");
            Self(path)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[derive(Default)]
    struct OutputExecutor {
        requests: Mutex<Vec<ExternalToolCallRequest>>,
    }

    #[async_trait::async_trait]
    impl ToolExecutor for OutputExecutor {
        fn supports(&self, capability: &str) -> bool {
            CAPABILITIES.contains(&capability)
        }

        async fn call(
            &self,
            capability: &str,
            request: ExternalToolCallRequest,
            _cancel: CancellationToken,
        ) -> Result<ExternalToolCallResponse> {
            assert_eq!(capability, "brain.output");
            let result = request.input.clone();
            self.requests.lock().expect("requests").push(request);
            Ok(serde_json::from_value(json!({
                "outcome": "completed",
                "content": "accepted",
                "is_error": false,
                "disposition": "complete_turn",
                "result": result
            }))?)
        }
    }

    #[test]
    fn hosted_capability_set_is_exact_and_stable() {
        assert_eq!(
            CAPABILITIES,
            [
                "brain.output",
                "brain.web.search",
                "brain.web.fetch",
                "brain.subagents"
            ]
        );
        let policies = official_capabilities();
        assert_eq!(policies.len(), CAPABILITIES.len());
        assert_eq!(
            policies["brain.output"].completion,
            ExternalToolCompletion::ReturnDirect
        );
        assert_eq!(policies["brain.output"].scope, ExternalToolScope::Root);
        assert!(
            CAPABILITIES
                .iter()
                .all(|capability| policies.contains_key(*capability))
        );
        assert!(validate_executor_url("http://127.0.0.1:8601/internal/v1/tools/call").is_ok());
        assert!(validate_executor_url("file:///tmp/executor").is_err());
        assert!(validate_executor_url("http://user:secret@localhost/call").is_err());
        assert!(validate_production_region("us-east-1").is_ok());
        assert!(validate_production_region("eu-west-1").is_err());
    }

    #[tokio::test]
    async fn aex_local_composition_creates_stores_and_returns_output() {
        let temp = TempDir::new();
        let fake = Arc::new(FakeProvider::new(brain::config::Dialect::AnthropicMessages));
        fake.script([Scripted::tool(
            "output",
            json!({"summary": "local composition works"}),
        )]);
        let provider = fake.clone();
        let provider_factory: ProviderFactory =
            Arc::new(move |_| provider.clone() as Arc<dyn brain::provider::Provider>);
        let executor = Arc::new(OutputExecutor::default());
        let config = BrainConfig {
            official_capabilities: official_capabilities(),
            external_executor_url: None,
            external_executor_token: None,
            external_executor_capabilities: Default::default(),
            ..BrainConfig::default()
        };
        let brain = compose_local(
            config,
            executor.clone(),
            CustomerTransportConfig::new(
                "ws://127.0.0.1:8700/v1/customer-environment/socket",
                "http://127.0.0.1:8700",
            )
            .expect("local customer transport"),
            temp.0.clone(),
            Some(provider_factory),
            Some(test_loop_registry()),
        )
        .expect("compose local Aex Brain");
        let (output_definition, _) = seal_definition(json!({
            "name": "output",
            "description": "Return the final structured result.",
            "input_schema": {
                "type": "object",
                "additionalProperties": true
            },
            "output_schema": {
                "type": "object",
                "additionalProperties": true
            }
        }));
        let request: CreateSessionRequest = serde_json::from_value(json!({
            "model": {
                "provider": "anthropic",
                "name": "scripted",
                "api_key": "sk-fake"
            },
            "agentloop": test_loop(),
            "tools": {"items": [{
                "definition": output_definition,
                "executor": {"kind": "engine", "capability": "brain.output"}
            }]}
        }))
        .expect("valid create request");
        let session = brain
            .create_session(request, Some("local-smoke"))
            .await
            .expect("create local session");
        let session_id = session.id.to_string();

        let stored = brain
            .storage_write_inline(
                &session_id,
                "notes/smoke.txt".into(),
                "bG9jYWwtc3RvcmFnZQ==".into(),
                Some("text/plain".into()),
                false,
            )
            .await
            .expect("write durable local storage");
        assert_eq!(stored.bytes, 13);
        let (_, bytes) = brain
            .storage_read_inline(&session_id, "notes/smoke.txt", 1024)
            .await
            .expect("read durable local storage");
        assert_eq!(bytes, b"local-storage");

        let content = MessageRequestContent::from(
            MessageRequestContentString::try_from("return the result").expect("message"),
        );
        brain
            .message(&session_id, content)
            .await
            .expect("admit local turn");
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let records = brain
                .journal
                .read_records(&session_id, 0)
                .await
                .expect("read local journal");
            if records
                .iter()
                .any(|entry| matches!(entry.record, Record::TurnCompleted { .. }))
            {
                break;
            }
            assert!(Instant::now() < deadline, "local output turn timed out");
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        {
            let requests = executor.requests.lock().expect("requests");
            assert_eq!(requests.len(), 1);
            assert_eq!(requests[0].name.as_str(), "output");
            assert_eq!(
                requests[0].input,
                json!({"summary": "local composition works"})
            );
        }
        fake.assert_drained(1, "one Aex output round")
            .expect("fake provider drained");

        // Reopening the exact Aex local composition sees the same durable object.
        drop(brain);
        let reopened = compose_local(
            BrainConfig {
                official_capabilities: official_capabilities(),
                ..BrainConfig::default()
            },
            executor,
            CustomerTransportConfig::new(
                "ws://127.0.0.1:8700/v1/customer-environment/socket",
                "http://127.0.0.1:8700",
            )
            .expect("local customer transport"),
            temp.0.clone(),
            None,
            Some(test_loop_registry()),
        )
        .expect("reopen local Aex Brain");
        let (_, bytes) = reopened
            .storage_read_inline(&session_id, "notes/smoke.txt", 1024)
            .await
            .expect("read storage after restart");
        assert_eq!(bytes, b"local-storage");
    }

    fn seal_definition(mut definition: serde_json::Value) -> (serde_json::Value, String) {
        let digest = hex::encode(Sha256::digest(
            serde_jcs::to_vec(&definition).expect("canonical Tool definition"),
        ));
        definition
            .as_object_mut()
            .expect("Tool definition object")
            .insert("contract_digest".into(), digest.clone().into());
        (definition, digest)
    }

    fn sealed_definition(name: &str, description: &str) -> (serde_json::Value, String) {
        seal_definition(json!({
            "name": name,
            "description": description,
            "input_schema": {
                "type": "object",
                "properties": {"value": {"type": "integer"}},
                "required": ["value"],
                "additionalProperties": false
            },
            "output_schema": {
                "type": "object",
                "additionalProperties": true
            }
        }))
    }

    async fn post_create(
        http: &reqwest::Client,
        base: &str,
        token: &str,
        tenant: &str,
        body: &serde_json::Value,
    ) -> reqwest::Response {
        http.post(format!("{base}/v1/sessions"))
            .bearer_auth(token)
            .header("x-brain-tenant-id", tenant)
            .json(body)
            .send()
            .await
            .expect("create request")
    }

    #[tokio::test]
    async fn aex_local_composition_runs_server_and_connected_node_client_tools() {
        if std::process::Command::new("node")
            .arg("--version")
            .output()
            .is_err()
        {
            panic!("Aex local mode requires Node 22 or later");
        }

        let temp = TempDir::new();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind local smoke server");
        let address = listener.local_addr().expect("local smoke address");
        let base = format!("http://{address}");
        let token = "local-smoke-operator";
        let tenant = "local-smoke-tenant";
        let client_id = "local-smoke-client";
        let registration = "local-smoke-client-tool";

        let fake = Arc::new(FakeProvider::new(brain::config::Dialect::AnthropicMessages));
        fake.script([
            Scripted::tool("server_tool", json!({"value": 2})),
            Scripted::tool("client_tool", json!({"value": 3})),
            Scripted::tool("output", json!({"summary": "both local realms ran"})),
        ]);
        let provider = fake.clone();
        let provider_factory: ProviderFactory =
            Arc::new(move |_| provider.clone() as Arc<dyn brain::provider::Provider>);
        let executor = Arc::new(OutputExecutor::default());
        let customer_transport = CustomerTransportConfig::new(
            format!("ws://{address}/v1/customer-environment/socket"),
            base.clone(),
        )
        .expect("local customer transport");
        let brain = compose_local(
            BrainConfig {
                official_capabilities: official_capabilities(),
                ..BrainConfig::default()
            },
            executor.clone(),
            customer_transport,
            temp.0.clone(),
            Some(provider_factory),
            Some(test_loop_registry()),
        )
        .expect("compose local Aex Brain");
        let server_brain = brain.clone();
        let server = tokio::spawn(async move {
            axum::serve(
                listener,
                brain_server::api::router(AppState {
                    brain: server_brain,
                    token: token.into(),
                    tenancy: Tenancy::Implicit("local".into()),
                }),
            )
            .await
        });

        let (server_definition, server_contract) =
            sealed_definition("server_tool", "Run inside the explicit local host Hand.");
        let (client_definition, client_contract) =
            sealed_definition("client_tool", "Run inside the connected Node application.");
        let server_bundle = format!(
            r#"import {{ writeFile }} from 'node:fs/promises';
export default Object.freeze({{
  kind: 'tool-runtime/v1',
  name: 'server_tool',
  description: 'Run inside the explicit local host Hand.',
  contractDigest: '{server_contract}',
  requiredEnv: [],
  execute: async (input, context) => {{
    await writeFile(`${{context.workspace}}/server-smoke.txt`, String(input.value));
    return {{ server_value: input.value * 2, runtime: 'local-host-environment' }};
  }},
}});
"#
        );
        let server_layer_digest = hex::encode(Sha256::digest(server_bundle.as_bytes()));
        let layer_reference = json!({
            "checksum": server_layer_digest,
            "bytes": server_bundle.len(),
            "media_type": "application/javascript+esm",
            "mount_path": "/tool/runtime.mjs",
            "unpack": "file"
        });
        let manifest_identity = json!({
            "profile": "computer/v1",
            "target": "linux-amd64",
            "execute_path": "/tool/runtime.mjs",
            "setup_path": null,
            "layers": [layer_reference.clone()]
        });
        let server_artifact_digest = hex::encode(Sha256::digest(
            serde_jcs::to_vec(&manifest_identity).expect("canonical artifact manifest"),
        ));
        let layer_document = json!({
            "checksum": server_layer_digest,
            "content_base64": base64::engine::general_purpose::STANDARD
                .encode(server_bundle.as_bytes()),
            "bytes": server_bundle.len(),
            "media_type": "application/javascript+esm"
        });
        let bundle_document = json!({
            "checksum": server_artifact_digest,
            "bytes": server_bundle.len(),
            "target": "linux-amd64",
            "execute_path": "/tool/runtime.mjs",
            "layers": [layer_reference]
        });
        let (output_definition, _) = seal_definition(json!({
            "name": "output",
            "description": "Finish the local composition smoke.",
            "input_schema": {"type": "object", "additionalProperties": true},
            "output_schema": {"type": "object", "additionalProperties": true}
        }));
        let http = reqwest::Client::new();

        // A supplied but undeclared bundle and a declared checksum that does not match its bytes
        // both fail before a session or customer code can be committed.
        let undeclared = post_create(
            &http,
            &base,
            token,
            tenant,
            &json!({
                "model": {"provider":"anthropic", "name":"scripted", "api_key":"sk-fake"},
                "agentloop": test_loop(),
                "tool_artifact_layers": [layer_document.clone()]
            }),
        )
        .await;
        assert_eq!(undeclared.status(), reqwest::StatusCode::BAD_REQUEST);
        let undeclared_error: serde_json::Value =
            undeclared.json().await.expect("undeclared error");
        assert_eq!(undeclared_error["error"]["code"], "invalid_request");
        let mismatched_digest = "b".repeat(64);
        let mismatch = post_create(
            &http,
            &base,
            token,
            tenant,
            &json!({
                "model": {"provider":"anthropic", "name":"scripted", "api_key":"sk-fake"},
                "agentloop": test_loop(),
                "environments": {"workspace": {
                    "extension": "brain.local",
                    "protocol": "environment/v1",
                    "profile": {"kind":"computer", "platform":"linux-amd64",
                                "network":"none", "recovery":"retained"},
                    "configuration": {}
                }},
                "tools": {"items": [{
                    "definition": server_definition.clone(),
                    "executor": {"kind":"environment", "environment":"workspace",
                                 "artifact_digest":mismatched_digest, "requirements":{}}
                }]},
                "tool_bundles": [{
                    "checksum": mismatched_digest,
                    "bytes": server_bundle.len(),
                    "target": "linux-amd64",
                    "execute_path": "/tool/runtime.mjs",
                    "layers": bundle_document["layers"]
                }],
                "tool_artifact_layers": [layer_document.clone()]
            }),
        )
        .await;
        assert_eq!(mismatch.status(), reqwest::StatusCode::BAD_REQUEST);
        let mismatch_error: serde_json::Value = mismatch.json().await.expect("mismatch error");
        assert_eq!(mismatch_error["error"]["code"], "invalid_request");

        let mut node = tokio::process::Command::new("node")
            .arg("--input-type=module")
            .arg("-e")
            .arg(CUSTOMER_RUNNER)
            .env("SMOKE_BASE_URL", &base)
            .env("SMOKE_BRAIN_TOKEN", token)
            .env("SMOKE_TENANT_ID", tenant)
            .env("SMOKE_CLIENT_ID", client_id)
            .env("SMOKE_REGISTRATION", registration)
            .env("SMOKE_TOOL_NAME", "client_tool")
            .env("SMOKE_CONTRACT_DIGEST", &client_contract)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .kill_on_drop(true)
            .spawn()
            .expect("start connected Node application");
        let stdout = node.stdout.take().expect("Node stdout");
        let mut lines = BufReader::new(stdout).lines();
        let ready = tokio::time::timeout(Duration::from_secs(10), lines.next_line())
            .await
            .expect("Node customer Hand readiness timeout")
            .expect("read Node readiness");
        assert_eq!(ready.as_deref(), Some("READY"));

        let create = post_create(
            &http,
            &base,
            token,
            tenant,
            &json!({
                "model": {"provider":"anthropic", "name":"scripted", "api_key":"sk-fake"},
                "agentloop": test_loop(),
                "client": {"id": client_id, "submit_retries": 1},
                "environments": {
                    "workspace": {
                        "extension": "brain.local",
                        "protocol": "environment/v1",
                        "profile": {"kind":"computer", "platform":"linux-amd64",
                                    "network":"none", "recovery":"retained"},
                        "configuration": {}
                    },
                    "app": {
                        "extension": "test.app",
                        "protocol": "environment/v1",
                        "profile": {"kind":"callbacks", "network":"unrestricted",
                                    "recovery":"connection"},
                        "configuration": {"id": client_id}
                    }
                },
                "tools": {"items": [
                    {
                        "definition": server_definition,
                        "executor": {"kind":"environment", "environment":"workspace",
                                     "artifact_digest":server_artifact_digest,
                                     "requirements":{}}
                    },
                    {
                        "definition": client_definition,
                        "executor": {"kind":"environment", "environment":"app",
                                     "callback_registration":registration,
                                     "requirements":{}}
                    },
                    {
                        "definition": output_definition,
                        "executor": {"kind":"engine", "capability":"brain.output"}
                    }
                ]},
                "tool_bundles": [bundle_document],
                "tool_artifact_layers": [layer_document]
            }),
        )
        .await;
        assert_eq!(create.status(), reqwest::StatusCode::CREATED);
        let session: serde_json::Value = create.json().await.expect("created session body");
        let session_id = session["id"].as_str().expect("created session id");
        let accepted = http
            .post(format!("{base}/v1/sessions/{session_id}/messages"))
            .bearer_auth(token)
            .header("x-brain-tenant-id", tenant)
            .json(&json!({"content":"run both local Tool realms"}))
            .send()
            .await
            .expect("local smoke message");
        assert_eq!(accepted.status(), reqwest::StatusCode::ACCEPTED);

        let deadline = Instant::now() + Duration::from_secs(20);
        let records = loop {
            let records = brain
                .journal
                .read_records(session_id, 0)
                .await
                .expect("read local realm journal");
            if records
                .iter()
                .any(|entry| matches!(entry.record, Record::TurnCompleted { .. }))
            {
                break records;
            }
            assert!(Instant::now() < deadline, "local Tool realm turn timed out");
            tokio::time::sleep(Duration::from_millis(25)).await;
        };
        let results: Vec<_> = records
            .iter()
            .filter_map(|entry| match &entry.record {
                Record::ToolResult {
                    name,
                    content,
                    is_error,
                    ..
                } => Some((name.as_str(), content.as_str(), *is_error)),
                _ => None,
            })
            .collect();
        assert!(results.iter().any(|(name, content, is_error)| {
            *name == "server_tool" && !is_error && content.contains("local-host-environment")
        }));
        assert!(results.iter().any(|(name, content, is_error)| {
            *name == "client_tool" && !is_error && content.contains("node-app")
        }));
        let environment = brain
            .environment_status(session_id, "workspace")
            .await
            .expect("read local workspace environment status");
        let generation = environment
            .generation
            .as_ref()
            .expect("local workspace environment generation");
        let exported = brain
            .sandbox_file_stat(
                session_id,
                "workspace",
                generation.as_str(),
                "/workspace/server-smoke.txt",
            )
            .await
            .expect("read server Tool workspace output through the public files port");
        assert_eq!(exported.bytes, 1);
        assert_eq!(executor.requests.lock().expect("output requests").len(), 1);
        fake.assert_drained(3, "server, client, and output rounds")
            .expect("fake provider drained");

        let _ = node.kill().await;
        server.abort();
    }
}
