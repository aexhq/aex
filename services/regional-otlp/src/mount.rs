//! The `axum` mount of the generated `regional:otlp` server trait.
//!
//! The mount is a loop over `RouteGroup::Otlp.routes()`, never a hand-written
//! list of templates. A route that is authored and not mounted is therefore a
//! composition-test failure rather than a runtime `404`, and there is no
//! unavailable-port stub anywhere: a deployable never mounts a route it cannot
//! fully serve.

use std::collections::BTreeSet;
use std::sync::Arc;

use aex_otlp_admission::{ContentCoding, OtlpEncoding};
use aex_wire::dispatch::{DispatchOutcome, RawRequest, RawResponse, RequestLimits};
use aex_wire::error::{ErrorCode, WireError};
use aex_wire::ids::SessionId;
use aex_wire::routes::{Plane, RouteId, TransportKind, match_route, route};
use aex_wire::server::{RouteGroup, dispatch_otlp};
use aex_wire::types::HttpMethod;
use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode, Uri, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{MethodFilter, on};

use crate::admission::{OtlpRequest, OtlpService, SESSION_HEADER};
use aex_internal_contracts::assertion::AssertionAudience;
use aex_regional_http::authz::{LambdaAssertionSource, RegionalProjection};
use aex_regional_http::edge::{RegionalEdge, SystemClock};
use aex_regional_http::mount::{AdmissionRequest, EdgeAdmission as _};

/// The group this deployable owns, and the only one it mounts.
pub const GROUP: RouteGroup = RouteGroup::Otlp;

/// The audience every assertion this deployable accepts must carry.
///
/// One deployable, one audience: an assertion minted for another regional role
/// must never be accepted here, and the reverse. The envelope binds it, so this
/// is a fact the signature covers rather than a check that can be skipped.
pub const AUDIENCE: AssertionAudience = AssertionAudience::RegionalOtlp;

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
    /// The admission service.
    pub service: Arc<OtlpService>,
    /// The body bounds every dispatch enforces.
    pub limits: RequestLimits,
    /// Whether every declared readiness probe has passed.
    pub ready: bool,
    /// The release digest reported on both health endpoints.
    pub release_digest: String,
}

/// The templates this deployable mounts, derived from the route table.
#[must_use]
pub fn mounted_templates() -> BTreeSet<&'static str> {
    GROUP
        .routes()
        .iter()
        .map(|id| route(*id).template)
        .collect()
}

/// Builds the router: every route of the group, plus the two health endpoints.
pub fn router(state: Arc<AppState>) -> axum::Router {
    let mut router = axum::Router::new();
    let declared = mounted_templates();
    for id in owned_routes() {
        let descriptor = route(*id);
        // The one group this deployable serves carries no NDJSON route, which
        // `DispatchOutcome<NoStream>` makes unconstructible rather than merely
        // unreachable.
        debug_assert_eq!(descriptor.transport, TransportKind::Unary);
        debug_assert!(declared.contains(descriptor.template));
        let filter = method_filter(descriptor.method);
        router = router.route(descriptor.template, on(filter, handle));
    }
    debug_assert_eq!(declared.len(), owned_routes().len());
    router
        .with_state(Arc::clone(&state))
        .merge(health_router(state))
}

/// The two internal health endpoints every Rust deployable mounts.
fn health_router(state: Arc<AppState>) -> axum::Router {
    axum::Router::new()
        .route(
            aex_observation_store_dynamodb::health::HEALTHZ,
            axum::routing::get(healthz),
        )
        .route(
            aex_observation_store_dynamodb::health::READYZ,
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
    // One matcher, one table: the composition never re-types a path.
    let Some((id, binding)) = match_route(Plane::Regional, method, uri.path()) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    if id.group() != GROUP {
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
    let context = authorized.to_wire(aex_wire::server::AcceptKind::Json);
    let encoding = match content_type(&headers) {
        Ok(encoding) => encoding,
        Err(failure) => return render_error(Some(&context), &failure),
    };
    let coding = match content_coding(&headers) {
        Ok(coding) => coding,
        Err(failure) => return render_error(Some(&context), &failure),
    };
    let session = match session_header(&headers) {
        Ok(session) => session,
        Err(failure) => return render_error(Some(&context), &failure),
    };

    let request = OtlpRequest::new(
        Arc::clone(&state.service),
        authorized,
        session,
        encoding,
        coding,
    );
    let raw = RawRequest {
        route: id,
        path: binding,
        query: uri.query().unwrap_or_default(),
        body: &body,
    };
    match dispatch_otlp(&request, &context, raw, state.limits).await {
        Ok(DispatchOutcome::Unary(response)) => render(response),
        // `OtlpApi` declares no NDJSON route, so `NoStream` is uninhabited and
        // this arm is unconstructible.
        Ok(DispatchOutcome::Ndjson(never)) => match never.0 {},
        Err(failure) => render_error(Some(&context), &failure),
    }
}

/// Resolves the declared OTLP encoding.
fn content_type(headers: &HeaderMap) -> Result<OtlpEncoding, WireError> {
    let value = headers
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default();
    OtlpEncoding::from_content_type(value).map_err(|error| {
        WireError::new(ErrorCode::InvalidTelemetry).with_message(error.to_string())
    })
}

/// Resolves the declared content coding.
fn content_coding(headers: &HeaderMap) -> Result<ContentCoding, WireError> {
    let value = headers
        .get(header::CONTENT_ENCODING)
        .and_then(|value| value.to_str().ok());
    ContentCoding::from_content_encoding(value).map_err(|error| {
        WireError::new(ErrorCode::InvalidTelemetry).with_message(error.to_string())
    })
}

/// Resolves the session an in-guest collector declared.
fn session_header(headers: &HeaderMap) -> Result<Option<SessionId>, WireError> {
    let Some(value) = headers.get(SESSION_HEADER) else {
        return Ok(None);
    };
    value
        .to_str()
        .ok()
        .and_then(|text| text.parse::<SessionId>().ok())
        .map(Some)
        .ok_or_else(|| {
            WireError::new(ErrorCode::InvalidRequest)
                .with_message(format!("`{SESSION_HEADER}` is not a session identifier"))
        })
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

/// Maps a table method onto the framework's filter.
fn method_filter(method: HttpMethod) -> MethodFilter {
    match method {
        HttpMethod::Get => MethodFilter::GET,
        HttpMethod::Put => MethodFilter::PUT,
        HttpMethod::Post => MethodFilter::POST,
        HttpMethod::Delete => MethodFilter::DELETE,
    }
}

/// Every route this deployable must serve, for the composition assertion.
#[must_use]
pub fn owned_routes() -> &'static [RouteId] {
    GROUP.routes()
}

#[cfg(test)]
mod tests {
    use aex_wire::routes::{BodyClass, Plane, RouteId, TransportKind, route};
    use aex_wire::server::RouteGroup;

    use super::{GROUP, mounted_templates, owned_routes};

    #[test]
    fn every_route_of_the_group_is_mounted() {
        let mounted = mounted_templates();
        for id in owned_routes() {
            assert!(
                mounted.contains(route(*id).template),
                "`{}` is authored and not mounted",
                route(*id).operation_id
            );
        }
        assert_eq!(
            mounted.len(),
            owned_routes().len(),
            "the mounted set and the group's route slice must be the same set"
        );
    }

    #[test]
    fn the_group_is_exactly_the_three_otlp_ingest_routes() {
        assert_eq!(GROUP, RouteGroup::Otlp);
        assert_eq!(owned_routes().len(), 3);
        for id in owned_routes() {
            let descriptor = route(*id);
            assert_eq!(descriptor.plane, Plane::Regional);
            assert_eq!(descriptor.body_class, BodyClass::Otlp);
            assert_eq!(descriptor.transport, TransportKind::Unary);
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
            .filter(|route| route["servedArtifact"] == "regional-otlp")
            .map(|route| {
                RouteId::parse(route["operationId"].as_str().expect("operation id"))
                    .expect("generated operation id")
            })
            .collect();
        assert_eq!(owned_routes(), generated);
    }

    #[test]
    fn this_deployable_mounts_no_query_or_export_route() {
        for id in aex_wire::routes::RouteId::ALL {
            if id.group() == GROUP {
                continue;
            }
            assert!(
                !mounted_templates().contains(route(*id).template)
                    || route(*id).template == route(owned_routes()[0]).template,
                "`{}` belongs to another deployable",
                route(*id).operation_id
            );
        }
    }
}
