//! The `axum` mount of the generated observation server traits.
//!
//! The mount is a loop over each group's route constant, never a hand-written
//! list of templates. A route that is authored and not mounted is therefore a
//! composition-test failure rather than a runtime `404`.

use std::collections::BTreeSet;
use std::sync::Arc;

use aex_wire::dispatch::{DispatchOutcome, RawRequest, RawResponse, RequestLimits};
use aex_wire::error::{ErrorCode, WireError};
use aex_wire::routes::{Plane, RouteId, match_route, route};
use aex_wire::server::{RouteGroup, dispatch_observations, dispatch_telemetry_lifecycle};
use aex_wire::types::{HttpMethod, Timestamp};
use axum::body::{Body, Bytes};
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode, Uri, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{MethodFilter, on};

use crate::api::{ObservationRequest, ObservationService};
use crate::edge::{Ed25519Anchors, HttpAssertionSource, RequestAuthority};

/// The two groups this deployable owns.
pub const GROUPS: [RouteGroup; 2] = [RouteGroup::Observations, RouteGroup::TelemetryLifecycle];

/// The composed edge this deployable runs.
pub type Edge = RequestAuthority<HttpAssertionSource, Ed25519Anchors>;

/// Everything one request needs, resolved once at start-up.
pub struct AppState {
    /// The authenticated edge.
    pub edge: Arc<Edge>,
    /// The query and lifecycle service.
    pub service: Arc<ObservationService>,
    /// The body bounds every dispatch enforces.
    pub limits: RequestLimits,
    /// Whether every declared readiness probe has passed.
    pub ready: bool,
    /// The release digest reported on both health endpoints.
    pub release_digest: String,
}

/// Every route this deployable must serve.
#[must_use]
pub fn owned_routes() -> Vec<RouteId> {
    GROUPS
        .iter()
        .flat_map(|group| group.routes().iter().copied())
        .collect()
}

/// The templates this deployable mounts, derived from the route table.
#[must_use]
pub fn mounted_templates() -> BTreeSet<&'static str> {
    owned_routes()
        .into_iter()
        .map(|id| route(id).template)
        .collect()
}

/// Builds the router: every route of both groups, plus the health endpoints.
pub fn router(state: Arc<AppState>) -> axum::Router {
    let mut router = axum::Router::new();
    let declared = mounted_templates();
    let mut mounted: BTreeSet<&'static str> = BTreeSet::new();
    for id in owned_routes() {
        let descriptor = route(id);
        debug_assert!(declared.contains(descriptor.template));
        if !mounted.insert(descriptor.template) {
            // Two methods can share one template; the matcher resolves which.
            continue;
        }
        let filter = MethodFilter::GET
            .or(MethodFilter::POST)
            .or(MethodFilter::PUT)
            .or(MethodFilter::DELETE);
        router = router.route(descriptor.template, on(filter, handle));
    }
    debug_assert_eq!(mounted.len(), declared.len());
    router
        .with_state(Arc::clone(&state))
        .merge(health_router(state))
}

/// The two internal health endpoints every Rust deployable mounts.
fn health_router(state: Arc<AppState>) -> axum::Router {
    axum::Router::new()
        .route(
            aex_observation_store_aws::health::HEALTHZ,
            axum::routing::get(healthz),
        )
        .route(
            aex_observation_store_aws::health::READYZ,
            axum::routing::get(readyz),
        )
        .with_state(state)
}

/// Liveness: the process is running.
async fn healthz(State(state): State<Arc<AppState>>) -> Response {
    (
        StatusCode::OK,
        [(header::CACHE_CONTROL, "no-store")],
        axum::Json(serde_json::json!({
            "status": "healthy",
            "releaseDigest": state.release_digest,
        })),
    )
        .into_response()
}

/// Readiness: every declared probe has actually passed.
async fn readyz(State(state): State<Arc<AppState>>) -> Response {
    let (status, text) = if state.ready {
        (StatusCode::OK, "ready")
    } else {
        (StatusCode::SERVICE_UNAVAILABLE, "not_ready")
    };
    (
        status,
        [(header::CACHE_CONTROL, "no-store")],
        axum::Json(serde_json::json!({
            "status": text,
            "releaseDigest": state.release_digest,
        })),
    )
        .into_response()
}

/// One handler for every mounted route: the table resolves which.
async fn handle(
    State(state): State<Arc<AppState>>,
    method: axum::http::Method,
    uri: Uri,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let Some(method) = HttpMethod::parse(method.as_str()) else {
        return StatusCode::METHOD_NOT_ALLOWED.into_response();
    };
    let Some((id, binding)) = match_route(Plane::Regional, method, uri.path()) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let group = id.group();
    if !GROUPS.contains(&group) {
        return StatusCode::NOT_FOUND.into_response();
    }
    let descriptor = route(id);
    let Ok(now) = Timestamp::from_datetime_trunc_ms(time::OffsetDateTime::now_utc()) else {
        return render_error(None, &WireError::new(ErrorCode::InternalError));
    };
    let authorized = match state.edge.authorize(descriptor, &headers, now).await {
        Ok(authorized) => authorized,
        Err(failure) => return render_error(None, &failure),
    };
    let context = authorized.context.clone();
    let request = ObservationRequest::new(Arc::clone(&state.service), authorized);
    let raw = RawRequest {
        route: id,
        path: binding,
        query: uri.query().unwrap_or_default(),
        body: &body,
    };
    match group {
        RouteGroup::Observations => {
            match dispatch_observations(&request, &context, raw, state.limits).await {
                Ok(DispatchOutcome::Unary(response)) => render(response),
                Ok(DispatchOutcome::Ndjson(stream)) => render_frames(stream.0),
                Err(failure) => render_error(Some(&context), &failure),
            }
        }
        _ => match dispatch_telemetry_lifecycle(&request, &context, raw, state.limits).await {
            Ok(DispatchOutcome::Unary(response)) => render(response),
            // `TelemetryLifecycleApi` declares no NDJSON route, so `NoStream` is
            // uninhabited and this arm is unconstructible.
            Ok(DispatchOutcome::Ndjson(never)) => match never.0 {},
            Err(failure) => render_error(Some(&context), &failure),
        },
    }
}

/// Renders a unary response exactly as the route table declares it.
fn render(response: RawResponse) -> Response {
    let mut rendered = Response::builder().status(response.status);
    if let Some(content_type) = response.content_type {
        rendered = rendered.header(header::CONTENT_TYPE, content_type);
    }
    if let Some(etag) = response.etag {
        rendered = rendered.header(header::ETAG, etag.as_str());
    }
    if let Some(location) = response.location {
        rendered = rendered.header(header::LOCATION, location);
    }
    rendered
        .body(response.body.into())
        .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
}

/// Renders one frame stream.
///
/// The status is fixed before the first frame, so after this point the only
/// terminal signal is a `rotate` frame — which is exactly why the frame
/// vocabulary has one.
fn render_frames(stream: crate::ndjson::FrameStream) -> Response {
    Response::builder()
        .status(StatusCode::OK)
        .header(
            header::CONTENT_TYPE,
            aex_wire::dispatch::CONTENT_TYPE_NDJSON,
        )
        .header(header::CACHE_CONTROL, "no-store")
        .body(Body::from_stream(stream))
        .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
}

/// Renders one failure through the single published envelope.
fn render_error(
    context: Option<&aex_wire::server::RequestContext>,
    failure: &WireError,
) -> Response {
    let request_id = context.map_or_else(
        || {
            aex_wire::types::RequestId::parse("00000000000000000000000000000000")
                .expect("the fallback diagnostic id always parses")
        },
        |context| context.request_id.clone(),
    );
    let (status, envelope, retry_after) = failure.clone().into_response_parts(
        &request_id,
        context.and_then(|context| context.operation_id),
    );
    let mut rendered = Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, "application/json");
    if let Some(after) = retry_after {
        rendered = rendered.header(header::RETRY_AFTER, after.as_secs().max(1));
    }
    let body = serde_json::to_vec(&envelope).unwrap_or_default();
    rendered
        .body(body.into())
        .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
}

#[cfg(test)]
mod tests {
    use aex_wire::routes::{Plane, RouteId, route};
    use aex_wire::server::RouteGroup;

    use super::{GROUPS, mounted_templates, owned_routes};

    #[test]
    fn every_route_of_both_groups_is_mounted() {
        let mounted = mounted_templates();
        for id in owned_routes() {
            assert!(
                mounted.contains(route(id).template),
                "`{}` is authored and not mounted",
                route(id).operation_id
            );
        }
    }

    #[test]
    fn this_deployable_owns_fifty_one_routes() {
        // The 39 observation operations plus the 12 gap and export operations.
        assert_eq!(RouteGroup::Observations.routes().len(), 39);
        assert_eq!(RouteGroup::TelemetryLifecycle.routes().len(), 12);
        assert_eq!(owned_routes().len(), 51);
    }

    #[test]
    fn no_route_outside_the_two_groups_is_claimed() {
        for id in RouteId::ALL {
            let owned = owned_routes().contains(id);
            assert_eq!(
                owned,
                GROUPS.contains(&id.group()),
                "`{}` disagrees with its own group",
                route(*id).operation_id
            );
        }
    }

    #[test]
    fn every_owned_route_is_regional() {
        for id in owned_routes() {
            assert_eq!(route(id).plane, Plane::Regional);
        }
    }
}
