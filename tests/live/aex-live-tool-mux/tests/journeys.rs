//! Live scenario bodies. They run only when the private Tool Mux URL is supplied.

use aex_tool_mux::{ToolStart, ToolStartRequest, ToolTarget};

fn base() -> Option<String> {
    std::env::var(aex_live_tool_mux::URL_ENV)
        .ok()
        .map(|value| value.trim_end_matches('/').to_owned())
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct StartFixture {
    request: ToolStartRequest,
    assertion: String,
}

async fn start_fixture(name: &str) -> Option<(ToolStartRequest, ToolStart)> {
    let base = base()?;
    let fixture: StartFixture =
        serde_json::from_str(&std::env::var(name).ok()?).expect("typed live fixture");
    let response = reqwest::Client::new()
        .post(format!("{base}/internal/tools/start"))
        .header("x-aex-tool-assertion", fixture.assertion)
        .json(&fixture.request)
        .send()
        .await
        .expect("private Tool Mux endpoint is reachable");
    assert!(response.status().is_success());
    let result = response.json().await.expect("typed start result");
    Some((fixture.request, result))
}

#[tokio::test]
async fn private_service_exposes_health_and_readiness() {
    let Some(base) = base() else {
        return;
    };
    let client = reqwest::Client::new();
    for path in ["/internal/healthz", "/internal/readyz"] {
        let response = client
            .get(format!("{base}{path}"))
            .send()
            .await
            .expect("private Tool Mux endpoint is reachable");
        assert!(response.status().is_success(), "{path} must pass");
    }
}

#[tokio::test]
async fn disabled_session_returns_a_model_visible_error_without_a_runtime() {
    let Some((request, response)) = start_fixture(aex_live_tool_mux::DISABLED_REQUEST_ENV).await
    else {
        return;
    };
    assert!(!request.sandbox.enabled && request.sandbox.generation.is_none());
    assert!(matches!(request.target, ToolTarget::OfficialSandbox { .. }));
    let ToolStart::Completed { result } = response else {
        panic!("sandbox opt-out is terminal")
    };
    assert_eq!(
        result.error.as_ref().map(|error| error.code.as_str()),
        Some("sandbox_disabled")
    );
}

#[tokio::test]
async fn official_sandbox_call_waits_for_the_exact_ready_generation() {
    let Some((request, result)) = start_fixture(aex_live_tool_mux::SANDBOX_REQUEST_ENV).await
    else {
        return;
    };
    assert!(request.sandbox.enabled && request.sandbox.generation.is_some());
    assert!(matches!(request.target, ToolTarget::OfficialSandbox { .. }));
    assert!(matches!(
        result,
        ToolStart::Completed { .. } | ToolStart::Accepted { .. }
    ));
}

#[tokio::test]
async fn remote_streamable_http_mcp_starts_through_the_exact_hand() {
    let Some((request, result)) = start_fixture(aex_live_tool_mux::REMOTE_MCP_REQUEST_ENV).await
    else {
        return;
    };
    assert!(request.sandbox.enabled && request.sandbox.generation.is_some());
    assert!(matches!(request.target, ToolTarget::RemoteMcp { .. }));
    assert!(matches!(result, ToolStart::Accepted { .. }));
}

#[tokio::test]
async fn storage_persist_commits_the_verified_latest_value() {
    let Some((request, result)) = start_fixture(aex_live_tool_mux::STORAGE_REQUEST_ENV).await
    else {
        return;
    };
    assert!(matches!(request.target, ToolTarget::StoragePersist { .. }));
    assert!(matches!(result, ToolStart::Accepted { .. }));
}

#[tokio::test]
async fn large_tool_output_starts_detached_without_holding_the_private_connection() {
    let Some((request, result)) = start_fixture(aex_live_tool_mux::LARGE_RESULT_REQUEST_ENV).await
    else {
        return;
    };
    assert!(request.sandbox.enabled && request.sandbox.generation.is_some());
    assert!(matches!(result, ToolStart::Accepted { .. }));
}
