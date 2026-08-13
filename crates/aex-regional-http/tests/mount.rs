//! Route ownership is a partition of the generated regional table, and a mount
//! answers exactly the owned set.
//!
//! These are composition facts, not HTTP facts: what is asserted is that the
//! mounted route set is derived from the one table and cannot drift from it.

use std::sync::Arc;

use aex_regional_http::context::{
    AccountState, AuthorizationEpochs, EffectiveLimits, RegionalAuthorization, RequestContext,
};
use aex_regional_http::envelope::ENVELOPE_BYTES;
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

    /// The production narrowing, derived exactly as every composition derives
    /// it: everything owned that the contract does not defer.
    fn served(&self) -> Vec<RouteId> {
        self.0
            .routes()
            .into_iter()
            .filter(|id| !route(*id).deferred)
            .collect()
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

/// The regional launch surface is one finite API owner.
#[test]
fn the_session_api_owns_every_finite_regional_route() {
    let regional: Vec<RouteId> = RouteId::ALL
        .iter()
        .copied()
        .filter(|id| route(*id).plane == Plane::Regional)
        .collect();
    assert!(
        !regional.is_empty(),
        "the finite API serves regional routes"
    );
    assert_eq!(RouteOwner::SessionApi.routes(), regional);
    for id in RouteOwner::SessionApi.routes() {
        assert_eq!(route_owner(id), Some(RouteOwner::SessionApi));
        assert_eq!(route(id).serving_artifact, "session-stream-api");
        assert_ne!(
            route(id).transport,
            aex_wire::routes::TransportKind::Ndjson,
            "`{id}` must remain finite"
        );
    }
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
fn the_session_api_owns_the_complete_provider_credential_group() {
    let group = RouteGroup::ProviderCredentials;
    let session = RouteOwner::SessionApi.routes_in(group);
    assert_eq!(session, group.routes(), "{group:?} has one public owner");
}

// --- mounting -----------------------------------------------------------------

#[tokio::test]
async fn a_mount_answers_exactly_the_owned_route_set() {
    for owner in [RouteOwner::SessionApi] {
        let mounted = mount_unary(
            Arc::new(EchoDispatch(owner)),
            Arc::new(AlwaysAdmit),
            RequestLimits::DEFAULT,
        )
        .expect("the owned set mounts");
        let mut answered = mounted.routes.clone();
        answered.extend(mounted.refused.iter().copied());
        answered.sort_unstable();
        assert_eq!(
            answered,
            owner.routes(),
            "{} answers its whole owned set, serving or refusing",
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
async fn a_refused_admission_never_reaches_the_dispatcher() {
    let mounted = mount_unary(
        Arc::new(EchoDispatch(RouteOwner::SessionApi)),
        Arc::new(AlwaysRefuse),
        RequestLimits::DEFAULT,
    )
    .expect("mounts");
    let response = mounted
        .router
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(concrete_path(RouteId::ProviderCredentialRegister))
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

#[tokio::test]
async fn the_declared_envelope_is_the_transport_body_ceiling() {
    // axum's default extractor cap is 2 MiB — five times smaller than the
    // 10 MiB provider envelope this crate declares in `envelope::ENVELOPE_BYTES`
    // and checks before authentication. The mount must raise the transport
    // limit to the declared ceiling, or a body inside the published contract is
    // refused at the extractor before admission ever measures it.
    let mounted = mount_unary(
        Arc::new(EchoDispatch(RouteOwner::SessionApi)),
        Arc::new(AlwaysAdmit),
        RequestLimits::DEFAULT,
    )
    .expect("mounts");

    // A served route with a body: the refusal arm never reads one, so it could
    // not exercise the extractor's ceiling at all.
    let inside = mounted
        .router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(concrete_path(RouteId::ProviderCredentialRegister))
                .header("content-type", "application/json")
                .body(Body::from(vec![b'x'; 3 * 1024 * 1024]))
                .expect("request"),
        )
        .await
        .expect("response");
    assert_ne!(
        inside.status(),
        StatusCode::PAYLOAD_TOO_LARGE,
        "a 3 MiB body is inside the declared envelope and must reach admission"
    );

    let outside = mounted
        .router
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(concrete_path(RouteId::ProviderCredentialRegister))
                .header("content-type", "application/json")
                .body(Body::from(vec![b'x'; ENVELOPE_BYTES + 1]))
                .expect("request"),
        )
        .await
        .expect("response");
    assert_eq!(
        outside.status(),
        StatusCode::PAYLOAD_TOO_LARGE,
        "above the declared envelope the transport still refuses"
    );
}

#[test]
fn dispatcher_refuses_a_route_it_does_not_own() {
    // Central routes are outside this regional mount: "no such resource here"
    // is the true statement, not "declared but not built".
    let foreign = RouteId::ApiKeysList;
    let refusal = not_served(foreign);
    assert_eq!(refusal.code, ErrorCode::NotFound);
    assert_eq!(route_owner(foreign), None);
    assert!(
        !RouteOwner::SessionApi.routes().contains(&foreign),
        "the refused route is genuinely unmounted"
    );
}

#[test]
fn the_launch_contract_has_no_deferred_routes() {
    assert!(
        aex_wire::routes::ROUTES
            .iter()
            .all(|descriptor| !descriptor.deferred),
        "the launch contract must not regain an unimplemented route"
    );
    assert!(
        aex_wire::routes::ROUTES
            .iter()
            .all(|descriptor| !descriptor.declares(ErrorCode::NotImplemented)),
        "a complete route must not publish `not_implemented`"
    );
}

#[test]
fn a_complete_owner_mounts_no_refusal_arms() {
    let mounted = mount_unary(
        Arc::new(EchoDispatch(RouteOwner::SessionApi)),
        Arc::new(AlwaysRefuse),
        RequestLimits::DEFAULT,
    )
    .expect("mounts");
    assert!(mounted.refused.is_empty());
    assert_eq!(mounted.routes, RouteOwner::SessionApi.routes());
}

/// The totality check: every owned route is served here or deferred by the
/// contract, and a composition that accounts for neither refuses to build.
#[test]
fn an_owned_route_in_neither_set_fails_composition() {
    struct Forgetful;
    #[async_trait::async_trait]
    impl UnaryDispatch for Forgetful {
        fn owner(&self) -> RouteOwner {
            RouteOwner::SessionApi
        }
        fn served(&self) -> Vec<RouteId> {
            // Drops a route the ledger does not defer.
            RouteOwner::SessionApi
                .routes()
                .into_iter()
                .filter(|id| !route(*id).deferred && *id != RouteId::ProviderCredentialGet)
                .collect()
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
    let error = mount_unary(
        Arc::new(Forgetful),
        Arc::new(AlwaysAdmit),
        RequestLimits::DEFAULT,
    )
    .expect_err("an unaccounted route must fail composition");
    assert_eq!(
        error,
        MountError::Unaccounted {
            route: "provider_credential_get",
            deployable: RouteOwner::SessionApi.half(),
        }
    );
}
