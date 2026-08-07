//! The `axum` mount of the generated observation server traits.
//!
//! The mount is a loop over each group's route constant, narrowed by one
//! explicit served predicate. A route can leave the router only through that
//! reviewed fail-closed predicate, and tests drive every such route through the
//! production router to prove it answers a bare `404`.

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
use aex_regional_http::capacity::CapacityProjection;
use aex_regional_http::edge::{RegionalEdge, SystemClock};
use aex_regional_http::mount::AdmissionRequest;
#[cfg(not(test))]
use aex_regional_http::mount::EdgeAdmission as _;
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
    CapacityProjection<aex_session_dynamodb::projection::ProjectionReader>,
    SystemClock,
>;

/// The admission edge stored in production application state.
///
/// Production keeps the concrete edge so request admission remains statically
/// dispatched. Unit tests substitute the same owned trait only to prove an
/// unmounted path never reaches admission.
#[cfg(not(test))]
pub type AppEdge = Edge;
#[cfg(test)]
pub type AppEdge = dyn aex_regional_http::mount::EdgeAdmission;

/// Everything one request needs, resolved once at start-up.
pub struct AppState {
    /// The authenticated edge.
    pub edge: Arc<AppEdge>,
    /// The query and lifecycle service.
    pub service: Arc<ObservationService>,
    /// The body bounds every dispatch enforces.
    pub limits: RequestLimits,
    /// Whether every declared readiness probe has passed.
    pub ready: bool,
    /// The release digest reported on both health endpoints.
    pub release_digest: String,
}

/// Every generated route this deployable owns, including temporarily unserved
/// routes whose authority is incomplete.
#[must_use]
pub fn owned_routes() -> Vec<RouteId> {
    GROUPS
        .iter()
        .flat_map(|group| group.routes().iter().copied())
        .filter(|id| route_owner(*id) == Some(RouteOwner::ObservationApi))
        .collect()
}

/// Whether this deployable can answer an owned route completely.
///
/// Export admission is deliberately absent. Its current handler writes only
/// the observation export row while returning a generic operation that the
/// session operation authority cannot read, list or cancel. Mounting it would
/// publish a durable identity with no total lifecycle API, and retrying the
/// caller-minted operation id can currently create a second export. The read,
/// download and revoke routes remain served for export rows produced after the
/// authorities are reconciled.
#[must_use]
pub const fn is_served(id: RouteId) -> bool {
    !matches!(
        id,
        RouteId::TelemetryExportCreate | RouteId::SessionTelemetryExportCreate
    )
}

/// Every owned route this deployable can answer completely today.
#[must_use]
pub fn served_routes() -> Vec<RouteId> {
    owned_routes()
        .into_iter()
        .filter(|id| is_served(*id))
        .collect()
}

/// The templates this deployable mounts, derived from the route table.
#[must_use]
pub fn mounted_templates() -> BTreeSet<&'static str> {
    served_routes()
        .into_iter()
        .map(|id| route(id).template)
        .collect()
}

/// Builds the router: every completely served route of both groups, plus the
/// health endpoints.
pub fn router(state: Arc<AppState>) -> axum::Router {
    let mut router = axum::Router::new();
    let declared = mounted_templates();
    let mut mounted: BTreeSet<&'static str> = BTreeSet::new();
    for id in served_routes() {
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
    if !GROUPS.contains(&group)
        || route_owner(id) != Some(RouteOwner::ObservationApi)
        || !is_served(id)
    {
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
    use std::sync::Arc;

    use aex_observation_domain::keys::ScopeKey;
    use aex_observation_query::plan::Budget;
    use aex_regional_http::context::RequestContext as EdgeContext;
    use aex_regional_http::cursor::{CursorKey, CursorKeyRing};
    use aex_regional_http::mount::{AdmissionRequest, EdgeAdmission};
    use aex_wire::error::{ErrorCode, WireError, WireResult};
    use aex_wire::routes::{Plane, RouteId, route};
    use aex_wire::server::RouteGroup;
    use aex_wire::types::Region;
    use axum::body::{Body, to_bytes};
    use axum::http::{Request, StatusCode, header};
    use regional_observation_api::api::{ObservationService, StreamPolicy, StreamRevalidator};
    use regional_observation_api::reader::ObservationReader;
    use tower::ServiceExt as _;

    use super::{
        AppState, GROUPS, is_served, mounted_templates, owned_routes, router, served_routes,
    };

    #[derive(Debug)]
    struct RefusingEdge;

    #[async_trait::async_trait]
    impl EdgeAdmission for RefusingEdge {
        async fn admit(&self, _request: &AdmissionRequest<'_>) -> Result<EdgeContext, WireError> {
            Err(WireError::new(ErrorCode::Unauthenticated))
        }
    }

    #[derive(Debug)]
    struct RefusingRevalidator;

    #[async_trait::async_trait]
    impl StreamRevalidator for RefusingRevalidator {
        async fn revalidate(
            &self,
            _authorization: &aex_regional_http::context::RegionalAuthorization,
            _scope: &ScopeKey,
        ) -> WireResult<()> {
            Err(WireError::new(ErrorCode::Unauthenticated))
        }
    }

    fn test_state() -> Arc<AppState> {
        let dynamodb = aws_sdk_dynamodb::Client::from_conf(
            aws_sdk_dynamodb::Config::builder()
                .behavior_version(aws_sdk_dynamodb::config::BehaviorVersion::latest())
                .region(aws_sdk_dynamodb::config::Region::new("eu-west-1"))
                .credentials_provider(aws_sdk_dynamodb::config::Credentials::new(
                    "AKIDTESTTESTTESTTEST",
                    "test-secret",
                    None,
                    None,
                    "aex-tests",
                ))
                .build(),
        );
        let s3 = aws_sdk_s3::Client::from_conf(
            aws_sdk_s3::Config::builder()
                .behavior_version(aws_sdk_s3::config::BehaviorVersion::latest())
                .region(aws_sdk_s3::config::Region::new("eu-west-1"))
                .credentials_provider(aws_sdk_s3::config::Credentials::new(
                    "AKIDTESTTESTTESTTEST",
                    "test-secret",
                    None,
                    None,
                    "aex-tests",
                ))
                .build(),
        );
        let reader = ObservationReader::new(
            dynamodb,
            s3,
            "observation-authority",
            "session-authority",
            "observations",
            2_000,
            Arc::new(regional_observation_api::counters::ReadCounters::default()),
        );
        let ring = CursorKeyRing::new(
            CursorKey::new("test", vec![7; 32]).expect("a strong test key"),
            Vec::new(),
        )
        .expect("one unique cursor key");
        let service = ObservationService::new(
            reader,
            Budget::default(),
            1,
            ring,
            Region::EuWest1,
            StreamPolicy::new(Arc::new(RefusingRevalidator)),
        );
        Arc::new(AppState {
            edge: Arc::new(RefusingEdge),
            service: Arc::new(service),
            limits: aex_wire::dispatch::RequestLimits::DEFAULT,
            ready: true,
            release_digest: "test".to_owned(),
        })
    }

    #[test]
    fn every_served_route_of_both_groups_is_mounted() {
        let mounted = mounted_templates();
        for id in served_routes() {
            assert!(
                mounted.contains(route(id).template),
                "`{}` is authored and not mounted",
                route(id).operation_id
            );
        }
    }

    #[test]
    fn this_deployable_owns_twenty_seven_routes_and_serves_twenty_five() {
        // The observations authoring group has 39 operations, but its 24
        // NDJSON operations belong exclusively to `regional-stream`. This
        // Lambda owns the 15 finite observation operations plus 12 lifecycle
        // operations.
        assert_eq!(RouteGroup::Observations.routes().len(), 39);
        assert_eq!(RouteGroup::TelemetryLifecycle.routes().len(), 12);
        assert_eq!(owned_routes().len(), 27);
        assert_eq!(served_routes().len(), 25);
    }

    #[test]
    fn served_routes_match_the_generated_actual_mount_authority() {
        let registry: serde_json::Value = serde_json::from_str(include_str!(
            "../../../api/generated/registries/routes.json"
        ))
        .expect("generated route registry");
        let generated: Vec<RouteId> = registry["routes"]
            .as_array()
            .expect("route rows")
            .iter()
            .filter(|route| route["servedArtifact"] == "regional-observation-api")
            .map(|route| {
                RouteId::parse(route["operationId"].as_str().expect("operation id"))
                    .expect("generated operation id")
            })
            .collect();
        assert_eq!(served_routes(), generated);
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

    #[test]
    fn telemetry_export_admission_is_owned_but_unserved() {
        for id in [
            RouteId::TelemetryExportCreate,
            RouteId::SessionTelemetryExportCreate,
        ] {
            assert!(owned_routes().contains(&id));
            assert!(!is_served(id));
            assert!(!served_routes().contains(&id));
            assert!(!mounted_templates().contains(route(id).template));
        }
    }

    #[tokio::test]
    async fn telemetry_export_admission_is_absent_from_the_real_router() {
        for path in [
            "/api/telemetry/exports",
            "/api/sessions/ses_0000000001e40r2081040g2081/telemetry/exports",
        ] {
            let response = router(test_state())
                .oneshot(
                    Request::post(path)
                        .header(header::CONTENT_TYPE, "application/json")
                        .body(Body::from("{}"))
                        .expect("a request"),
                )
                .await
                .expect("the router answers");
            assert_eq!(response.status(), StatusCode::NOT_FOUND, "{path}");
            assert!(
                response.headers().get(header::LOCATION).is_none(),
                "{path} must not advertise an unreadable operation"
            );
            let body = to_bytes(response.into_body(), usize::MAX)
                .await
                .expect("the fallback body is readable");
            assert!(body.is_empty(), "{path} must publish no partial body");
        }
    }
}
