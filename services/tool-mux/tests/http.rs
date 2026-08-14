//! Private HTTP route smoke tests.

use std::sync::Arc;

use aex_tool_mux::{
    ExecutorOutput, GuestPort, ReadyHand, RuntimePort, StoragePersistPort, TelemetryEnvelope,
    TelemetryPort, TelemetryPressure, ToolCallIdentity, ToolMux, ToolMuxFuture, ToolTarget,
};
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
    fn start_waiter<'a>(
        &'a self,
        _session: aex_wire::ids::SessionId,
        _hand: aex_runtime_control::HandId,
        _generation: aex_wire::ids::GenerationId,
        _call: &'a ToolCallIdentity,
    ) -> ToolMuxFuture<'a, Result<(), String>> {
        Box::pin(async { Err("unused".to_owned()) })
    }

    fn poll_waiter<'a>(
        &'a self,
        _session: aex_wire::ids::SessionId,
        _hand: aex_runtime_control::HandId,
        _generation: aex_wire::ids::GenerationId,
        _call: &'a ToolCallIdentity,
    ) -> ToolMuxFuture<'a, Result<Option<ReadyHand>, String>> {
        Box::pin(async { Err("unused".to_owned()) })
    }

    fn cancel_waiter<'a>(
        &'a self,
        _generation: aex_wire::ids::GenerationId,
        _call: &'a ToolCallIdentity,
    ) -> ToolMuxFuture<'a, Result<Option<ReadyHand>, String>> {
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
        _deadline_ms: i64,
        _max_result_bytes: usize,
        _timeout_ms: u32,
    ) -> ToolMuxFuture<'a, Result<aex_hands_protocol::rpc::HandsOperationId, String>> {
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
        _call: &'a ToolCallIdentity,
    ) -> ToolMuxFuture<'a, Result<(), String>> {
        Box::pin(async { Err("unused".to_owned()) })
    }
}

impl StoragePersistPort for Unused {
    fn start_persist<'a>(
        &'a self,
        _ready: ReadyHand,
        _source: &'a aex_hands_protocol::operation::GuestPath,
        _logical_name: &'a str,
        _media_type: Option<&'a str>,
        _call: &'a ToolCallIdentity,
    ) -> ToolMuxFuture<'a, Result<(), String>> {
        Box::pin(async { Err("unused".to_owned()) })
    }

    fn read_persist<'a>(
        &'a self,
        _ready: ReadyHand,
        _call: &'a ToolCallIdentity,
    ) -> ToolMuxFuture<'a, Result<Option<ExecutorOutput>, String>> {
        Box::pin(async { Err("unused".to_owned()) })
    }

    fn cancel_persist<'a>(
        &'a self,
        _ready: ReadyHand,
        _call: &'a ToolCallIdentity,
    ) -> ToolMuxFuture<'a, Result<(), String>> {
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
async fn readiness_fails_closed_when_private_assertions_are_unavailable() {
    let unused = Arc::new(Unused);
    let mux = Arc::new(ToolMux::new(
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
}
