//! Generated-table-driven NDJSON routing for `regional-stream`.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use aex_regional_http::edge::{RegionalEdge, SystemClock};
use aex_regional_http::mount::{AdmissionRequest, EdgeAdmission as _};
use aex_regional_http::router::{RouteOwner, route_owner};
use aex_wire::dispatch::{DispatchOutcome, RawRequest, RequestLimits};
use aex_wire::error::{ErrorCode, WireError};
use aex_wire::routes::{Plane, RouteId, match_route, route};
use aex_wire::server::{AcceptKind, dispatch_observations};
use aex_wire::types::{HttpMethod, RequestId};
use axum::Router;
use axum::body::{Body, Bytes};
use axum::extract::{RawQuery, State};
use axum::http::{HeaderMap, StatusCode, Uri, header};
use axum::response::{IntoResponse as _, Response};
use axum::routing::{MethodFilter, get, on};
use futures::StreamExt as _;
use regional_observation_api::{ObservationRequest, ObservationService};

use crate::{ConnectionClass, QuotaManager, Reservation};

/// The production regional edge used by this service.
pub type Edge = RegionalEdge<
    aex_regional_http::authz::LambdaAssertionSource,
    aex_regional_http::authz::RegionalProjection<
        aex_session_dynamodb::projection::ProjectionReader,
    >,
    aex_regional_http::capacity::CapacityProjection<
        aex_session_dynamodb::projection::ProjectionReader,
    >,
    SystemClock,
>;

/// Resolved state shared by every connection.
pub struct AppState {
    /// Authenticated regional admission.
    pub edge: Arc<Edge>,
    /// Authoritative observation reader and stream engine.
    pub service: Arc<ObservationService>,
    /// Generated body decode limits.
    pub limits: RequestLimits,
    /// Per-task and per-workspace socket budgets.
    pub quotas: QuotaManager,
    /// Process-wide drain signal.
    pub draining: Arc<AtomicBool>,
    /// Immutable artifact identity.
    pub release_digest: String,
}

/// Builds exactly the 24 generated NDJSON routes plus internal health.
pub fn router(state: Arc<AppState>) -> Router {
    let mut tree = Router::new();
    for id in RouteOwner::Stream.routes() {
        let descriptor = route(id);
        tree = tree.route(
            descriptor.template,
            on(method_filter(descriptor.method), handle),
        );
    }
    tree.with_state(Arc::clone(&state)).merge(
        Router::new()
            .route("/internal/healthz", get(healthz))
            .route("/internal/readyz", get(readyz))
            .with_state(state),
    )
}

const fn method_filter(method: HttpMethod) -> MethodFilter {
    match method {
        HttpMethod::Get => MethodFilter::GET,
        HttpMethod::Put => MethodFilter::PUT,
        HttpMethod::Post => MethodFilter::POST,
        HttpMethod::Delete => MethodFilter::DELETE,
    }
}

async fn handle(
    State(state): State<Arc<AppState>>,
    method: axum::http::Method,
    uri: Uri,
    headers: HeaderMap,
    RawQuery(query): RawQuery,
    body: Bytes,
) -> Response {
    let request_id = request_id(&headers);
    if state.draining.load(Ordering::Acquire) {
        return unavailable(&request_id);
    }
    let Some(method) = HttpMethod::parse(method.as_str()) else {
        return StatusCode::METHOD_NOT_ALLOWED.into_response();
    };
    let Some((id, binding)) = match_route(Plane::Regional, method, uri.path()) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    if route_owner(id) != Some(RouteOwner::Stream) {
        return StatusCode::NOT_FOUND.into_response();
    }
    let admission = AdmissionRequest {
        request_id: &request_id,
        route: id,
        method,
        headers: &headers,
        body: &body,
    };
    let authorized = match state.edge.admit(&admission).await {
        Ok(context) => context,
        Err(failure) => return aex_regional_http::mount::render_error(&request_id, None, failure),
    };
    let Ok(reservation) = state.quotas.reserve(
        authorized.auth.workspace_id.to_string(),
        connection_class(id),
    ) else {
        return unavailable(&authorized.request_id);
    };
    let context = authorized.to_wire(accept_kind(&headers));
    let request = ObservationRequest::new(Arc::clone(&state.service), authorized);
    let query = query.unwrap_or_default();
    let raw = RawRequest {
        route: id,
        path: binding,
        query: &query,
        body: &body,
    };
    match dispatch_observations(&request, &context, raw, state.limits).await {
        Ok(DispatchOutcome::Ndjson(stream)) => render_frames(stream.0, reservation),
        Ok(DispatchOutcome::Unary(_)) => aex_regional_http::mount::render_error(
            &context.request_id,
            context.operation_id,
            WireError::new(ErrorCode::InternalError),
        ),
        Err(failure) => aex_regional_http::mount::render_error(
            &context.request_id,
            context.operation_id,
            failure,
        ),
    }
}

fn render_frames(
    stream: regional_observation_api::ndjson::FrameStream,
    reservation: Reservation,
) -> Response {
    // The reservation remains in the unfold state until the response body ends,
    // including disconnect and error paths, and is then released by `Drop`.
    let guarded = futures::stream::unfold(
        (stream, Some(reservation)),
        |(mut stream, reservation)| async move {
            stream
                .next()
                .await
                .map(|frame| (frame, (stream, reservation)))
        },
    );
    Response::builder()
        .status(StatusCode::OK)
        .header(
            header::CONTENT_TYPE,
            aex_wire::dispatch::CONTENT_TYPE_NDJSON,
        )
        .header(header::CACHE_CONTROL, "no-store")
        .body(Body::from_stream(guarded))
        .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
}

fn connection_class(id: RouteId) -> ConnectionClass {
    let operation = route(id).operation_id;
    if operation.contains("telemetry") {
        ConnectionClass::Telemetry
    } else if operation.contains("events") {
        ConnectionClass::Session
    } else {
        ConnectionClass::Observation
    }
}

fn accept_kind(headers: &HeaderMap) -> AcceptKind {
    match headers
        .get(header::ACCEPT)
        .and_then(|value| value.to_str().ok())
    {
        Some(value) if value.contains("application/x-ndjson") => AcceptKind::Ndjson,
        Some(value) if value.contains("application/pdf") => AcceptKind::Pdf,
        _ => AcceptKind::Json,
    }
}

fn request_id(headers: &HeaderMap) -> RequestId {
    headers
        .get("x-request-id")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| RequestId::parse(value).ok())
        .unwrap_or_else(|| {
            RequestId::parse(&uuid::Uuid::now_v7().to_string())
                .expect("a UUID is always a request id")
        })
}

fn unavailable(request_id: &RequestId) -> Response {
    aex_regional_http::mount::render_error(
        request_id,
        None,
        WireError::new(ErrorCode::ObservabilityUnavailable)
            .with_retry_after(Duration::from_secs(1)),
    )
}

async fn healthz(State(state): State<Arc<AppState>>) -> Response {
    (
        StatusCode::OK,
        [(header::CACHE_CONTROL, "no-store")],
        axum::Json(serde_json::json!({
            "status": "healthy",
            "releaseDigest": state.release_digest,
            "unavailable": [],
        })),
    )
        .into_response()
}

async fn readyz(State(state): State<Arc<AppState>>) -> Response {
    let draining = state.draining.load(Ordering::Acquire);
    (
        if draining {
            StatusCode::SERVICE_UNAVAILABLE
        } else {
            StatusCode::OK
        },
        [(header::CACHE_CONTROL, "no-store")],
        axum::Json(serde_json::json!({
            "status": if draining { "not_ready" } else { "ready" },
            "releaseDigest": state.release_digest,
            "unavailable": if draining { vec!["draining"] } else { Vec::<&str>::new() },
        })),
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use aex_regional_http::router::RouteOwner;
    use aex_wire::routes::RouteId;

    use super::connection_class;
    use crate::ConnectionClass;

    #[test]
    fn every_owned_route_has_one_bounded_connection_class() {
        let routes = RouteOwner::Stream.routes();
        assert_eq!(routes.len(), 24);
        for id in routes {
            let class = connection_class(id);
            let operation = aex_wire::routes::route(id).operation_id;
            if operation.contains("telemetry") {
                assert_eq!(class, ConnectionClass::Telemetry, "{operation}");
            } else if operation.contains("events") {
                assert_eq!(class, ConnectionClass::Session, "{operation}");
            } else {
                assert_eq!(class, ConnectionClass::Observation, "{operation}");
            }
        }
    }

    #[test]
    fn mounted_routes_match_the_generated_actual_mount_authority() {
        let registry: serde_json::Value = serde_json::from_str(include_str!(
            "../../../api/generated/registries/routes.json"
        ))
        .expect("generated route registry");
        let generated: Vec<RouteId> = registry["routes"]
            .as_array()
            .expect("route rows")
            .iter()
            .filter(|route| route["servedArtifact"] == "regional-stream")
            .map(|route| {
                RouteId::parse(route["operationId"].as_str().expect("operation id"))
                    .expect("generated operation id")
            })
            .collect();
        assert_eq!(RouteOwner::Stream.routes(), generated);
    }
}
