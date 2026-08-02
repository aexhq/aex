//! The `axum` mount of the generated observation server traits.
//!
//! The mount is a loop over each group's route constant, never a hand-written
//! list of templates. A route that is authored and not mounted is therefore a
//! composition-test failure rather than a runtime `404`.

use std::collections::BTreeSet;
use std::sync::Arc;

use aex_wire::dispatch::{DispatchOutcome, RawRequest, RawResponse, RequestLimits};
use aex_wire::error::WireError;
use aex_wire::routes::{Plane, RouteId, match_route, route};
use aex_wire::server::{RouteGroup, dispatch_observations, dispatch_telemetry_lifecycle};
use aex_wire::types::HttpMethod;
use axum::body::{Body, Bytes};
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode, Uri, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{MethodFilter, on};

use aex_internal_contracts::assertion::AssertionAudience;
use aex_regional_http::authz::{LambdaAssertionSource, RegionalProjection};
use aex_regional_http::edge::{RegionalEdge, SystemClock};
use aex_regional_http::mount::{AdmissionRequest, EdgeAdmission as _};
use aex_regional_http::router::{RouteOwner, route_owner};

use regional_observation_api::api::{ObservationRequest, ObservationService};

/// The two groups this deployable owns.
pub const GROUPS: [RouteGroup; 2] = [RouteGroup::Observations, RouteGroup::TelemetryLifecycle];

/// The audience every assertion this deployable accepts must carry.
///
/// One deployable, one audience: an assertion minted for another regional role
/// must never be accepted here, and the reverse. The envelope binds it, so this
/// is a fact the signature covers rather than a check that can be skipped.
pub const AUDIENCE: AssertionAudience = AssertionAudience::RegionalObservation;

/// The edge every regional deployable runs.
///
/// There is one implementation of the precedence stages in the platform and this
/// is it. An earlier revision of this binary carried a private copy — its own
/// trust anchors, its own signed-assertion shape and an `HttpAssertionSource`
/// that posted the customer's credential **verbatim** to a path `central-authz`
/// does not expose. All three are gone.
pub type Edge = RegionalEdge<
    LambdaAssertionSource,
    RegionalProjection<aex_session_dynamodb::projection::ProjectionReader>,
    SystemClock,
>;

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
        .filter(|id| route_owner(*id) == Some(RouteOwner::ObservationApi))
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
    if !GROUPS.contains(&group) || route_owner(id) != Some(RouteOwner::ObservationApi) {
        return StatusCode::NOT_FOUND.into_response();
    }
    let request_id = diagnostic_id(&headers);
    let authorized = match state
        .edge
        .admit(&AdmissionRequest {
            request_id: &request_id,
            route: id,
            method,
            headers: &headers,
            body: &body,
        })
        .await
    {
        Ok(context) => context,
        Err(failure) => return render_error(None, &failure),
    };
    let context = authorized.to_wire(accept_kind(&headers));
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
fn render_frames(stream: regional_observation_api::ndjson::FrameStream) -> Response {
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

/// The diagnostic identity the edge stamps this request with.
fn diagnostic_id(headers: &HeaderMap) -> aex_wire::types::RequestId {
    headers
        .get("x-request-id")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| aex_wire::types::RequestId::parse(value).ok())
        .unwrap_or_else(|| {
            aex_wire::types::RequestId::parse(&uuid::Uuid::now_v7().to_string())
                .expect("a UUID is always a valid request id")
        })
}

/// Which representation the caller asked for.
fn accept_kind(headers: &HeaderMap) -> aex_wire::server::AcceptKind {
    match headers
        .get(header::ACCEPT)
        .and_then(|value| value.to_str().ok())
    {
        Some(value) if value.contains("application/x-ndjson") => {
            aex_wire::server::AcceptKind::Ndjson
        }
        Some(value) if value.contains("application/pdf") => aex_wire::server::AcceptKind::Pdf,
        _ => aex_wire::server::AcceptKind::Json,
    }
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
    fn this_deployable_owns_the_twenty_seven_finite_observation_routes() {
        // The observations authoring group has 39 operations, but its 24
        // NDJSON operations belong exclusively to `regional-stream`. This
        // Lambda owns the 15 finite observation operations plus 12 lifecycle
        // operations.
        assert_eq!(RouteGroup::Observations.routes().len(), 39);
        assert_eq!(RouteGroup::TelemetryLifecycle.routes().len(), 12);
        assert_eq!(owned_routes().len(), 27);
    }

    #[test]
    fn no_route_outside_the_two_groups_is_claimed() {
        for id in RouteId::ALL {
            let owned = owned_routes().contains(id);
            let expected = GROUPS.contains(&id.group())
                && aex_regional_http::router::route_owner(*id)
                    == Some(aex_regional_http::router::RouteOwner::ObservationApi);
            assert_eq!(
                owned,
                expected,
                "`{}` disagrees with ownership",
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
