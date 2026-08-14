//! Private distributed Tool Mux service.

pub mod auth;
pub mod detached;
pub mod mcp;
pub mod preparation;
pub mod production_hands;
pub mod production_mcp;
pub mod production_storage;
pub mod storage;
pub mod telemetry;

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use aex_tool_mux::{ToolHandleRequest, ToolMux, ToolStartRequest};
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};

/// Internal API state assembled from the trusted adapters at startup.
#[derive(Clone)]
pub struct App {
    mux: Arc<ToolMux>,
    authorizer: Arc<dyn InternalAuthorizer>,
    accepting: Arc<AtomicBool>,
}

/// Tenant facts the assertion must bind for one request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AuthorizationScope {
    /// Agent session principal.
    pub session: aex_wire::ids::SessionId,
    /// Tenant organization, when carried by this request.
    pub organization: Option<aex_wire::ids::OrganizationId>,
    /// Tenant workspace, when carried by this request.
    pub workspace: Option<aex_wire::ids::WorkspaceId>,
}

impl App {
    /// Binds the application service.
    #[must_use]
    pub fn new(mux: Arc<ToolMux>, authorizer: Arc<dyn InternalAuthorizer>) -> Self {
        Self {
            mux,
            authorizer,
            accepting: Arc::new(AtomicBool::new(true)),
        }
    }

    /// Flips readiness before graceful listener drain begins.
    pub fn begin_drain(&self) {
        self.accepting.store(false, Ordering::Release);
    }
}

/// Private-listener authentication and readiness boundary. Implementations
/// verify the short-lived Brain assertion and its call/session binding; no
/// customer credential is accepted by this audience.
pub trait InternalAuthorizer: Send + Sync + 'static {
    /// Whether verification keys and the private audience binding are usable.
    fn is_ready(&self) -> bool;

    /// Verifies the request headers for this exact session.
    fn authorize<'a>(
        &'a self,
        headers: &'a HeaderMap,
        scope: AuthorizationScope,
        binding: [u8; 32],
    ) -> aex_tool_mux::ToolMuxFuture<'a, Result<(), ()>>;
}

/// Exact private route set.
pub fn router(app: App) -> Router {
    Router::new()
        .route("/internal/healthz", get(|| async { StatusCode::OK }))
        .route("/internal/readyz", get(ready))
        .route("/internal/tools/start", post(start))
        .route("/internal/tools/read", post(read))
        .route("/internal/tools/cancel", post(cancel))
        .with_state(app)
}

async fn ready(State(app): State<App>) -> StatusCode {
    if app.accepting.load(Ordering::Acquire) && app.authorizer.is_ready() {
        StatusCode::OK
    } else {
        StatusCode::SERVICE_UNAVAILABLE
    }
}

async fn start(
    State(app): State<App>,
    headers: HeaderMap,
    Json(request): Json<ToolStartRequest>,
) -> Response {
    let Ok(binding) = aex_tool_mux::request_binding(&request) else {
        return StatusCode::BAD_REQUEST.into_response();
    };
    if app
        .authorizer
        .authorize(
            &headers,
            auth::scope(
                request.identity.session,
                Some(request.identity.organization),
                Some(request.identity.workspace),
            ),
            binding,
        )
        .await
        .is_err()
    {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    match app.mux.start(&request).await {
        Ok(result) => (StatusCode::OK, Json(result)).into_response(),
        Err(_) => StatusCode::BAD_GATEWAY.into_response(),
    }
}

async fn read(
    State(app): State<App>,
    headers: HeaderMap,
    Json(request): Json<ToolHandleRequest>,
) -> Response {
    let Ok(binding) = aex_tool_mux::request_binding(&request) else {
        return StatusCode::BAD_REQUEST.into_response();
    };
    if app
        .authorizer
        .authorize(
            &headers,
            auth::scope(
                request.identity.session,
                Some(request.identity.organization),
                Some(request.identity.workspace),
            ),
            binding,
        )
        .await
        .is_err()
    {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    match app.mux.read(&request).await {
        Ok(result) => (StatusCode::OK, Json(result)).into_response(),
        Err(_) => StatusCode::BAD_GATEWAY.into_response(),
    }
}

async fn cancel(
    State(app): State<App>,
    headers: HeaderMap,
    Json(request): Json<ToolHandleRequest>,
) -> Response {
    let Ok(binding) = aex_tool_mux::request_binding(&request) else {
        return StatusCode::BAD_REQUEST.into_response();
    };
    if app
        .authorizer
        .authorize(
            &headers,
            auth::scope(
                request.identity.session,
                Some(request.identity.organization),
                Some(request.identity.workspace),
            ),
            binding,
        )
        .await
        .is_err()
    {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    match app.mux.cancel(&request).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(_) => StatusCode::BAD_GATEWAY.into_response(),
    }
}
