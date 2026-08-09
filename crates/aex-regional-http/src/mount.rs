//! Generated-table-driven `axum` mounting for the finite regional APIs.
//!
//! The mount loop iterates [`RouteOwner::routes`] — a projection of the one
//! generated route table — and never lists a template. A route that exists in
//! the table and is absent from a deployable's owned set therefore fails the
//! composition test rather than answering `404` at runtime.
//!
//! Everything a handler is allowed to trust is settled before it runs:
//! [`EdgeAdmission`] resolves the verified regional context, and the generated
//! `dispatch_*` function does path, query and body decoding from the same table.
//! This module never re-types a path, a status or an error code.

use std::sync::Arc;

use aex_wire::dispatch::{RawRequest, RawResponse, RequestLimits};
use aex_wire::error::{ErrorCode, WireError, WireResult};
use aex_wire::routes::{Plane, RouteId, match_route, route};
use aex_wire::server::AcceptKind;
use aex_wire::types::{HttpMethod, RequestId};
use axum::Router;
use axum::body::Bytes;
use axum::extract::{DefaultBodyLimit, RawQuery, State};
use axum::http::{HeaderMap, StatusCode, Uri, header};
use axum::response::{IntoResponse as _, Response};
use axum::routing::{MethodFilter, on};

use crate::context::RequestContext;
use crate::envelope::ENVELOPE_BYTES;
use crate::router::{RouteOwner, route_owner};

/// Everything the edge is given about one request before it is admitted.
#[derive(Debug, Clone, Copy)]
pub struct AdmissionRequest<'a> {
    /// The diagnostic identity already minted for this request.
    pub request_id: &'a RequestId,
    /// Which generated route matched.
    pub route: RouteId,
    /// The method the table declares for that route.
    pub method: HttpMethod,
    /// Request headers, verbatim.
    pub headers: &'a HeaderMap,
    /// The received body, already bounded by the provider envelope.
    pub body: &'a [u8],
}

/// Establishes the verified regional request context before any handler runs.
///
/// This is precedence stages 1 through 11: authentication, placement, scope,
/// account state, body limits and replay identity. `dispatch_*` runs stages 7
/// and 12–13 only.
#[async_trait::async_trait]
pub trait EdgeAdmission: Send + Sync + 'static {
    /// Admits one request or refuses it with the published error vocabulary.
    ///
    /// # Errors
    ///
    /// Returns the typed refusal of the first precedence stage that failed.
    async fn admit(&self, request: &AdmissionRequest<'_>) -> Result<RequestContext, WireError>;
}

/// The total unary dispatch surface of one deployable.
///
/// One implementation per deployable matches on [`RouteId::group`] and calls the
/// generated `dispatch_*` for that group, so the deployable never decodes a
/// path, a query parameter or a body itself.
#[async_trait::async_trait]
pub trait UnaryDispatch: Send + Sync + 'static {
    /// The deployable whose owned route set this dispatcher serves.
    fn owner(&self) -> RouteOwner;

    /// The routes this deployable fully serves, in `RouteId` order.
    ///
    /// It defaults to the whole owned set and is a subset of it. A deployable
    /// narrows it only while a named application capability is still owed by a
    /// peer: RS-18 forbids mounting a route that cannot be fully served, so the
    /// route is *absent* from the router rather than mounted and answering a
    /// permanent failure.
    fn served(&self) -> Vec<RouteId> {
        self.owner().routes()
    }

    /// Decodes, calls and encodes one request through the generated dispatchers.
    ///
    /// The **regional** context is passed rather than the wire context alone.
    /// The wire context deliberately carries only what a handler may reason
    /// about — who is asking and what it may replay — and for an `Account`
    /// principal that does not include a workspace, so a handler given only the
    /// wire context could not scope an authority read at all. The implementor
    /// derives the wire context with [`RequestContext::to_wire`] and hands it to
    /// the generated `dispatch_*`.
    ///
    /// # Errors
    ///
    /// Returns the handler's own declared failure, or a decode failure the route
    /// declares.
    async fn dispatch(
        &self,
        cx: &RequestContext,
        accept: AcceptKind,
        raw: RawRequest<'_>,
        limits: RequestLimits,
    ) -> WireResult<RawResponse>;
}

/// Why a composition root refused to build its router.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum MountError {
    /// A route in the owned set is not on the regional plane.
    #[error("route `{route}` is not a regional route and cannot be mounted here")]
    NotRegional {
        /// The offending route.
        route: &'static str,
    },
    /// A route in the owned set belongs to another deployable.
    #[error("route `{route}` belongs to `{owner}`, not `{deployable}`")]
    WrongOwner {
        /// The offending route.
        route: &'static str,
        /// Its real owner.
        owner: &'static str,
        /// The deployable that tried to mount it.
        deployable: &'static str,
    },
    /// The same route was offered twice.
    #[error("route `{route}` was mounted twice")]
    Duplicate {
        /// The offending route.
        route: &'static str,
    },
    /// The owned set was empty, which is never a deployable composition.
    #[error("`{deployable}` mounted no route at all")]
    Empty {
        /// The deployable.
        deployable: &'static str,
    },
}

/// A built router plus the exact route set it answers.
///
/// A composition test asserts `routes` equals [`RouteOwner::routes`], which is
/// what makes an unmounted route a build failure.
pub struct Mounted {
    /// The `axum` router, ready for `axum::serve` on a bound listener.
    pub router: Router,
    /// Every mounted route, in `RouteId` order.
    pub routes: Vec<RouteId>,
}

impl std::fmt::Debug for Mounted {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Mounted")
            .field("routes", &self.routes.len())
            .finish_non_exhaustive()
    }
}

struct MountState<D, A> {
    api: Arc<D>,
    admission: Arc<A>,
    limits: RequestLimits,
}

impl<D, A> Clone for MountState<D, A> {
    fn clone(&self) -> Self {
        Self {
            api: Arc::clone(&self.api),
            admission: Arc::clone(&self.admission),
            limits: self.limits,
        }
    }
}

/// Mounts every route the dispatcher declares it serves.
///
/// The loop iterates the owned projection of the generated table. Two routes
/// sharing a template are mounted as two method filters on that one template, so
/// `PUT` on a path this deployable only reads is a `405` from the router rather
/// than a handler running against the wrong request.
///
/// # Errors
///
/// Returns [`MountError`] when the served set is empty, contains a duplicate, or
/// contains a route this deployable does not own.
pub fn mount_unary<D, A>(
    api: Arc<D>,
    admission: Arc<A>,
    limits: RequestLimits,
) -> Result<Mounted, MountError>
where
    D: UnaryDispatch,
    A: EdgeAdmission,
{
    let owner = api.owner();
    let routes = api.served();
    if routes.is_empty() {
        return Err(MountError::Empty {
            deployable: owner.deployable(),
        });
    }
    let state = MountState {
        api,
        admission,
        limits,
    };
    let mut tree = Router::new();
    let mut mounted: Vec<RouteId> = Vec::with_capacity(routes.len());
    for id in routes {
        let descriptor = route(id);
        if descriptor.plane != Plane::Regional {
            return Err(MountError::NotRegional { route: id.as_str() });
        }
        match route_owner(id) {
            Some(actual) if actual == owner => {}
            Some(actual) => {
                return Err(MountError::WrongOwner {
                    route: id.as_str(),
                    owner: actual.deployable(),
                    deployable: owner.deployable(),
                });
            }
            None => {
                return Err(MountError::NotRegional { route: id.as_str() });
            }
        }
        if mounted.contains(&id) {
            return Err(MountError::Duplicate { route: id.as_str() });
        }
        let filter = method_filter(descriptor.method);
        // `descriptor.template` is already axum 0.8 path syntax (`{sessionId}`).
        tree = tree.route(descriptor.template, on(filter, handle::<D, A>));
        mounted.push(id);
    }
    // The transport ceiling is the declared provider envelope. axum's own
    // extractor default (2 MiB) is smaller than [`ENVELOPE_BYTES`], so leaving
    // it in place would refuse a body the published contract admits — at the
    // extractor, before admission measures anything.
    Ok(Mounted {
        router: tree
            .layer(DefaultBodyLimit::max(ENVELOPE_BYTES))
            .with_state(state),
        routes: mounted,
    })
}

const fn method_filter(method: HttpMethod) -> MethodFilter {
    match method {
        HttpMethod::Get => MethodFilter::GET,
        HttpMethod::Put => MethodFilter::PUT,
        HttpMethod::Post => MethodFilter::POST,
        HttpMethod::Delete => MethodFilter::DELETE,
    }
}

async fn handle<D, A>(
    State(state): State<MountState<D, A>>,
    method: axum::http::Method,
    uri: Uri,
    headers: HeaderMap,
    RawQuery(query): RawQuery,
    body: Bytes,
) -> Response
where
    D: UnaryDispatch,
    A: EdgeAdmission,
{
    let request_id = request_id(&headers);
    let Some(method) = HttpMethod::parse(method.as_str()) else {
        return StatusCode::METHOD_NOT_ALLOWED.into_response();
    };
    // One matcher, one table: the composition crate never re-types a path.
    let Some((id, binding)) = match_route(Plane::Regional, method, uri.path()) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    if route_owner(id) != Some(state.api.owner()) {
        return StatusCode::NOT_FOUND.into_response();
    }
    let admission = AdmissionRequest {
        request_id: &request_id,
        route: id,
        method,
        headers: &headers,
        body: &body,
    };
    let context = match state.admission.admit(&admission).await {
        Ok(context) => context,
        Err(failure) => return render_error(&request_id, None, failure),
    };
    let query = query.unwrap_or_default();
    let raw = RawRequest {
        route: id,
        path: binding,
        query: &query,
        body: &body,
    };
    let effective_limits = RequestLimits {
        max_json_body_bytes: context.limits.json_body_bytes,
        max_otlp_body_bytes: context.limits.otlp_body_bytes,
    };
    match state
        .api
        .dispatch(&context, accept_kind(&headers), raw, effective_limits)
        .await
    {
        Ok(response) => render(response),
        Err(failure) => render_error(&context.request_id, context.operation_id, failure),
    }
}

/// Renders one successful generated response.
///
/// The status is the one the route declares: a handler never names one.
#[must_use]
pub fn render(response: RawResponse) -> Response {
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

/// Renders one refusal as the single published error envelope.
#[must_use]
pub fn render_error(
    request_id: &RequestId,
    operation_id: Option<aex_wire::ids::OperationId>,
    failure: WireError,
) -> Response {
    let (status, envelope, retry_after) = failure.into_response_parts(request_id, operation_id);
    let mut rendered = Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, "application/json");
    if let Some(after) = retry_after {
        rendered = rendered.header(header::RETRY_AFTER, after.as_secs());
    }
    let Ok(body) = serde_json::to_vec(&envelope) else {
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    };
    rendered
        .body(body.into())
        .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
}

/// The refusal a deployable answers for a route it does not serve.
///
/// This exists for the two authoring fragments that are split across two
/// deployables. It is unreachable through the router, which mounts only owned
/// templates, and `dispatcher_refuses_a_route_it_does_not_own` proves it.
#[must_use]
pub fn not_served(id: RouteId) -> WireError {
    WireError::new(ErrorCode::NotFound)
        .with_message(format!("`{id}` is not served by this deployable"))
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
                .expect("a UUID is always a valid request id")
        })
}
