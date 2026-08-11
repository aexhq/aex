//! Generated-table-driven NDJSON routing for `regional-stream`.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use aex_regional_http::capability::{Grant, StreamSocket};
use aex_regional_http::edge::{RegionalEdge, SystemClock};
use aex_regional_http::envelope::ENVELOPE_BYTES;
use aex_regional_http::mount::{AdmissionRequest, EdgeAdmission as _};
use aex_regional_http::router::{RouteOwner, route_owner};
use aex_wire::dispatch::{DispatchOutcome, RawRequest, RequestLimits};
use aex_wire::error::{ErrorCode, WireError};
use aex_wire::routes::{Plane, RouteId, match_route, route};
use aex_wire::server::{AcceptKind, dispatch_observations};
use aex_wire::types::{HttpMethod, RequestId};
use axum::Router;
use axum::body::{Body, Bytes};
use axum::extract::{DefaultBodyLimit, RawQuery, State};
use axum::http::{HeaderMap, StatusCode, Uri, header};
use axum::response::{IntoResponse as _, Response};
use axum::routing::{MethodFilter, on};
use futures::StreamExt as _;
use regional_observation_api::{ObservationRequest, ObservationService};

use crate::stream::{ConnectionClass, QuotaManager, Reservation};

/// The production regional edge used by this service.
pub type Edge = RegionalEdge<
    aex_regional_http::authz::RegionalProjection<
        aex_session_dynamodb::projection::ProjectionReader,
    >,
    SystemClock,
>;

/// Resolved state shared by every connection.
///
/// **This struct is the stream half's capability statement, and it is
/// write-free by construction.** It holds an edge, a read-only observation
/// service, decode limits, socket quotas and the shared drain flag. There is no
/// [`crate::session::Stores`] here and no field from which one can be derived,
/// which is what makes "a stream route cannot reach a write handle" a fact the
/// compiler enforces rather than a fact the environment used to imply. Widening
/// it is a visible diff in this file; see [`crate::capability`] for the whole
/// argument.
pub struct AppState {
    /// Authenticated regional admission, bound to the `RegionalStream` audience.
    pub edge: Arc<Edge>,
    /// Authoritative observation reader and stream engine.
    pub service: Arc<ObservationService>,
    /// Generated body decode limits.
    pub limits: RequestLimits,
    /// Per-task and per-workspace socket budgets.
    pub quotas: QuotaManager,
    /// Process-wide drain signal, shared with the session half and readiness.
    pub draining: Arc<AtomicBool>,
}

/// Builds exactly the 24 generated NDJSON routes.
///
/// Health is deliberately **not** mounted here. This function used to hand-roll
/// `/internal/healthz` and `/internal/readyz` on the same two paths the shared
/// [`aex_regional_http::health::router`] registers, which the session half
/// already merges. Two routers offering the same method on the same path is a
/// `Router::merge` panic on axum 0.8 — a start-up crash, not a duplicate
/// endpoint — so the merged composition root mounts the shared pair once and
/// this half reports drain through the shared [`aex_regional_http::health::Readiness`]
/// drain signal instead of through a second copy of the same JSON body.
///
/// The [`Grant<StreamSocket>`] argument is unforgeable outside
/// [`crate::capability::Composition`], so no library and no test helper can
/// stand this router up without being handed one by the composition root.
pub fn router(_grant: &Grant<StreamSocket>, state: Arc<AppState>) -> Router {
    let mut tree = Router::new();
    for id in RouteOwner::Stream.routes() {
        let descriptor = route(id);
        tree = tree.route(
            descriptor.template,
            on(method_filter(descriptor.method), handle),
        );
    }
    // The transport ceiling is the declared provider envelope, exactly as on
    // the unary mounts: axum's own extractor default (2 MiB) is smaller than
    // `ENVELOPE_BYTES` and would refuse a body the published contract admits
    // before this service's admission ever measures it.
    tree.layer(DefaultBodyLimit::max(ENVELOPE_BYTES))
        .with_state(state)
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
    // The drain asymmetry, stated rather than inherited.
    //
    // This half refuses a *new* socket the instant the drain flag rises; the
    // shared `mount_unary::handle` the session half uses has no drain check at
    // all and keeps serving. In two processes nobody could see that difference.
    // In one process, sharing one flag, it is visible — so it is a decision:
    //
    // A new stream connection admitted during a drain would be a 15-minute lease
    // handed out by a task with at most `AEX_DRAIN_DEADLINE_MS` left to live; it
    // is refused so the client reconnects to a task that can honour it. A new
    // unary request is milliseconds of work that finishes comfortably inside the
    // same deadline, and refusing it would turn every rolling deployment into a
    // burst of 503s during the window where the load balancer is still
    // deregistering this target.
    //
    // Both halves are covered by the same `/internal/readyz` 503, which is what
    // actually gets the target deregistered. This check is only about what to do
    // with the requests that arrive before that finishes.
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

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::time::Duration;

    use aex_regional_http::router::RouteOwner;
    use aex_wire::models::RotateReason;
    use aex_wire::routes::RouteId;
    use regional_observation_api::counters::{ReadCounter, ReadCounters};
    use regional_observation_api::ndjson;

    use super::{connection_class, render_frames};
    use crate::stream::{ConnectionClass, QuotaLimits, QuotaManager};

    #[tokio::test]
    async fn a_disconnect_releases_the_socket_quota_and_the_producer() {
        let counters = Arc::new(ReadCounters::default());
        let quotas = QuotaManager::new(QuotaLimits {
            total: 2,
            session: 2,
            observation: 2,
            per_workspace: 2,
        })
        .expect("consistent limits");
        let reservation = quotas
            .reserve("ws_one", ConnectionClass::Observation)
            .expect("capacity");
        assert_eq!(quotas.counts(), (1, 0, 1));
        let (mut sender, stream) =
            ndjson::channel(8, Duration::from_millis(50), Arc::clone(&counters));

        // The reader disconnects: the response body, the unfold state it holds
        // and the reservation inside that state are dropped together.
        drop(render_frames(stream, reservation));

        assert_eq!(
            quotas.counts(),
            (0, 0, 0),
            "a disconnect releases every class the socket charged"
        );
        assert!(
            !sender
                .send(&ndjson::rotate(None, RotateReason::ServerRotating, false))
                .await,
            "the producer is told to stop rather than left blocking on a gone reader"
        );
        assert_eq!(
            counters.total(ReadCounter::WriteStall),
            0,
            "a disconnect is not a stalled transport"
        );
    }

    #[test]
    fn every_owned_route_has_one_bounded_connection_class() {
        let routes = RouteOwner::Stream.routes();
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

    /// The generated contract now names `session-stream-api` for both halves.
    ///
    /// Merging the two deployables into one unit forced this: the release graph
    /// requires every `servingArtifact`/`servedArtifact` to resolve to a real
    /// unit id, and `regional-stream` stopped being one the moment its row left
    /// `release/units.toml`. What separates the two halves is therefore the
    /// transport the contract already declares per route — which is why this
    /// filter names the transport explicitly rather than letting an artifact
    /// string stand in for a mount strategy.
    #[test]
    fn mounted_routes_match_the_generated_actual_mount_authority() {
        let registry: serde_json::Value = serde_json::from_str(include_str!(
            "../../../../api/generated/registries/routes.json"
        ))
        .expect("generated route registry");
        let generated: Vec<RouteId> = registry["routes"]
            .as_array()
            .expect("route rows")
            .iter()
            .filter(|route| {
                route["servedArtifact"] == "session-stream-api" && route["transport"] == "ndjson"
            })
            .map(|route| {
                RouteId::parse(route["operationId"].as_str().expect("operation id"))
                    .expect("generated operation id")
            })
            .collect();
        assert_eq!(RouteOwner::Stream.routes(), generated);
        assert_eq!(
            generated.len(),
            24,
            "the stream half mounts 24 NDJSON routes"
        );
    }
}
