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

use std::convert::Infallible;
use std::pin::Pin;
use std::sync::Arc;

use aex_wire::dispatch::{CONTENT_TYPE_NDJSON, RawRequest, RawResponse, RequestLimits};
use aex_wire::error::{ErrorCode, WireError, WireResult};
use aex_wire::routes::{Plane, RouteId, match_route, route};
use aex_wire::server::AcceptKind;
use aex_wire::types::{HttpMethod, RequestId};
use axum::Router;
use axum::body::{Body, Bytes};
use axum::extract::{DefaultBodyLimit, RawQuery, State};
use axum::http::{HeaderMap, StatusCode, Uri, header};
use axum::response::{IntoResponse as _, Response};
use axum::routing::{MethodFilter, on};
use futures::Stream;

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

/// A boxed, backpressure-aware NDJSON byte stream.
pub type ResponseStream = Pin<Box<dyn Stream<Item = Result<Bytes, Infallible>> + Send + 'static>>;

/// One response produced by a regional generated dispatcher.
pub enum DispatchResponse {
    /// One finite generated response.
    Unary(RawResponse),
    /// A live or retained NDJSON frame stream.
    Ndjson(ResponseStream),
}

impl std::fmt::Debug for DispatchResponse {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unary(response) => formatter.debug_tuple("Unary").field(response).finish(),
            Self::Ndjson(_) => formatter.write_str("Ndjson(<stream>)"),
        }
    }
}

/// The total generated dispatch surface of one deployable.
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
    /// peer, and the narrowing is derived from the deferral ledger rather than
    /// written out. RS-18 still forbids mounting a *handler* that cannot be
    /// fully served; what the route gets instead is the generated refusal arm,
    /// which has no handler, no port and no adapter and so cannot produce a
    /// partial answer.
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
    ) -> WireResult<DispatchResponse>;
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
    /// An owned route is neither served here nor deferred by the ledger.
    ///
    /// This is the totality check the previous design did not have: without it
    /// a route could silently leave a deployable's served set and answer
    /// nothing at all, which is exactly the bare `404` the refusal arm exists
    /// to remove.
    #[error("route `{route}` is owned by `{deployable}` but is neither served nor deferred")]
    Unaccounted {
        /// The offending route.
        route: &'static str,
        /// The deployable that failed to account for it.
        deployable: &'static str,
    },
}

/// A built router plus the exact route set it answers.
///
/// A composition test asserts `routes` equals [`RouteOwner::routes`], which is
/// what makes an unmounted route a build failure. `refused` is the rest of the
/// owned set: templates that answer `501 not_implemented` and reach no handler.
pub struct Mounted {
    /// The `axum` router, ready for `axum::serve` on a bound listener.
    pub router: Router,
    /// Every route with a handler behind it, in `RouteId` order.
    pub routes: Vec<RouteId>,
    /// Every owned route the ledger defers, in `RouteId` order.
    pub refused: Vec<RouteId>,
}

impl std::fmt::Debug for Mounted {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Mounted")
            .field("routes", &self.routes.len())
            .field("refused", &self.refused.len())
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

/// Mounts every route this deployable owns: a handler for the ones it serves,
/// the generated refusal arm for the ones the contract defers.
///
/// The loop iterates the owned projection of the generated table. Two routes
/// sharing a template are mounted as two method filters on that one template, so
/// `PUT` on a path this deployable only reads is a `405` from the router rather
/// than a handler running against the wrong request — and so a wrong method on a
/// deferred template is a `405` rather than the `404` it used to be.
///
/// The refusal set is keyed on `RouteDescriptor::deferred`, never on
/// `owned − served()`. The `session-api` release unit spans both transport
/// halves, so its unary half's `served()` legitimately
/// excludes 24 NDJSON routes that *work*; refusing "everything owned and not
/// served" would take them down.
///
/// # Errors
///
/// Returns [`MountError`] when the served set is empty, contains a duplicate,
/// contains a route this deployable does not own, or when an owned route is
/// neither served nor deferred.
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
    let deferred: Vec<RouteId> = owner
        .routes()
        .into_iter()
        .filter(|id| route(*id).deferred)
        .collect();
    for id in owner.routes() {
        if !routes.contains(&id) && !deferred.contains(&id) {
            return Err(MountError::Unaccounted {
                route: id.as_str(),
                deployable: owner.half(),
            });
        }
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
                    owner: actual.half(),
                    deployable: owner.half(),
                });
            }
            None => {
                return Err(MountError::NotRegional { route: id.as_str() });
            }
        }
        if mounted.contains(&id) || deferred.contains(&id) {
            return Err(MountError::Duplicate { route: id.as_str() });
        }
        let filter = method_filter(descriptor.method);
        // `descriptor.template` is already axum 0.8 path syntax (`{sessionId}`).
        tree = tree.route(descriptor.template, on(filter, handle::<D, A>));
        mounted.push(id);
    }
    for id in &deferred {
        let descriptor = route(*id);
        let filter = method_filter(descriptor.method);
        tree = tree.route(descriptor.template, on(filter, refuse));
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
        refused: deferred,
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
        Ok(DispatchResponse::Unary(response)) => render(response),
        Ok(DispatchResponse::Ndjson(stream)) => render_ndjson(stream),
        Err(failure) => render_error(&context.request_id, context.operation_id, failure),
    }
}

/// The generated refusal arm: `501 not_implemented`, and nothing else.
///
/// It has no handler, no port, no adapter and no application dependency, so it
/// cannot produce a partial answer — which is what lets it be mounted where
/// RS-18 forbids mounting a handler. It returns before [`EdgeAdmission::admit`]
/// runs: no credential parse, no store read, no clock, no network. The body is
/// the published envelope with the vocabulary's own message; the ledger's
/// reason is an engineering note and is not published.
async fn refuse(headers: HeaderMap) -> Response {
    let request_id = request_id(&headers);
    render_error(&request_id, None, WireError::new(ErrorCode::NotImplemented))
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

/// Renders a generated NDJSON stream without buffering it at the edge.
#[must_use]
pub fn render_ndjson(stream: ResponseStream) -> Response {
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, CONTENT_TYPE_NDJSON)
        .header(header::CACHE_CONTROL, "no-store")
        .body(Body::from_stream(stream))
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

/// The refusal a dispatcher answers for a route it cannot handle.
///
/// Two different facts reach this one function, and the code is derived from
/// the route rather than chosen by the caller. A route the *contract* defers
/// answers `not_implemented`, the same code the mounted refusal arm answers, so
/// the stub and the arm can never disagree. A route another deployable owns —
/// the two authoring fragments split across two deployables — keeps
/// `not_found`, because "declared but not built" is false of it. Choosing the
/// wrong one is not cosmetic: `dispatch::declared` replaces any code the route
/// does not declare with `internal_error`.
///
/// Both are unreachable through the router, and
/// `dispatcher_refuses_a_route_it_does_not_own` proves the second.
#[must_use]
pub fn not_served(id: RouteId) -> WireError {
    if route(id).deferred {
        return WireError::new(ErrorCode::NotImplemented);
    }
    WireError::new(ErrorCode::NotFound)
        .with_message(format!("`{id}` is not served by this deployable"))
}

fn accept_kind(headers: &HeaderMap) -> AcceptKind {
    match headers
        .get(header::ACCEPT)
        .and_then(|value| value.to_str().ok())
    {
        Some(value) if value.contains("application/x-ndjson") => AcceptKind::Ndjson,
        Some(value) if value.contains("application/octet-stream") => AcceptKind::Binary,
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
