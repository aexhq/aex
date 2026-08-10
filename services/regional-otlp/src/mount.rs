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
use aex_wire::routes::{Plane, RouteId, TransportKind, match_route, route};
use aex_wire::server::{RouteGroup, dispatch_otlp};
use aex_wire::types::HttpMethod;
use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode, Uri, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{MethodFilter, on};

use crate::admission::{OtlpRequest, OtlpService};
use aex_internal_contracts::assertion::AssertionAudience;
use aex_regional_http::authz::RegionalProjection;
use aex_regional_http::edge::{RegionalEdge, SystemClock};
use aex_regional_http::mount::{AdmissionRequest, EdgeAdmission as _};

/// The group this deployable owns, and the only one it mounts.
pub const GROUP: RouteGroup = RouteGroup::Otlp;

/// The header a caller attributes its batch to a session with.
///
/// It is the caller's own claim and nothing verifies it. That is deliberate:
/// see [`session_id`].
pub const SESSION_HEADER: &str = "aex-session-id";

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
        // The registry pins a 4 MiB encoded OTLP ceiling, but `axum`'s
        // extractor default is 2 MiB — without this layer a 2–4 MiB batch died
        // as a bare framework `413` before the handler ever ran, despite the
        // pinned contract. The layer only raises the framework guard to the
        // configured ceiling; the admission path still enforces the exact
        // per-workspace bound with the typed refusal.
        .layer(axum::extract::DefaultBodyLimit::max(
            state.limits.max_otlp_body_bytes,
        ))
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
    // Read strictly after the edge admitted the request, so an unauthenticated
    // caller is refused as one and never learns anything from the syntax of a
    // header it was never entitled to send.
    let session = match session_id(&headers) {
        Ok(session) => session,
        Err(failure) => return render_error(Some(&context), &failure),
    };
    // Three facts come off this request's headers: what the bytes are encoded
    // as, how they are compressed, and which session the caller says they
    // belong to. Only the workspace half of the attribution comes from
    // `authorized`, and it is the half that decides the partition — see
    // `OtlpRequest::scope`.
    let request = OtlpRequest::new(
        Arc::clone(&state.service),
        authorized,
        encoding,
        coding,
        session,
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

/// Resolves the session the caller attributes this batch to.
///
/// The value is parsed as a [`SessionId`] and used as-is. Nothing here proves
/// the session exists, and nothing here proves the caller owns it — an ingest
/// path carrying the platform's whole telemetry volume does not get to spend a
/// session→workspace read per request to establish a fact the storage key
/// already encodes.
///
/// What makes an unverified identifier safe is the key it lands in.
/// `ScopeKey::Session` renders `S#{workspace}#{session}` with the workspace the
/// credential proved, so the only partitions a caller can reach are inside its
/// own workspace. A wrong session id therefore files a batch against the
/// caller's own wrong session, which is that customer's problem to fix, and can
/// never file it against another tenant's.
///
/// An earlier revision took this header and used it **as** the scope, when a
/// session scope rendered `S#{session}` and named no workspace at all. That let
/// any caller write into any session of any workspace, and removing the header
/// was the correct fix for the key shape of the day.
///
/// # Errors
///
/// Returns an [`ErrorCode::InvalidRequest`] `400` when the header is present and
/// is not a session identifier. Absent means workspace scope; a malformed value
/// is never silently treated as absent.
fn session_id(headers: &HeaderMap) -> Result<Option<aex_wire::ids::SessionId>, WireError> {
    use aex_wire::ids::PrefixedId as _;

    let Some(value) = headers.get(SESSION_HEADER) else {
        return Ok(None);
    };
    value
        .to_str()
        .ok()
        .and_then(|text| aex_wire::ids::SessionId::parse(text).ok())
        .map(Some)
        .ok_or_else(|| {
            WireError::new(ErrorCode::InvalidRequest)
                .with_message(format!("`{SESSION_HEADER}` must be a session identifier"))
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
    use std::sync::Arc;
    use std::time::Duration;

    use aex_otlp_admission::{MemoryBudget, OtlpLimits};
    use aex_wire::dispatch::RequestLimits;
    use aex_wire::routes::{BodyClass, Plane, RouteId, TransportKind, route};
    use aex_wire::server::RouteGroup;
    use axum::http::StatusCode;

    use super::{AUDIENCE, AppState, GROUP, mounted_templates, owned_routes};
    use crate::admission::OtlpService;
    use crate::authority::AdmissionAuthority;
    use crate::counters::AdmissionTelemetry;

    /// A fully composed state whose clients never reach a network.
    ///
    /// The two body-limit cases below never get past admission stage 1–2: an
    /// over-limit body is refused by the framework guard before the handler
    /// runs, and an in-limit body carries no credential so the edge refuses it
    /// locally before any provider client is exercised.
    fn offline_state() -> Arc<AppState> {
        let region = aex_wire::types::Region::from_name("eu-west-1").expect("a region");
        let dynamodb = aws_sdk_dynamodb::Client::from_conf(
            aws_sdk_dynamodb::Config::builder()
                .behavior_version(aws_sdk_dynamodb::config::BehaviorVersion::latest())
                .region(aws_sdk_dynamodb::config::Region::new("eu-west-1"))
                .build(),
        );
        let s3 = aws_sdk_s3::Client::from_conf(
            aws_sdk_s3::Config::builder()
                .behavior_version(aws_sdk_s3::config::BehaviorVersion::latest())
                .region(aws_sdk_s3::config::Region::new("eu-west-1"))
                .build(),
        );
        let peppers = aex_regional_http::authz::parse_pepper_ring(
            "test-pepper-ring",
            &format!(
                r#"{{"schemaVersion":1,"peppers":[{{"version":1,"material":"{}"}}]}}"#,
                <base64::engine::general_purpose::GeneralPurpose as base64::Engine>::encode(
                    &base64::engine::general_purpose::URL_SAFE_NO_PAD,
                    [3_u8; 32],
                )
            ),
        )
        .expect("a usable ring");
        let edge = aex_regional_http::edge::RegionalEdge::new(
            peppers,
            aex_regional_http::authz::RegionalProjection::new(
                aex_session_dynamodb::projection::ProjectionReader::new(
                    dynamodb.clone(),
                    "test-authz-projection".to_owned(),
                ),
                region,
            ),
            aex_regional_http::edge::SystemClock,
            aex_regional_http::edge::EdgeBinding {
                audience: AUDIENCE,
                region,
            },
        );
        let service = OtlpService::new(
            AdmissionAuthority::new(
                dynamodb,
                s3,
                "test-observation-authority",
                "test-observation-bucket",
                region,
            ),
            OtlpLimits::REGISTERED,
            MemoryBudget::new(32 * 1024 * 1024),
            Duration::from_millis(5),
            AdmissionTelemetry::new(
                aex_platform_telemetry::Handle::install(
                    &aex_platform_telemetry::Settings::default(),
                    None,
                ),
                "dev",
                "eu-west-1",
            ),
        );
        Arc::new(AppState {
            edge: Arc::new(edge),
            service: Arc::new(service),
            limits: RequestLimits {
                max_json_body_bytes: RequestLimits::DEFAULT_JSON_BODY_BYTES,
                max_otlp_body_bytes: OtlpLimits::REGISTERED.encoded_max,
            },
            ready: true,
            release_digest: "test".to_owned(),
        })
    }

    /// One OTLP ingest request carrying `bytes` of body and no credential.
    fn ingest_request(bytes: usize) -> axum::http::Request<axum::body::Body> {
        let template = route(owned_routes()[0]).template;
        axum::http::Request::builder()
            .method("POST")
            .uri(template)
            .header("content-type", "application/x-protobuf")
            .body(axum::body::Body::from(vec![0u8; bytes]))
            .expect("a request builds")
    }

    #[tokio::test]
    async fn the_mount_admits_bodies_up_to_the_pinned_ceiling_and_refuses_past_it() {
        use tower::ServiceExt as _;
        let router = super::router(offline_state());

        // Both sizes are derived from the ceiling rather than written down, so
        // moving the ceiling moves the test with it. The explicit body limit
        // still has to be set: `axum`'s extractor default is its own number and
        // whether it sits above or below the contract is not ours to rely on.
        let ceiling = aex_otlp_admission::OtlpLimits::REGISTERED.encoded_max;
        let inside = router
            .clone()
            .oneshot(ingest_request(ceiling))
            .await
            .expect("served");
        assert_ne!(
            inside.status(),
            StatusCode::PAYLOAD_TOO_LARGE,
            "a body inside the pinned contract must reach admission, not die \
             at the framework guard"
        );

        // Past the configured ceiling the framework guard still refuses.
        let refused = router
            .oneshot(ingest_request(ceiling + 1))
            .await
            .expect("served");
        assert_eq!(refused.status(), StatusCode::PAYLOAD_TOO_LARGE);
    }

    /// This replaces `a_caller_supplied_session_header_is_not_read_and_cannot_
    /// fail_a_request`, which pinned that `aex-session-id` was never read. That
    /// is no longer true and is no longer what protects a tenant.
    ///
    /// What protects a tenant is the key. `ScopeKey::Session` renders the
    /// workspace before the session, so an identifier a caller invents — or
    /// copies from another customer — resolves inside the workspace its
    /// credential proved and nowhere else. Asserted here on the key itself
    /// rather than through the router, so it constrains **every** caller of the
    /// scope: the reader, the exporter, the reconciler and the stream, not just
    /// this one handler.
    #[test]
    fn a_session_id_from_one_workspace_can_never_address_another_workspaces_partition() {
        use aex_observation_domain::keys::{self, BucketHour, ScopeKey};
        use aex_observation_domain::signal::Signal;
        use aex_wire::ids::{PrefixedId as _, SessionId, WorkspaceId};

        // One session identifier, presented under two different credentials.
        let session = SessionId::from_uuid7(aex_wire::Uuid7::compose(1, [4; 10]));
        let mine = WorkspaceId::from_uuid7(aex_wire::Uuid7::compose(1, [2; 10]));
        let theirs = WorkspaceId::from_uuid7(aex_wire::Uuid7::compose(1, [9; 10]));
        let ours = ScopeKey::Session {
            workspace: mine,
            session,
        };
        let stranger = ScopeKey::Session {
            workspace: theirs,
            session,
        };

        assert_eq!(ours.session(), stranger.session(), "the same session id");
        assert_ne!(ours, stranger);
        assert_eq!(ours.workspace(), mine);
        assert_eq!(ours.to_key(), format!("S#{mine}#{session}"));

        let bucket = BucketHour::parse("2026-08-01T09").expect("a bucket");
        let mut partitions = vec![
            (keys::frontier_pk(&ours), keys::frontier_pk(&stranger)),
            (keys::gap_pk(&ours), keys::gap_pk(&stranger)),
            (
                keys::segment_pk(&ours, Signal::Logs),
                keys::segment_pk(&stranger, Signal::Logs),
            ),
            (
                keys::time_segment_pk(&ours, Signal::Logs),
                keys::time_segment_pk(&stranger, Signal::Logs),
            ),
        ];
        for signal in Signal::AUTHORITY {
            partitions.push((
                keys::observation_pk(&ours, *signal, bucket, 0),
                keys::observation_pk(&stranger, *signal, bucket, 0),
            ));
        }
        for (ours, theirs_key) in partitions {
            assert_ne!(
                ours, theirs_key,
                "one session id reached two workspaces' partitions"
            );
            assert!(
                ours.contains(&mine.to_string()) && !ours.contains(&theirs.to_string()),
                "`{ours}` must name its own workspace and no other"
            );
        }
    }

    #[test]
    fn the_session_header_is_admitted_and_a_malformed_one_is_a_four_hundred() {
        use axum::http::{HeaderMap, HeaderValue};

        let mut headers = HeaderMap::new();
        assert_eq!(
            super::session_id(&headers).expect("an absent header is not a failure"),
            None,
            "absent means workspace scope, exactly as before"
        );

        headers.insert(
            super::SESSION_HEADER,
            HeaderValue::from_static("ses_01h455vb4pex5vsknk084sn02q"),
        );
        let parsed = super::session_id(&headers)
            .expect("a well-formed session parses")
            .expect("a present header yields a session");
        assert_eq!(parsed.to_string(), "ses_01h455vb4pex5vsknk084sn02q");

        // A value nothing parses is refused rather than quietly dropped back to
        // workspace scope, which would file the batch somewhere the caller did
        // not ask for and tell them nothing.
        for hostile in [
            "not-a-session-identifier",
            "wsp_01h455vb4pex5vsknk084sn02q",
            "",
        ] {
            headers.insert(
                super::SESSION_HEADER,
                HeaderValue::from_str(hostile).expect("a header value"),
            );
            let failure = super::session_id(&headers).expect_err("a malformed session is refused");
            assert_eq!(failure.code.http_status(), 400, "`{hostile}` must be a 400");
        }
    }

    #[tokio::test]
    async fn the_session_header_never_overtakes_the_credential_check() {
        use tower::ServiceExt as _;
        // Precedence, unchanged by re-admitting the header: an unauthenticated
        // request is refused as unauthenticated. A caller must not be able to
        // probe this deployable's header parsing without a credential.
        let router = super::router(offline_state());
        let without = router
            .clone()
            .oneshot(ingest_request(64))
            .await
            .expect("served");
        let without_status = without.status();
        assert_ne!(without_status, StatusCode::OK);

        for value in ["ses_01h455vb4pex5vsknk084sn02q", "not-a-session-identifier"] {
            let mut request = ingest_request(64);
            request.headers_mut().insert(
                super::SESSION_HEADER,
                axum::http::HeaderValue::from_static(value),
            );
            let with = router.clone().oneshot(request).await.expect("served");
            assert_eq!(
                with.status(),
                without_status,
                "`{}: {value}` changed the outcome before the credential was judged",
                super::SESSION_HEADER
            );
        }
    }

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
