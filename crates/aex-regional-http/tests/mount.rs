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

/// `session-stream-api` is one deployable serving two owners.
///
/// Merging `regional-session-api` and `regional-stream` cost the artifact string
/// its ability to identify a mount strategy: the release graph requires every
/// `servingArtifact` to be a real unit id, so both halves now name the merged
/// unit. `route_owner` splits them by the transport the contract declares, and
/// this proves that split is total and disjoint rather than a lucky heuristic.
#[test]
fn the_two_halves_partition_the_merged_artifact_by_transport() {
    let merged: Vec<RouteId> = RouteId::ALL
        .iter()
        .copied()
        .filter(|id| {
            route(*id).plane == Plane::Regional
                && route(*id).serving_artifact == "session-stream-api"
        })
        .collect();
    assert!(!merged.is_empty(), "the merged unit serves regional routes");

    let unary = RouteOwner::SessionApi.routes();
    let ndjson = RouteOwner::Stream.routes();
    assert_eq!(
        unary.len() + ndjson.len(),
        merged.len(),
        "every route on the merged artifact belongs to exactly one half"
    );
    assert!(
        unary.iter().all(|id| !ndjson.contains(id)),
        "the two halves must not overlap"
    );
    for id in &unary {
        assert_ne!(
            route(*id).transport,
            aex_wire::routes::TransportKind::Ndjson,
            "`{id}` is a frame stream and belongs to the stream half"
        );
    }
    for id in &ndjson {
        assert_eq!(
            route(*id).transport,
            aex_wire::routes::TransportKind::Ndjson,
            "`{id}` is unary and belongs to the session half"
        );
    }

    // Both halves deploy as one artifact, which is exactly what merging means.
    assert_eq!(
        RouteOwner::SessionApi.deployable(),
        RouteOwner::Stream.deployable()
    );
    // A refusal still has to say which half it meant.
    assert_ne!(RouteOwner::SessionApi.half(), RouteOwner::Stream.half());
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
async fn a_route_owned_by_another_deployable_is_not_mounted() {
    let mounted = mount_unary(
        Arc::new(EchoDispatch(RouteOwner::SessionApi)),
        Arc::new(AlwaysAdmit),
        RequestLimits::DEFAULT,
    )
    .expect("mounts");
    // The secret edge's plaintext admission now carries its own first segment,
    // `/api/secrets/`, and the session API mounts no template under it. The
    // refusal is the router's own `404`, reached before any handler; it used to
    // be a `405` because `PUT` and `GET /api/workspace/secrets/{name}` shared
    // one template across the two deployables.
    for id in RouteOwner::SecretApi.routes() {
        let descriptor = route(id);
        let response = mounted
            .router
            .clone()
            .oneshot(
                Request::builder()
                    .method(descriptor.method.as_str())
                    .uri(concrete_path(id))
                    .header("content-type", "application/json")
                    .body(Body::from("{\"value\":\"x\"}"))
                    .expect("request"),
            )
            .await
            .expect("response");
        assert_eq!(
            response.status(),
            StatusCode::NOT_FOUND,
            "`{id}` is the secret edge's and must not answer on the session API"
        );
    }
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

#[tokio::test]
async fn the_declared_envelope_is_the_transport_body_ceiling() {
    // axum's default extractor cap is 2 MiB — five times smaller than the
    // 10 MiB provider envelope this crate declares in `envelope::ENVELOPE_BYTES`
    // and checks before authentication. The mount must raise the transport
    // limit to the declared ceiling, or a body inside the published contract is
    // refused at the extractor before admission ever measures it.
    let mounted = mount_unary(
        Arc::new(EchoDispatch(RouteOwner::SecretApi)),
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
                .uri(concrete_path(RouteId::SecretRevoke))
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
                .uri(concrete_path(RouteId::SecretRevoke))
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
    // Owned by the secret edge, served there, and not this deployable's: "no
    // such resource here" is the true statement, not "declared but not built".
    let refusal = not_served(RouteId::SecretDelete);
    assert_eq!(refusal.code, ErrorCode::NotFound);
    assert!(
        !RouteOwner::SessionApi
            .routes()
            .contains(&RouteId::SecretDelete),
        "the refused route is genuinely unmounted"
    );
}

#[test]
fn a_stub_for_a_deferred_route_answers_the_same_code_as_the_refusal_arm() {
    // The handler stub and the mounted arm must not disagree: a code the route
    // does not declare is rewritten to `internal_error` by `dispatch::declared`,
    // and only a deferred route declares `not_implemented`.
    //
    // The route is derived rather than named. This test named `secret_put` until
    // P3.3a landed it, at which point the first assertion was the only thing
    // standing between a served route and a silently vacuous check.
    let deferred = aex_wire::routes::ROUTES
        .iter()
        .find(|descriptor| descriptor.deferred)
        .expect("the contract still defers something");
    assert_eq!(not_served(deferred.id).code, ErrorCode::NotImplemented);
    assert!(route(deferred.id).declares(ErrorCode::NotImplemented));
}

/// An owned route the ledger defers is **mounted**, and it refuses honestly.
///
/// This replaces the assertion that it was absent. A bare `404` cannot be told
/// apart from a typo, a wrong base URL or a wrong region, and that ambiguity
/// lands in the first ten minutes of an integration.
#[tokio::test]
async fn a_deferred_route_answers_the_published_refusal() {
    let mounted = mount_unary(
        Arc::new(EchoDispatch(RouteOwner::SessionApi)),
        // Admission would refuse every request; the arm must answer before it.
        Arc::new(AlwaysRefuse),
        RequestLimits::DEFAULT,
    )
    .expect("mounts");
    let deferred = *mounted.refused.first().expect("the deployable owes routes");
    let descriptor = route(deferred);
    let response = mounted
        .router
        .clone()
        .oneshot(
            Request::builder()
                .method(descriptor.method.as_str())
                .uri(concrete_path(deferred))
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");
    assert_eq!(
        response.status(),
        StatusCode::NOT_IMPLEMENTED,
        "`{deferred}`"
    );
    let body = axum::body::to_bytes(response.into_body(), 64 * 1024)
        .await
        .expect("body");
    let envelope: aex_wire::error::ApiError = serde_json::from_slice(&body).expect("envelope");
    assert_eq!(
        envelope.error.code,
        aex_wire::error::ObservedErrorCode::Known(ErrorCode::NotImplemented)
    );
    assert!(!envelope.error.request_id.is_empty());
    assert!(!envelope.error.retryable);
    // No reason text: the ledger's prose is an engineering note, and one that
    // has drifted is worse than none.
    assert_eq!(
        envelope.error.message,
        ErrorCode::NotImplemented.default_message()
    );

    // A method the template does not publish at all is the router's `405`, not
    // a `404`. It has to be derived: several deferred templates publish three of
    // the four verbs — `/api/workspace/files/{name}` defers GET, PUT *and*
    // DELETE — so simply flipping to "the other method" picks another published
    // deferred operation, whose honest answer is the `501` arm and not a `405`.
    let published: Vec<_> = aex_wire::routes::ROUTES
        .iter()
        .filter(|other| other.template == descriptor.template)
        .map(|other| other.method)
        .collect();
    let wrong = [
        aex_wire::types::HttpMethod::Get,
        aex_wire::types::HttpMethod::Put,
        aex_wire::types::HttpMethod::Post,
        aex_wire::types::HttpMethod::Delete,
    ]
    .into_iter()
    .find(|method| !published.contains(method))
    .expect("a template that publishes every verb has no method to refuse");
    let response = mounted
        .router
        .oneshot(
            Request::builder()
                .method(wrong.as_str())
                .uri(concrete_path(deferred))
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");
    assert_eq!(response.status(), StatusCode::METHOD_NOT_ALLOWED);
}

/// The totality check: every owned route is served here or deferred by the
/// contract, and a composition that accounts for neither refuses to build.
#[test]
fn an_owned_route_in_neither_set_fails_composition() {
    struct Forgetful;
    #[async_trait::async_trait]
    impl UnaryDispatch for Forgetful {
        fn owner(&self) -> RouteOwner {
            RouteOwner::SecretApi
        }
        fn served(&self) -> Vec<RouteId> {
            // Drops `secret_revoke`, which the ledger does not defer.
            RouteOwner::SecretApi
                .routes()
                .into_iter()
                .filter(|id| !route(*id).deferred && *id != RouteId::SecretRevoke)
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
            route: "secret_revoke",
            deployable: RouteOwner::SecretApi.half(),
        }
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
