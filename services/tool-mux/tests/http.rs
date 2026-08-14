//! Private HTTP route smoke tests.

use std::sync::Arc;

use aex_tool_mux::{
    ExecutorOutput, GuestPort, McpPort, ReadyHand, ResultRetentionPort, RuntimePort, SandboxConfig,
    StoragePersistPort, TelemetryEnvelope, TelemetryPort, TelemetryPressure, ToolCallIdentity,
    ToolHandle, ToolMux, ToolMuxFuture, ToolTarget,
};
use aex_wire::ids::PrefixedId as _;
use http_body_util::BodyExt as _;
use tower::ServiceExt as _;

struct Unused;
struct Denied;

impl tool_mux::InternalAuthorizer for Unused {
    fn is_ready(&self) -> bool {
        true
    }

    fn authorize<'a>(
        &'a self,
        _headers: &'a axum::http::HeaderMap,
        _scope: tool_mux::AuthorizationScope,
        _binding: [u8; 32],
    ) -> ToolMuxFuture<'a, Result<(), ()>> {
        Box::pin(async { Ok(()) })
    }
}

impl tool_mux::InternalAuthorizer for Denied {
    fn is_ready(&self) -> bool {
        false
    }

    fn authorize<'a>(
        &'a self,
        _headers: &'a axum::http::HeaderMap,
        _scope: tool_mux::AuthorizationScope,
        _binding: [u8; 32],
    ) -> ToolMuxFuture<'a, Result<(), ()>> {
        Box::pin(async { Err(()) })
    }
}

impl RuntimePort for Unused {
    fn eager_prepare<'a>(
        &'a self,
        _session: aex_wire::ids::SessionId,
        _sandbox: SandboxConfig,
    ) -> ToolMuxFuture<'a, Result<Vec<aex_tool_mux::PreparationProgress>, String>> {
        Box::pin(async { Err("unused".to_owned()) })
    }

    fn wait_ready<'a>(
        &'a self,
        _session: aex_wire::ids::SessionId,
        _hand: aex_runtime_control::HandId,
        _generation: aex_wire::ids::GenerationId,
        _call: &'a ToolCallIdentity,
    ) -> ToolMuxFuture<'a, Result<ReadyHand, String>> {
        Box::pin(async { Err("unused".to_owned()) })
    }

    fn settle_waiter<'a>(
        &'a self,
        _ready: ReadyHand,
        _call: &'a ToolCallIdentity,
    ) -> ToolMuxFuture<'a, Result<(), String>> {
        Box::pin(async { Err("unused".to_owned()) })
    }
}

impl GuestPort for Unused {
    fn hello<'a>(&'a self, _ready: ReadyHand) -> ToolMuxFuture<'a, Result<(), String>> {
        Box::pin(async { Err("unused".to_owned()) })
    }

    fn start<'a>(
        &'a self,
        _ready: ReadyHand,
        _target: &'a ToolTarget,
        _arguments: &'a serde_json::Value,
        _call: &'a ToolCallIdentity,
    ) -> ToolMuxFuture<
        'a,
        Result<Result<ExecutorOutput, aex_hands_protocol::rpc::HandsOperationId>, String>,
    > {
        Box::pin(async { Err("unused".to_owned()) })
    }

    fn read<'a>(
        &'a self,
        _ready: ReadyHand,
        _operation: aex_hands_protocol::rpc::HandsOperationId,
        _max_result_bytes: usize,
        _timeout_ms: u32,
    ) -> ToolMuxFuture<'a, Result<Option<ExecutorOutput>, String>> {
        Box::pin(async { Err("unused".to_owned()) })
    }

    fn cancel<'a>(
        &'a self,
        _ready: ReadyHand,
        _operation: aex_hands_protocol::rpc::HandsOperationId,
    ) -> ToolMuxFuture<'a, Result<(), String>> {
        Box::pin(async { Err("unused".to_owned()) })
    }
}

impl McpPort for Unused {
    fn call_remote<'a>(
        &'a self,
        _endpoint: &'a str,
        _headers: &'a std::collections::BTreeMap<String, aex_wire::ids::ResourceName>,
        _server: &'a aex_wire::ids::ResourceName,
        _tool: &'a str,
        _arguments: &'a serde_json::Value,
        _call: &'a ToolCallIdentity,
    ) -> ToolMuxFuture<'a, Result<ExecutorOutput, String>> {
        Box::pin(async { Err("unused".to_owned()) })
    }
}

impl StoragePersistPort for Unused {
    fn persist<'a>(
        &'a self,
        _ready: ReadyHand,
        _source: &'a aex_hands_protocol::operation::GuestPath,
        _logical_name: &'a str,
        _media_type: Option<&'a str>,
        _call: &'a ToolCallIdentity,
    ) -> ToolMuxFuture<'a, Result<ExecutorOutput, String>> {
        Box::pin(async { Err("unused".to_owned()) })
    }
}

impl ResultRetentionPort for Unused {
    fn retain_inline<'a>(
        &'a self,
        _call: &'a ToolCallIdentity,
        _body: &'a [u8],
    ) -> ToolMuxFuture<'a, Result<aex_tool_mux::RetainedResult, String>> {
        Box::pin(async { Err("unused".to_owned()) })
    }

    fn retain_sandbox_file<'a>(
        &'a self,
        _call: &'a ToolCallIdentity,
        _ready: ReadyHand,
        _path: &'a aex_hands_protocol::operation::GuestPath,
        _bytes: u64,
        _hash: aex_wire::ids::ContentHash,
    ) -> ToolMuxFuture<'a, Result<aex_tool_mux::RetainedResult, String>> {
        Box::pin(async { Err("unused".to_owned()) })
    }

    fn read_handle<'a>(
        &'a self,
        _handle: &'a ToolHandle,
    ) -> ToolMuxFuture<'a, Result<Option<ExecutorOutput>, String>> {
        Box::pin(async { Err("unused".to_owned()) })
    }
}

impl TelemetryPort for Unused {
    fn try_emit(&self, _event: TelemetryEnvelope) -> Result<(), TelemetryPressure> {
        Err(TelemetryPressure::Closed)
    }
}

#[tokio::test]
async fn health_and_readiness_are_the_only_get_routes() {
    let unused = Arc::new(Unused);
    let mux = Arc::new(ToolMux::new(
        unused.clone(),
        unused.clone(),
        unused.clone(),
        unused.clone(),
        unused.clone(),
        unused,
    ));
    let app = tool_mux::router(tool_mux::App::new(mux, Arc::new(Unused)));
    for path in ["/internal/healthz", "/internal/readyz"] {
        let response = app
            .clone()
            .oneshot(
                axum::http::Request::builder()
                    .uri(path)
                    .body(axum::body::Body::empty())
                    .expect("request"),
            )
            .await
            .expect("response");
        assert_eq!(response.status(), axum::http::StatusCode::OK);
        let _ = response.into_body().collect().await.expect("bounded body");
    }
}

#[tokio::test]
async fn disabled_prepare_is_an_accepted_no_op() {
    let unused = Arc::new(Unused);
    let mux = Arc::new(ToolMux::new(
        unused.clone(),
        unused.clone(),
        unused.clone(),
        unused.clone(),
        unused.clone(),
        unused,
    ));
    let session = aex_wire::ids::SessionId::from_uuid7(aex_wire::ids::Uuid7::compose(1, [1; 10]));
    let body = serde_json::to_vec(&aex_tool_mux::EagerPrepareRequest {
        session,
        sandbox: SandboxConfig {
            enabled: false,
            generation: None,
        },
    })
    .expect("request encodes");
    let response = tool_mux::router(tool_mux::App::new(mux, Arc::new(Unused)))
        .oneshot(
            axum::http::Request::builder()
                .method("POST")
                .uri("/internal/hands/prepare")
                .header("content-type", "application/json")
                .body(axum::body::Body::from(body))
                .expect("request"),
        )
        .await
        .expect("response");
    assert_eq!(response.status(), axum::http::StatusCode::ACCEPTED);
}

#[tokio::test]
async fn an_unbound_or_invalid_private_assertion_fails_closed() {
    let unused = Arc::new(Unused);
    let mux = Arc::new(ToolMux::new(
        unused.clone(),
        unused.clone(),
        unused.clone(),
        unused.clone(),
        unused.clone(),
        unused,
    ));
    let app = tool_mux::router(tool_mux::App::new(mux, Arc::new(Denied)));
    let readiness = app
        .clone()
        .oneshot(
            axum::http::Request::builder()
                .uri("/internal/readyz")
                .body(axum::body::Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");
    assert_eq!(
        readiness.status(),
        axum::http::StatusCode::SERVICE_UNAVAILABLE
    );

    let session = aex_wire::ids::SessionId::from_uuid7(aex_wire::ids::Uuid7::compose(1, [1; 10]));
    let body = serde_json::to_vec(&aex_tool_mux::EagerPrepareRequest {
        session,
        sandbox: SandboxConfig {
            enabled: false,
            generation: None,
        },
    })
    .expect("request encodes");
    let unauthorized = app
        .oneshot(
            axum::http::Request::builder()
                .method("POST")
                .uri("/internal/hands/prepare")
                .header("content-type", "application/json")
                .body(axum::body::Body::from(body))
                .expect("request"),
        )
        .await
        .expect("response");
    assert_eq!(unauthorized.status(), axum::http::StatusCode::UNAUTHORIZED);
}
