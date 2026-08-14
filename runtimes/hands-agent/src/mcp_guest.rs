//! Maintained-library sandbox-process MCP client mode.
//!
//! Brain starts this same credential-free guest binary as an exact-generation
//! `Exec`. The invocation is carried in a redacting environment value and the
//! helper starts only the configured child with a cleared, explicit environment.

use aex_hands_protocol::operation::{
    SANDBOX_MCP_QUALIFY_VAR, SANDBOX_MCP_REQUEST_VAR, SandboxMcpCall, SandboxMcpQualification,
};
use rmcp::model::{CallToolRequestParams, ClientInfo, PaginatedRequestParams, ProtocolVersion};
use rmcp::transport::{ConfigureCommandExt as _, TokioChildProcess};
use rmcp::{ClientLifecycleMode, ClientServiceExt as _};

const MAX_LIST_PAGES: usize = 8;
const MAX_TOOLS: usize = 128;

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

/// Starts one configured server, completes the pinned handshake, and lists a
/// bounded launch surface before session admission may publish readiness.
pub async fn qualify() -> std::process::ExitCode {
    match qualify_server().await {
        Ok(body) => {
            println!("{body}");
            std::process::ExitCode::SUCCESS
        }
        Err(reason) => {
            eprintln!("sandbox MCP qualification failed: {reason}");
            std::process::ExitCode::FAILURE
        }
    }
}

async fn call() -> Result<String, &'static str> {
    let encoded = std::env::var(SANDBOX_MCP_REQUEST_VAR).map_err(|_| "request_missing")?;
    let request: SandboxMcpCall = serde_json::from_str(&encoded).map_err(|_| "request_invalid")?;
    let service = connect(
        &request.command,
        &request.args,
        &request.environment,
        request.working_directory.as_str(),
    )
    .await?;
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

async fn qualify_server() -> Result<String, &'static str> {
    let encoded = std::env::var(SANDBOX_MCP_QUALIFY_VAR).map_err(|_| "request_missing")?;
    let request: SandboxMcpQualification =
        serde_json::from_str(&encoded).map_err(|_| "request_invalid")?;
    let service = connect(
        &request.command,
        &request.args,
        &request.environment,
        request.working_directory.as_str(),
    )
    .await?;
    let mut names = Vec::new();
    let mut cursor = None;
    for _ in 0..MAX_LIST_PAGES {
        let page = service
            .list_tools(Some(PaginatedRequestParams::default().with_cursor(cursor)))
            .await
            .map_err(|_| "tools_list_failed")?;
        for tool in page.tools {
            if names.len() == MAX_TOOLS {
                return Err("too_many_tools");
            }
            names.push(tool.name.to_string());
        }
        cursor = page.next_cursor;
        if cursor.is_none() {
            names.sort();
            let body = serde_json::to_string(&names).map_err(|_| "result_invalid")?;
            let _ = service.cancel().await;
            return Ok(body);
        }
    }
    Err("too_many_pages")
}

async fn connect(
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
