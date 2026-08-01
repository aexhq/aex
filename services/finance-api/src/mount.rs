//! Mounting the generated `central:billing` group on `axum`.
//!
//! The mount is a loop over `RouteGroup::Billing.routes()`, so a route that is
//! authored and never mounted is a failing composition test rather than a
//! runtime `404`. No template is re-typed here: `descriptor.template` is
//! already `axum` 0.8 path syntax, and `match_route` resolves the concrete path
//! back to the one route table inside the handler.

use std::sync::Arc;

use aex_wire::dispatch::{DispatchOutcome, RawRequest, RawResponse};
use aex_wire::error::WireError;
use aex_wire::routes::{Plane, RouteId, match_route, route};
use aex_wire::server::{BillingApi, RouteGroup, dispatch_billing};
use aex_wire::types::HttpMethod;
use axum::body::Bytes;
use axum::extract::{RawQuery, State};
use axum::http::{HeaderMap, StatusCode, Uri, header};
use axum::response::{IntoResponse as _, Response};
use axum::routing::{MethodFilter, get, on};

use crate::edge::{CentralEdge, EdgeRequest, IDEMPOTENCY_KEY_HEADER, REQUEST_ID_HEADER};
use crate::health::{Readiness, health, ready};

/// Everything one request needs, shared by every mounted route.
#[derive(Debug)]
pub struct AppState<E, A> {
    /// The request edge that establishes the context.
    pub edge: Arc<E>,
    /// The generated server-trait implementation.
    pub api: Arc<A>,
    /// The readiness gate, driven by the database-role probe.
    pub readiness: Arc<Readiness>,
}

impl<E, A> Clone for AppState<E, A> {
    fn clone(&self) -> Self {
        Self {
            edge: Arc::clone(&self.edge),
            api: Arc::clone(&self.api),
            readiness: Arc::clone(&self.readiness),
        }
    }
}

/// The route ids this deployable serves.
///
/// One projection of the generated table: `finance-api` owns exactly the
/// `central:billing` fragment, and nothing else.
#[must_use]
pub fn mounted_routes() -> &'static [RouteId] {
    RouteGroup::Billing.routes()
}

/// Builds the whole application: the two internal probes plus every billing
/// route the generated table declares.
pub fn app<E, A>(state: AppState<E, A>) -> axum::Router
where
    E: CentralEdge,
    A: BillingApi,
{
    let mut router = axum::Router::new()
        .route("/internal/healthz", get(health))
        .route("/internal/readyz", get(ready::<E, A>));
    for id in mounted_routes() {
        let descriptor = route(*id);
        debug_assert_eq!(
            descriptor.plane,
            Plane::Central,
            "finance-api serves only central routes"
        );
        let filter = match descriptor.method {
            HttpMethod::Get => MethodFilter::GET,
            HttpMethod::Put => MethodFilter::PUT,
            HttpMethod::Post => MethodFilter::POST,
            HttpMethod::Delete => MethodFilter::DELETE,
        };
        router = router.route(descriptor.template, on(filter, handle::<E, A>));
    }
    router.with_state(state)
}

/// One request, from the raw parts to the rendered response.
async fn handle<E, A>(
    State(state): State<AppState<E, A>>,
    method: axum::http::Method,
    uri: Uri,
    headers: HeaderMap,
    RawQuery(query): RawQuery,
    body: Bytes,
) -> Response
where
    E: CentralEdge,
    A: BillingApi,
{
    let query = query.unwrap_or_default();
    let Some(method) = HttpMethod::parse(method.as_str()) else {
        return StatusCode::METHOD_NOT_ALLOWED.into_response();
    };
    // One matcher, one table: the composition never re-types a path.
    let Some((id, binding)) = match_route(Plane::Central, method, uri.path()) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    if !mounted_routes().contains(&id) {
        return StatusCode::NOT_FOUND.into_response();
    }

    let supplied_request_id = headers
        .get(REQUEST_ID_HEADER)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    let edge_request = EdgeRequest {
        route: id,
        path: uri.path(),
        query: &query,
        headers: &headers,
    };

    // The route table's replay policy is enforced before the credential, so a
    // request that cannot be replayed safely is refused on its own terms.
    let supplied_key = headers
        .get(IDEMPOTENCY_KEY_HEADER)
        .and_then(|value| value.to_str().ok());
    if let Err(failure) = crate::edge::policy::replay_key(id, supplied_key) {
        return render_error(supplied_request_id.as_deref(), None, failure);
    }

    let cx = match state.edge.admit(&edge_request).await {
        Ok(cx) => cx,
        Err(failure) => return render_error(supplied_request_id.as_deref(), None, failure),
    };
    if let Err(failure) = crate::edge::policy::scope(id, &cx.granted_scopes) {
        return render_error(Some(cx.request_id.as_str()), cx.operation_id, failure);
    }

    let raw = RawRequest {
        route: id,
        path: binding,
        query: &query,
        body: &body,
    };
    match dispatch_billing(state.api.as_ref(), &cx, raw, state.edge.limits()).await {
        Ok(DispatchOutcome::Unary(response)) => render(response),
        // `BillingApi` declares no NDJSON route, so `NoStream` is uninhabited
        // and this arm is unreachable by construction rather than by convention.
        Ok(DispatchOutcome::Ndjson(never)) => match never {},
        Err(failure) => render_error(Some(cx.request_id.as_str()), cx.operation_id, failure),
    }
}

/// Renders a successful dispatch.
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

/// Renders a refusal through the one published envelope.
fn render_error(
    request_id: Option<&str>,
    operation_id: Option<aex_wire::ids::OperationId>,
    failure: WireError,
) -> Response {
    let request_id = crate::edge::request_id(request_id);
    let (status, envelope, retry_after) = failure.into_response_parts(&request_id, operation_id);
    let mut rendered = Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, "application/json");
    if let Some(after) = retry_after {
        rendered = rendered.header(header::RETRY_AFTER, after.as_secs());
    }
    let body = serde_json::to_vec(&envelope).unwrap_or_else(|_| b"{}".to_vec());
    rendered
        .body(body.into())
        .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
}

#[cfg(test)]
mod tests {
    use aex_wire::routes::{Plane, RouteId, route};
    use aex_wire::server::RouteGroup;

    use super::mounted_routes;

    #[test]
    fn the_mounted_set_is_exactly_the_generated_billing_group() {
        assert_eq!(mounted_routes(), RouteGroup::Billing.routes());
        assert_eq!(mounted_routes().len(), 8);
        for id in mounted_routes() {
            assert_eq!(route(*id).plane, Plane::Central);
            assert_eq!(route(*id).fragment, "billing");
            assert_eq!(RouteId::group(*id), RouteGroup::Billing);
        }
    }

    #[test]
    fn no_route_outside_the_billing_group_is_mounted() {
        for id in RouteId::ALL {
            let owned = route(*id).fragment == "billing" && route(*id).plane == Plane::Central;
            assert_eq!(
                mounted_routes().contains(id),
                owned,
                "{} is mounted by the wrong deployable",
                route(*id).operation_id
            );
        }
    }
}
