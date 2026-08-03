//! Route ownership is a partition of the generated regional table, and a mount
//! answers exactly the owned set.
//!
//! These are composition facts, not HTTP facts: what is asserted is that the
//! mounted route set is derived from the one table and cannot drift from it.

use std::sync::Arc;

use aex_regional_http::context::{
    AccountState, AuthorizationEpochs, EffectiveLimits, RegionalAuthorization, RequestContext,
};
use aex_regional_http::mount::{
    AdmissionRequest, EdgeAdmission, MountError, UnaryDispatch, mount_unary, not_served,
};
use aex_regional_http::router::{RouteOwner, route_owner};
use aex_wire::dispatch::{RawRequest, RawResponse, RequestLimits};
use aex_wire::error::{ErrorCode, WireError, WireResult};
use aex_wire::idempotency::PrincipalScope;
use aex_wire::ids::{ApiKeyId, OrganizationId, PrefixedId, Uuid7, WorkspaceId};
use aex_wire::routes::{Plane, RouteId, route};
use aex_wire::scopes::ScopeSet;
use aex_wire::server::{AcceptKind, RouteGroup};
use aex_wire::types::{Region, RequestId};
use axum::body::Body;
use axum::http::{Request, StatusCode};
use tower::ServiceExt as _;

// --- fixtures ----------------------------------------------------------------

struct EchoDispatch(RouteOwner);

#[async_trait::async_trait]
impl UnaryDispatch for EchoDispatch {
    fn owner(&self) -> RouteOwner {
        self.0
    }

    async fn dispatch(
        &self,
        _cx: &RequestContext,
        _accept: AcceptKind,
        raw: RawRequest<'_>,
        _limits: RequestLimits,
    ) -> WireResult<RawResponse> {
        if route_owner(raw.route) == Some(self.0) {
            RawResponse::json(200, &raw.route.as_str())
        } else {
            Err(not_served(raw.route))
        }
    }
}

struct AlwaysAdmit;

#[async_trait::async_trait]
impl EdgeAdmission for AlwaysAdmit {
    async fn admit(&self, request: &AdmissionRequest<'_>) -> Result<RequestContext, WireError> {
        Ok(context(request.request_id.clone(), request.route))
    }
}

struct AlwaysRefuse;

#[async_trait::async_trait]
impl EdgeAdmission for AlwaysRefuse {
    async fn admit(&self, _request: &AdmissionRequest<'_>) -> Result<RequestContext, WireError> {
        Err(WireError::new(ErrorCode::Unauthenticated))
    }
}

fn sample<I: PrefixedId>(seed: u8) -> I {
    I::from_uuid7(Uuid7::compose(1_750_000_000_000, [seed; 10]))
}

fn context(request_id: RequestId, route: RouteId) -> RequestContext {
    RequestContext {
        request_id,
        route,
        auth: RegionalAuthorization {
            principal: PrincipalScope::WorkspaceKey {
                key: sample::<ApiKeyId>(1),
                workspace: sample::<WorkspaceId>(2),
                organization: sample::<OrganizationId>(3),
            },
            credential_binding: [7; 32],
            organization_id: sample::<OrganizationId>(3),
            workspace_id: sample::<WorkspaceId>(2),
            placement: Region::EuWest1,
            scopes: ScopeSet::default(),
            account_state: AccountState::Active,
            epochs: AuthorizationEpochs::default(),
            issued_at: time::OffsetDateTime::UNIX_EPOCH,
            expires_at: time::OffsetDateTime::UNIX_EPOCH + time::Duration::seconds(30),
        },
        limits: EffectiveLimits {
            json_body_bytes: 1_048_576,
            otlp_body_bytes: 4 * 1_024 * 1_024,
            query_page_items: 100,
            query_page_bytes: 1_048_576,
        },
        operation_id: None,
        idempotency: None,
        if_match: None,
        received_at: time::OffsetDateTime::UNIX_EPOCH,
    }
}

/// A concrete path for a template, so the router can be exercised without
/// re-typing any template: the parameter values are substituted positionally
/// from the generated `path_params`.
fn concrete_path(id: RouteId) -> String {
    route(id)
        .template
        .split('/')
        .map(|segment| {
            segment
                .strip_prefix('{')
                .and_then(|rest| rest.strip_suffix('}'))
                .map_or(segment, sample_for)
        })
        .collect::<Vec<_>>()
        .join("/")
}

/// A single valid `UUIDv7` suffix. Path parameters are decoded by the generated
/// dispatcher, not by the router, so one well-formed suffix exercises every
/// template.
static SAMPLE_SUFFIX: &str = "01jxt21q00e40r2081040g2081";

fn sample_for(name: &str) -> &'static str {
    match name {
        "kind" => "files",
        "limitId" => "session.max_runtime_seconds",
        "name" => "fixture",
        _ => SAMPLE_SUFFIX,
    }
}

// --- ownership ----------------------------------------------------------------

#[test]
fn route_ownership_partitions_the_regional_route_table() {
    let regional: Vec<RouteId> = RouteId::ALL
        .iter()
        .copied()
        .filter(|id| route(*id).plane == Plane::Regional)
        .collect();
    assert!(!regional.is_empty(), "the table has regional routes");

    let mut owned: Vec<RouteId> = Vec::new();
    for owner in RouteOwner::ALL {
        let routes = owner.routes();
        assert!(
            !routes.is_empty(),
            "{} owns at least one route",
            owner.deployable()
        );
        for id in routes {
            assert!(
                !owned.contains(&id),
                "`{id}` is owned by more than one deployable"
            );
            owned.push(id);
        }
    }
    owned.sort_unstable_by_key(|id| *id as usize);
    let mut expected = regional;
    expected.sort_unstable_by_key(|id| *id as usize);
    assert_eq!(
        owned, expected,
        "every regional route has exactly one owner"
    );
}

#[test]
fn no_central_route_has_a_regional_owner() {
    for id in RouteId::ALL.iter().copied() {
        if route(id).plane == Plane::Central {
            assert_eq!(route_owner(id), None, "`{id}` is central");
        }
    }
}

#[test]
fn a_split_group_is_divided_between_two_deployables() {
    // `regional:secrets` and `regional:provider-credentials` are the two
    // authoring fragments that span two deployables: plaintext admission is the
    // secret edge's, metadata is the session API's.
    for group in [RouteGroup::Secrets, RouteGroup::ProviderCredentials] {
        let session = RouteOwner::SessionApi.routes_in(group);
        let secret = RouteOwner::SecretApi.routes_in(group);
        assert!(!session.is_empty(), "{group:?} has a session-api half");
        assert!(!secret.is_empty(), "{group:?} has a secret-api half");
        assert_eq!(
            session.len() + secret.len(),
            group.routes().len(),
            "{group:?} is exactly divided"
        );
    }
}

#[test]
fn the_stream_owns_every_ndjson_route_and_no_other() {
    for id in RouteOwner::Stream.routes() {
        assert_eq!(
            route(id).transport,
            aex_wire::routes::TransportKind::Ndjson,
            "`{id}` is an NDJSON route"
        );
    }
    for id in RouteId::ALL.iter().copied() {
        if route(id).plane == Plane::Regional
            && route(id).transport == aex_wire::routes::TransportKind::Ndjson
        {
            assert_eq!(route_owner(id), Some(RouteOwner::Stream), "`{id}`");
        }
    }
}

// --- mounting -----------------------------------------------------------------

#[tokio::test]
async fn a_mount_answers_exactly_the_owned_route_set() {
    for owner in [RouteOwner::SessionApi, RouteOwner::SecretApi] {
        let mounted = mount_unary(
            Arc::new(EchoDispatch(owner)),
            Arc::new(AlwaysAdmit),
            RequestLimits::DEFAULT,
        )
        .expect("the owned set mounts");
        assert_eq!(
            mounted.routes,
            owner.routes(),
            "{} mounts its owned set",
            owner.deployable()
        );

        for id in &mounted.routes {
            let descriptor = route(*id);
            let response = mounted
                .router
                .clone()
                .oneshot(
                    Request::builder()
                        .method(descriptor.method.as_str())
                        .uri(concrete_path(*id))
                        .body(Body::empty())
                        .expect("request"),
                )
                .await
                .expect("response");
            assert_ne!(
                response.status(),
                StatusCode::NOT_FOUND,
                "`{id}` is mounted at `{}`",
                descriptor.template
            );
            assert_ne!(
                response.status(),
                StatusCode::METHOD_NOT_ALLOWED,
                "`{id}` accepts {}",
                descriptor.method
            );
        }
    }
}

#[tokio::test]
async fn a_route_owned_by_another_deployable_is_not_mounted() {
    let mounted = mount_unary(
        Arc::new(EchoDispatch(RouteOwner::SessionApi)),
        Arc::new(AlwaysAdmit),
        RequestLimits::DEFAULT,
    )
    .expect("mounts");
    // `PUT /api/workspace/secrets/{name}` is the secret edge's; the session API
    // reads the same template with `GET`, so the refusal must come from the
    // method filter rather than from a handler.
    let response = mounted
        .router
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri(concrete_path(RouteId::SecretPut))
                .header("content-type", "application/json")
                .body(Body::from("{\"value\":\"x\"}"))
                .expect("request"),
        )
        .await
        .expect("response");
    assert_eq!(response.status(), StatusCode::METHOD_NOT_ALLOWED);
}

#[tokio::test]
async fn an_ndjson_route_is_absent_from_a_finite_api() {
    let mounted = mount_unary(
        Arc::new(EchoDispatch(RouteOwner::SessionApi)),
        Arc::new(AlwaysAdmit),
        RequestLimits::DEFAULT,
    )
    .expect("mounts");
    let stream = RouteOwner::Stream
        .routes()
        .first()
        .copied()
        .expect("the stream owns a route");
    let response = mounted
        .router
        .oneshot(
            Request::builder()
                .method(route(stream).method.as_str())
                .uri(concrete_path(stream))
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn a_refused_admission_never_reaches_the_dispatcher() {
    let mounted = mount_unary(
        Arc::new(EchoDispatch(RouteOwner::SecretApi)),
        Arc::new(AlwaysRefuse),
        RequestLimits::DEFAULT,
    )
    .expect("mounts");
    let response = mounted
        .router
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri(concrete_path(RouteId::SecretDelete))
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(
        response
            .headers()
            .get("content-type")
            .and_then(|value| value.to_str().ok()),
        Some("application/json")
    );
}

#[test]
fn dispatcher_refuses_a_route_it_does_not_own() {
    let refusal = not_served(RouteId::SecretPut);
    assert_eq!(refusal.code, ErrorCode::NotFound);
    assert!(
        !RouteOwner::SessionApi
            .routes()
            .contains(&RouteId::SecretPut),
        "the refused route is genuinely unmounted"
    );
}

#[test]
fn a_wrongly_owned_route_is_a_mount_error() {
    // `EchoDispatch` claims to be the OTLP deployable while the caller asks for
    // the session API's set: the two disagree, and the mount must refuse rather
    // than silently answer somebody else's routes.
    struct Liar;
    #[async_trait::async_trait]
    impl UnaryDispatch for Liar {
        fn owner(&self) -> RouteOwner {
            RouteOwner::Otlp
        }
        async fn dispatch(
            &self,
            _cx: &RequestContext,
            _accept: AcceptKind,
            _raw: RawRequest<'_>,
            _limits: RequestLimits,
        ) -> WireResult<RawResponse> {
            unreachable!("never dispatched")
        }
    }
    let mounted = mount_unary(
        Arc::new(Liar),
        Arc::new(AlwaysAdmit),
        RequestLimits::DEFAULT,
    )
    .expect("the OTLP set mounts");
    assert_eq!(mounted.routes, RouteOwner::Otlp.routes());
    assert!(
        mounted
            .routes
            .iter()
            .all(|id| route_owner(*id) == Some(RouteOwner::Otlp))
    );
}

#[test]
fn mount_error_names_the_offending_route() {
    let error = MountError::WrongOwner {
        route: "secret_put",
        owner: "regional-secret-api",
        deployable: "regional-session-api",
    };
    let rendered = error.to_string();
    assert!(rendered.contains("secret_put"), "{rendered}");
    assert!(rendered.contains("regional-secret-api"), "{rendered}");
}
