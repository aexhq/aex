//! Maintained-library sandbox-process MCP client mode.
//!
//! Brain starts this same credential-free guest binary as an exact-generation
//! `Exec`. The invocation is carried in a redacting environment value and the
//! helper starts only the configured child with a cleared, explicit environment.

use aex_hands_protocol::operation::{SANDBOX_MCP_REQUEST_VAR, SandboxMcpCall, SandboxMcpTransport};
use rmcp::model::{CallToolRequestParams, ClientInfo, ProtocolVersion};
use rmcp::transport::streamable_http_client::StreamableHttpClientTransportConfig;
use rmcp::transport::{ConfigureCommandExt as _, TokioChildProcess};
use rmcp::{ClientLifecycleMode, ClientServiceExt as _};

pub async fn run() -> std::process::ExitCode {
    match call().await {
        Ok(body) => {
            println!("{body}");
            std::process::ExitCode::SUCCESS
        }
        Err(reason) => {
            eprintln!("sandbox MCP call failed: {reason}");
            std::process::ExitCode::FAILURE
        }
    }
}

async fn call() -> Result<String, &'static str> {
    let encoded = std::env::var(SANDBOX_MCP_REQUEST_VAR).map_err(|_| "request_missing")?;
    let request: SandboxMcpCall = serde_json::from_str(&encoded).map_err(|_| "request_invalid")?;
    let service = match &request.transport {
        SandboxMcpTransport::StreamableHttp { endpoint, headers } => {
            connect_remote(endpoint, headers).await?
        }
        SandboxMcpTransport::ChildProcess {
            command,
            args,
            environment,
            working_directory,
        } => connect_process(command, args, environment, working_directory.as_str()).await?,
    };
    let arguments = request
        .arguments
        .to_value()
        .as_object()
        .cloned()
        .ok_or("arguments_not_object")?;
    let response = service
        .call_tool(CallToolRequestParams::new(request.tool).with_arguments(arguments))
        .await
        .map_err(|_| "tools_call_failed")?;
    let body = serde_json::to_string(&response).map_err(|_| "result_invalid")?;
    let _ = service.cancel().await;
    Ok(body)
}

async fn connect_process(
    executable: &str,
    args: &[String],
    environment: &std::collections::BTreeMap<String, aex_hands_protocol::operation::EnvValue>,
    working_directory: &str,
) -> Result<rmcp::service::RunningService<rmcp::RoleClient, ClientInfo>, &'static str> {
    let mut command = tokio::process::Command::new(executable);
    command
        .args(args)
        .current_dir(working_directory)
        .env_clear();
    for (name, value) in environment {
        command.env(name, value.expose());
    }
    let transport =
        TokioChildProcess::new(command.configure(|_| {})).map_err(|_| "server_start_failed")?;
    let service = ClientInfo::default()
        .serve_with_lifecycle(
            transport,
            ClientLifecycleMode::Discover {
                preferred_versions: vec![ProtocolVersion::V_2026_07_28],
            },
        )
        .await
        .map_err(|_| "protocol_start_failed")?;
    Ok(service)
}

async fn connect_remote(
    endpoint: &str,
    headers: &std::collections::BTreeMap<String, aex_hands_protocol::operation::EnvValue>,
) -> Result<rmcp::service::RunningService<rmcp::RoleClient, ClientInfo>, &'static str> {
    let mut config = StreamableHttpClientTransportConfig::with_uri(endpoint.to_owned())
        .reinit_on_expired_session(false);
    let mut custom_headers = std::collections::HashMap::new();
    for (name, value) in headers {
        let name = http::HeaderName::try_from(name).map_err(|_| "header_invalid")?;
        let mut value =
            http::HeaderValue::try_from(value.expose()).map_err(|_| "header_invalid")?;
        value.set_sensitive(true);
        custom_headers.insert(name, value);
    }
    config.custom_headers = custom_headers;
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|_| "client_invalid")?;
    let transport = rmcp::transport::StreamableHttpClientTransport::with_client(client, config);
    ClientInfo::default()
        .serve_with_lifecycle(
            transport,
            ClientLifecycleMode::Discover {
                preferred_versions: vec![ProtocolVersion::V_2026_07_28],
            },
        )
        .await
        .map_err(|_| "protocol_start_failed")
}
