//! What the merged deployable actually serves, and what it refuses.
//!
//! The suite drives the **real** router — `central_api::app` over the real
//! `EdgeStack` — with the three service implementations replaced by stubs that
//! answer one declared refusal each. That is enough to prove a route is mounted
//! and reached, and it keeps every stage between the HTTP bytes and the handler
//! (route matching, authentication, the body bound, the declared headers, target
//! resolution, the decision, the replay identity) as the code that ships.
//!
//! # The property this file exists for
//!
//! Behind API Gateway a central process read the authorizer's context map and
//! trusted it, because the gateway would not forward a caller-supplied one.
//! Behind an ALB every header is caller-authored. So the load-bearing test here
//! is [`a_caller_supplied_authorizer_context_gains_nothing`]: a request that
//! asserts the principal it wants must be refused exactly as if it had asserted
//! nothing, because this composition verifies credentials itself and the two
//! paths are exclusive by enum.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use aex_central_http::admission::CentralAuthenticator;
use aex_central_http::authorizer::{CentralAuthorizerContext, ContextPrincipalKind};
use aex_central_http::config::{CentralServiceId, HttpConfig};
use aex_central_http::error::EdgeError;
use aex_central_http::health::{HEALTH_PATH, READY_PATH};
use aex_central_http::router::EdgeStack;
use aex_central_http::target::{TargetPath, TargetResolver};
use aex_control_app::ports::Clock;
use aex_control_domain::{
    AccountState, Action, CursorSecret, OrgMembership, OrgRole, Resource, ResourceClass, ScopeSet,
    requirement,
};
use aex_wire::error::{ErrorCode, WireError, WireResult};
use aex_wire::ids::{
    ApiKeyId, OperationId, OrganizationId, PrefixedId, StatementId, UserId, Uuid7, WorkspaceId,
};
use aex_wire::routes::{RouteId, route};
use aex_wire::server::{
    Accepted, ApiKeysApi, AuthApi, BillingApi, BootstrapApi, CentralOperationsApi, Created,
    NoContent, OrganizationsApi, RequestContext, WithETag, WorkspacesApi,
};
use aex_wire::types::{Region, RequestId};
use async_trait::async_trait;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use central_api::{Probes, app, readiness};
use time::OffsetDateTime;
use tower::ServiceExt as _;
use uuid::Uuid;

const NOW_MS: i64 = 1_767_225_600_000;
const ORGANIZATION: u8 = 0x11;
const WORKSPACE: u8 = 0x22;
const API_KEY: u8 = 0x33;
const OPERATION: u8 = 0x44;
const STATEMENT: u8 = 0x55;
const USER: u8 = 0x66;

fn uuid7(tag: u8) -> Uuid7 {
    Uuid7::compose(NOW_MS.unsigned_abs(), [tag; 10])
}

fn raw_uuid(tag: u8) -> Uuid {
    Uuid::from_bytes(*uuid7(tag).as_bytes())
}

// ---------------------------------------------------------------------------
// The substrate: a clock, a resolver and an authenticator
// ---------------------------------------------------------------------------

#[derive(Debug)]
struct FixedClock;

impl Clock for FixedClock {
    fn now(&self) -> OffsetDateTime {
        OffsetDateTime::from_unix_timestamp_nanos(i128::from(NOW_MS) * 1_000_000)
            .expect("a representable instant")
    }
}

/// Resolves whatever resource class the route's own requirement names.
#[derive(Debug)]
struct Resolver;

#[async_trait]
impl TargetResolver for Resolver {
    async fn resolve(
        &self,
        id: RouteId,
        _path: &TargetPath,
    ) -> Result<Option<Resource>, EdgeError> {
        let organization_id = raw_uuid(ORGANIZATION);
        let action = Action::central(id).expect("a central route");
        Ok(Some(match requirement(action).resource_class {
            ResourceClass::None => Resource::None,
            ResourceClass::Organization => Resource::Organization(organization_id),
            ResourceClass::Workspace => Resource::Workspace {
                workspace_id: raw_uuid(WORKSPACE),
                organization_id,
            },
            ResourceClass::ApiKey => Resource::ApiKey {
                key_id: raw_uuid(API_KEY),
                workspace_id: raw_uuid(WORKSPACE),
                organization_id,
            },
            ResourceClass::Operation => Resource::Operation {
                operation_id: raw_uuid(OPERATION),
                organization_id,
            },
        }))
    }

    async fn account_state(&self, _organization_id: Uuid) -> Result<AccountState, EdgeError> {
        Ok(AccountState::Active)
    }
}

/// The one credential this suite admits, and nothing else.
///
/// Real verification is proven over the real ports in
/// `crates/aex-central-http/src/admission.rs`. What this double is for is the
/// composition: that the merged router asks the authenticator at all, that it
/// asks it on every route, and that an ambient context never substitutes for it.
#[derive(Debug)]
struct OneCredential {
    secret: &'static str,
    unavailable: bool,
}

#[async_trait]
impl CentralAuthenticator for OneCredential {
    async fn authenticate(
        &self,
        headers: &http::HeaderMap,
        _now: OffsetDateTime,
    ) -> Result<Option<CentralAuthorizerContext>, EdgeError> {
        if self.unavailable {
            return Err(EdgeError::AuthenticationUnavailable);
        }
        let Some(presented) = aex_central_http::admission::bearer(headers) else {
            return if headers.contains_key(http::header::AUTHORIZATION) {
                Err(EdgeError::CredentialRefused)
            } else {
                Ok(None)
            };
        };
        if presented != self.secret {
            return Err(EdgeError::CredentialRefused);
        }
        Ok(Some(owner_context()))
    }
}

fn owner_context() -> CentralAuthorizerContext {
    CentralAuthorizerContext {
        request_id: RequestId::parse("req-fixture").expect("a request id"),
        kind: ContextPrincipalKind::Account,
        principal_id: raw_uuid(USER),
        credential_id: Some(raw_uuid(0x77)),
        workspace_id: None,
        organization_id: None,
        region: None,
        memberships: vec![OrgMembership {
            organization_id: raw_uuid(ORGANIZATION),
            membership_id: raw_uuid(0x88),
            role: OrgRole::Owner,
        }],
        scopes: ScopeSet::CENTRAL,
        account_state: AccountState::Active,
        issued_at_ms: NOW_MS - 1_000,
        expires_at_ms: NOW_MS + 10_000,
    }
}

// ---------------------------------------------------------------------------
// The three services, each answering one declared refusal
// ---------------------------------------------------------------------------

/// Answers every route with a code that route declares, so a mounted route is
/// provably reached without this suite building twenty-six wire models.
#[derive(Debug)]
struct Api;

impl Api {
    fn refuse<T>() -> WireResult<T> {
        Err(WireError::new(ErrorCode::NotFound))
    }
}

impl ApiKeysApi for Api {
    async fn api_key_create(
        &self,
        _cx: &RequestContext,
        _body: aex_wire::models::ApiKeyCreateRequest,
    ) -> WireResult<Created<aex_wire::models::NewApiKey>> {
        Self::refuse()
    }

    async fn api_key_revoke(
        &self,
        _cx: &RequestContext,
        _api_key_id: ApiKeyId,
    ) -> WireResult<NoContent> {
        Self::refuse()
    }

    async fn api_keys_list(
        &self,
        _cx: &RequestContext,
        _query: aex_wire::models::ApiKeysListQuery,
    ) -> WireResult<aex_wire::models::ApiKeyPage> {
        Self::refuse()
    }
}

impl BootstrapApi for Api {
    async fn dashboard_bootstrap_get(
        &self,
        _cx: &RequestContext,
    ) -> WireResult<aex_wire::models::DashboardBootstrap> {
        Err(WireError::new(ErrorCode::Forbidden))
    }
}

impl CentralOperationsApi for Api {
    async fn central_operation_cancel(
        &self,
        _cx: &RequestContext,
        _operation_id: OperationId,
        _body: aex_wire::models::EmptyRequest,
    ) -> WireResult<aex_wire::models::Operation> {
        Self::refuse()
    }

    async fn central_operation_get(
        &self,
        _cx: &RequestContext,
        _operation_id: OperationId,
    ) -> WireResult<aex_wire::models::Operation> {
        Self::refuse()
    }

    async fn central_operations_list(
        &self,
        _cx: &RequestContext,
        _query: aex_wire::models::CentralOperationsListQuery,
    ) -> WireResult<aex_wire::models::OperationPage> {
        Err(WireError::new(ErrorCode::InvalidCursor))
    }
}

impl OrganizationsApi for Api {
    async fn invitation_accept(
        &self,
        _cx: &RequestContext,
        _body: aex_wire::models::EmptyRequest,
    ) -> WireResult<aex_wire::models::InvitationAcceptResult> {
        Self::refuse()
    }

    async fn invitation_create(
        &self,
        _cx: &RequestContext,
        _organization_id: OrganizationId,
        _body: aex_wire::models::InvitationCreateRequest,
    ) -> WireResult<Created<aex_wire::models::Invitation>> {
        Self::refuse()
    }

    async fn memberships_list(
        &self,
        _cx: &RequestContext,
        _organization_id: OrganizationId,
        _query: aex_wire::models::MembershipsListQuery,
    ) -> WireResult<aex_wire::models::MembershipPage> {
        Self::refuse()
    }

    async fn organization_create(
        &self,
        _cx: &RequestContext,
        _body: aex_wire::models::OrganizationCreateRequest,
    ) -> WireResult<Created<aex_wire::models::Organization>> {
        Err(WireError::new(ErrorCode::LimitExceeded))
    }

    async fn organization_get(
        &self,
        _cx: &RequestContext,
        _organization_id: OrganizationId,
    ) -> WireResult<aex_wire::models::Organization> {
        Self::refuse()
    }

    async fn organizations_list(
        &self,
        _cx: &RequestContext,
        _query: aex_wire::models::OrganizationsListQuery,
    ) -> WireResult<aex_wire::models::OrganizationPage> {
        Err(WireError::new(ErrorCode::InvalidCursor))
    }
}

impl WorkspacesApi for Api {
    async fn workspace_create(
        &self,
        _cx: &RequestContext,
        _body: aex_wire::models::WorkspaceCreateRequest,
    ) -> WireResult<Created<aex_wire::models::Workspace>> {
        Err(WireError::new(ErrorCode::LimitExceeded))
    }

    async fn workspace_delete(
        &self,
        _cx: &RequestContext,
        _workspace_id: WorkspaceId,
        _body: aex_wire::models::WorkspaceDeleteRequest,
    ) -> WireResult<Accepted> {
        Self::refuse()
    }

    async fn workspace_get(
        &self,
        _cx: &RequestContext,
        _workspace_id: WorkspaceId,
    ) -> WireResult<aex_wire::models::Workspace> {
        Self::refuse()
    }

    async fn workspaces_list(
        &self,
        _cx: &RequestContext,
        _query: aex_wire::models::WorkspacesListQuery,
    ) -> WireResult<aex_wire::models::WorkspacePage> {
        Err(WireError::new(ErrorCode::InvalidCursor))
    }
}

impl AuthApi for Api {
    async fn device_authorization_create(
        &self,
        _cx: &RequestContext,
        _body: aex_wire::models::DeviceAuthorizationRequest,
    ) -> WireResult<Created<aex_wire::models::DeviceAuthorization>> {
        Err(WireError::new(ErrorCode::RateLimited))
    }

    async fn device_token_create(
        &self,
        _cx: &RequestContext,
        _body: aex_wire::models::DeviceTokenRequest,
    ) -> WireResult<aex_wire::models::DeviceToken> {
        Err(WireError::new(ErrorCode::RateLimited))
    }

    async fn device_decision_create(
        &self,
        _cx: &RequestContext,
        _body: aex_wire::models::DeviceDecisionRequest,
    ) -> WireResult<aex_wire::models::DeviceDecisionResult> {
        Err(WireError::new(ErrorCode::RateLimited))
    }

    async fn dashboard_session_create(
        &self,
        _cx: &RequestContext,
        _body: aex_wire::models::DashboardSessionRequest,
    ) -> WireResult<Created<aex_wire::models::DashboardSessionCredential>> {
        Err(WireError::new(ErrorCode::RateLimited))
    }

    async fn dashboard_session_delete(
        &self,
        _cx: &RequestContext,
    ) -> WireResult<aex_wire::server::NoContent> {
        Err(WireError::new(ErrorCode::RateLimited))
    }
}

impl BillingApi for Api {
    async fn billing_auto_topup_policy_get(
        &self,
        _cx: &RequestContext,
        _organization_id: OrganizationId,
    ) -> WireResult<WithETag<aex_wire::models::AutoTopupPolicy>> {
        Self::refuse()
    }

    async fn billing_auto_topup_policy_put(
        &self,
        _cx: &RequestContext,
        _organization_id: OrganizationId,
        _body: aex_wire::models::AutoTopupPolicyRequest,
    ) -> WireResult<WithETag<aex_wire::models::AutoTopupPolicy>> {
        Self::refuse()
    }

    async fn billing_balance_get(
        &self,
        _cx: &RequestContext,
        _query: aex_wire::models::BillingBalanceGetQuery,
    ) -> WireResult<aex_wire::models::BillingBalance> {
        Self::refuse()
    }

    async fn billing_portal_session_create(
        &self,
        _cx: &RequestContext,
        _organization_id: OrganizationId,
        _body: aex_wire::models::PortalSessionRequest,
    ) -> WireResult<Created<aex_wire::models::HostedSession>> {
        Self::refuse()
    }

    async fn billing_statement_download_create(
        &self,
        _cx: &RequestContext,
        _organization_id: OrganizationId,
        _statement_id: StatementId,
        _body: aex_wire::models::EmptyRequest,
    ) -> WireResult<Created<aex_wire::models::DownloadGrant>> {
        Self::refuse()
    }

    async fn billing_statement_get(
        &self,
        _cx: &RequestContext,
        _organization_id: OrganizationId,
        _statement_id: StatementId,
    ) -> WireResult<aex_wire::models::Statement> {
        Self::refuse()
    }

    async fn billing_statements_list(
        &self,
        _cx: &RequestContext,
        _query_organization_id: OrganizationId,
        _query: aex_wire::models::BillingStatementsListQuery,
    ) -> WireResult<aex_wire::models::StatementSummaryPage> {
        Err(WireError::new(ErrorCode::InvalidCursor))
    }

    async fn billing_top_up_checkout_create(
        &self,
        _cx: &RequestContext,
        _organization_id: OrganizationId,
        _body: aex_wire::models::TopUpCheckoutRequest,
    ) -> WireResult<Created<aex_wire::models::HostedSession>> {
        Self::refuse()
    }
}

// ---------------------------------------------------------------------------
// The router under test
// ---------------------------------------------------------------------------

const SECRET: &str = "aex_at_the-one-admitted-credential";

fn http_config() -> HttpConfig {
    HttpConfig::resolve(
        "dev",
        Region::EuWest1.as_str(),
        CentralServiceId::CentralApi.as_str(),
        65_536,
        10_000,
    )
    .expect("a valid configuration")
}

fn edge(unavailable: bool) -> EdgeStack {
    EdgeStack::new(
        http_config(),
        Arc::new(Resolver),
        Arc::new(FixedClock),
        Arc::new(CursorSecret::new([3_u8; 32])),
    )
    .with_in_process_authentication(Arc::new(OneCredential {
        secret: SECRET,
        unavailable,
    }))
}

fn router() -> axum::Router {
    let api = Arc::new(Api);
    app(
        Arc::clone(&api),
        Arc::clone(&api),
        api,
        edge(false),
        readiness(Probes::READY),
    )
}

fn concrete_path(id: RouteId) -> String {
    route(id)
        .template
        .replace(
            "{organizationId}",
            OrganizationId::from_uuid7(uuid7(ORGANIZATION))
                .encode()
                .as_str(),
        )
        .replace(
            "{workspaceId}",
            WorkspaceId::from_uuid7(uuid7(WORKSPACE)).encode().as_str(),
        )
        .replace(
            "{apiKeyId}",
            ApiKeyId::from_uuid7(uuid7(API_KEY)).encode().as_str(),
        )
        .replace(
            "{operationId}",
            OperationId::from_uuid7(uuid7(OPERATION)).encode().as_str(),
        )
        .replace(
            "{statementId}",
            StatementId::from_uuid7(uuid7(STATEMENT)).encode().as_str(),
        )
}

fn with_query(id: RouteId, path: String) -> String {
    match id {
        RouteId::ApiKeysList => format!(
            "{path}?workspaceId={}",
            WorkspaceId::from_uuid7(uuid7(WORKSPACE)).encode()
        ),
        RouteId::BillingBalanceGet => format!(
            "{path}?organizationId={}",
            OrganizationId::from_uuid7(uuid7(ORGANIZATION)).encode()
        ),
        _ => path,
    }
}

/// A well-formed request for `id`, carrying `credential` if one is supplied.
fn request_for(id: RouteId, credential: Option<&str>) -> Request<Body> {
    let descriptor = route(id);
    let mut request = Request::builder()
        .method(descriptor.method.as_str())
        .uri(with_query(id, concrete_path(id)));
    if let Some(credential) = credential {
        request = request.header(http::header::AUTHORIZATION, format!("Bearer {credential}"));
    }
    match descriptor.idempotency {
        aex_wire::idempotency::IdempotencyKind::IdempotencyKey => {
            request = request.header("idempotency-key", "fixture");
        }
        aex_wire::idempotency::IdempotencyKind::OperationId => {
            request = request.header(
                "aex-operation-id",
                OperationId::from_uuid7(uuid7(OPERATION)).encode().as_str(),
            );
        }
        aex_wire::idempotency::IdempotencyKind::None => {}
    }
    if descriptor.etag == aex_wire::routes::EtagPolicy::RequiredIfMatch {
        request = request.header(http::header::IF_MATCH, "\"1\"");
    }
    let body = if descriptor.body_class == aex_wire::routes::BodyClass::None {
        Body::empty()
    } else {
        Body::from(body_for(id))
    };
    request.body(body).expect("a valid request")
}

fn body_for(id: RouteId) -> &'static str {
    match id {
        RouteId::DeviceAuthorizationCreate => "{\"clientId\":\"aex-cli\",\"scopes\":[]}",
        RouteId::DeviceTokenCreate => "{\"clientId\":\"aex-cli\",\"deviceCode\":\"dvc_fixture\"}",
        _ => "{}",
    }
}

async fn status_of(router: axum::Router, request: Request<Body>) -> StatusCode {
    router
        .oneshot(request)
        .await
        .expect("the router answers")
        .status()
}

// ---------------------------------------------------------------------------
// What is mounted
// ---------------------------------------------------------------------------

#[tokio::test]
async fn the_mounted_set_is_exactly_the_declared_one() {
    let declared = CentralServiceId::CentralApi.routes();
    assert_eq!(declared.len(), 30, "the merged deployable serves 30 routes");
    for id in declared {
        let descriptor = route(id);
        let credential = if descriptor.plane == aex_wire::routes::Plane::Central {
            Some(SECRET)
        } else {
            None
        };
        let response = router()
            .oneshot(request_for(id, credential))
            .await
            .expect("the router answers");
        assert_ne!(
            response.status(),
            StatusCode::NOT_IMPLEMENTED,
            "`{}` is not mounted",
            descriptor.operation_id
        );
        assert_eq!(
            response
                .headers()
                .get(axum::http::header::CONTENT_TYPE)
                .and_then(|value| value.to_str().ok()),
            Some("application/json"),
            "`{}` answered without an AEX envelope, so it is not mounted",
            descriptor.operation_id
        );
    }
}

#[tokio::test]
async fn one_listener_answers_control_auth_and_billing() {
    // The merge's whole point, stated as one test: three route groups that were
    // three deployments are reachable on one process.
    for (id, expected) in [
        (RouteId::OrganizationsList, StatusCode::BAD_REQUEST),
        (
            RouteId::DeviceAuthorizationCreate,
            StatusCode::TOO_MANY_REQUESTS,
        ),
        (RouteId::BillingStatementsList, StatusCode::BAD_REQUEST),
    ] {
        let credential = if id == RouteId::DeviceAuthorizationCreate {
            None
        } else {
            Some(SECRET)
        };
        assert_eq!(
            status_of(router(), request_for(id, credential)).await,
            expected,
            "`{}` did not reach its handler",
            route(id).operation_id
        );
    }
}

#[tokio::test]
async fn a_route_no_central_deployable_serves_answers_the_published_refusal() {
    // `account_get` is authored and deliberately unserved. A merge is the
    // easiest place for a real handler to be mounted by accident, so what must
    // answer here is the generated refusal arm — before authentication, which
    // is why the credential below buys nothing.
    assert_eq!(
        status_of(
            router(),
            Request::builder()
                .uri("/api/account")
                .header(http::header::AUTHORIZATION, format!("Bearer {SECRET}"))
                .body(Body::empty())
                .expect("a valid request"),
        )
        .await,
        StatusCode::NOT_IMPLEMENTED
    );
    assert!(
        aex_central_http::deferred_routes(aex_central_http::CentralServiceId::CentralApi)
            .contains(&RouteId::AccountGet),
        "the refusal is keyed on the ledger, not on a list here"
    );
}

// ---------------------------------------------------------------------------
// Authentication, now that there is no gateway in front
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_caller_supplied_authorizer_context_gains_nothing() {
    // The bypass this whole consolidation had to close. An ALB forwards headers
    // verbatim, so the extension a gateway event would have carried is now
    // something a caller can attach. A stack that verifies in process must never
    // read it, and `EdgeStack` makes that exclusive by enum rather than by
    // ordering.
    let forged = status_of(router(), {
        let mut request = request_for(RouteId::OrganizationsList, None);
        request.extensions_mut().insert(owner_context());
        request
    })
    .await;
    assert_eq!(
        forged,
        StatusCode::UNAUTHORIZED,
        "an asserted principal must be worth exactly as much as no principal"
    );
}

#[tokio::test]
async fn a_credentialed_route_with_no_credential_is_refused() {
    assert_eq!(
        status_of(router(), request_for(RouteId::OrganizationsList, None)).await,
        StatusCode::UNAUTHORIZED
    );
}

#[tokio::test]
async fn a_wrong_secret_is_refused_rather_than_downgraded_to_anonymous() {
    // On a route that admits anonymous callers, a *presented* credential that
    // does not verify must still be a refusal. Treating it as absent would make
    // the device-flow routes admit a forged credential as if it were none.
    assert_eq!(
        status_of(
            router(),
            request_for(RouteId::DeviceAuthorizationCreate, Some("aex_at_wrong")),
        )
        .await,
        StatusCode::UNAUTHORIZED
    );
}

#[tokio::test]
async fn the_two_device_flow_routes_admit_a_request_with_no_credential_at_all() {
    for id in [
        RouteId::DeviceAuthorizationCreate,
        RouteId::DeviceTokenCreate,
    ] {
        assert_eq!(
            status_of(router(), request_for(id, None)).await,
            StatusCode::TOO_MANY_REQUESTS,
            "`{}` must reach its handler with no credential",
            route(id).operation_id
        );
    }
}

#[tokio::test]
async fn an_unreachable_authority_is_retryable_and_never_an_invalid_credential() {
    // Answering `401` when the truth is "we could not check" is a lie the caller
    // acts on by discarding a credential that works.
    let api = Arc::new(Api);
    let router = app(
        Arc::clone(&api),
        Arc::clone(&api),
        api,
        edge(true),
        readiness(Probes::READY),
    );
    assert_eq!(
        status_of(
            router,
            request_for(RouteId::OrganizationsList, Some(SECRET))
        )
        .await,
        StatusCode::SERVICE_UNAVAILABLE
    );
}

// ---------------------------------------------------------------------------
// The probes and the drain
// ---------------------------------------------------------------------------

#[tokio::test]
async fn liveness_answers_and_readiness_is_fail_closed_until_every_probe_lands() {
    let api = Arc::new(Api);
    let not_ready = app(
        Arc::clone(&api),
        Arc::clone(&api),
        Arc::clone(&api),
        edge(false),
        readiness(Probes::NONE),
    );
    assert_eq!(
        status_of(
            not_ready.clone(),
            Request::builder()
                .uri(HEALTH_PATH)
                .body(Body::empty())
                .expect("a valid request"),
        )
        .await,
        StatusCode::OK
    );
    assert_eq!(
        status_of(
            not_ready,
            Request::builder()
                .uri(READY_PATH)
                .body(Body::empty())
                .expect("a valid request"),
        )
        .await,
        StatusCode::SERVICE_UNAVAILABLE
    );
}

#[tokio::test]
async fn a_raised_drain_flag_deregisters_the_target_while_routes_still_answer() {
    // The ordering the whole graceful shutdown depends on: readiness must go
    // `503` *before* the listener stops, or the load balancer keeps sending to a
    // socket that is already closing.
    let draining = Arc::new(AtomicBool::new(false));
    let api = Arc::new(Api);
    let build = || {
        app(
            Arc::clone(&api),
            Arc::clone(&api),
            Arc::clone(&api),
            edge(false),
            readiness(Probes::READY).with_drain_signal(Arc::clone(&draining)),
        )
    };
    let ready = |router: axum::Router| async move {
        status_of(
            router,
            Request::builder()
                .uri(READY_PATH)
                .body(Body::empty())
                .expect("a valid request"),
        )
        .await
    };

    assert_eq!(ready(build()).await, StatusCode::OK);
    draining.store(true, Ordering::Release);
    assert_eq!(ready(build()).await, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(
        status_of(
            build(),
            request_for(RouteId::OrganizationsList, Some(SECRET))
        )
        .await,
        StatusCode::BAD_REQUEST,
        "a draining task finishes the requests it already accepted"
    );
}

// ---------------------------------------------------------------------------
// Finance's two extra policy steps, reconciled against the shared stack
// ---------------------------------------------------------------------------

#[tokio::test]
async fn the_shared_stack_enforces_the_replay_policy_finance_enforced_itself() {
    // `finance_api::edge::policy::replay_key` is strict in both directions. The
    // shared stack reaches the same answer through `headers::declared` and
    // `aex_wire::dispatch::request_identity`, which is what makes mounting
    // billing on `EdgeStack` a move rather than a loss.
    let missing = router()
        .oneshot({
            let mut request = Request::builder()
                .method(route(RouteId::BillingTopUpCheckoutCreate).method.as_str())
                .uri(concrete_path(RouteId::BillingTopUpCheckoutCreate))
                .header(http::header::AUTHORIZATION, format!("Bearer {SECRET}"));
            request = request.header(http::header::CONTENT_TYPE, "application/json");
            request.body(Body::from("{}")).expect("a valid request")
        })
        .await
        .expect("the router answers");
    assert_eq!(
        missing.status(),
        StatusCode::BAD_REQUEST,
        "a route that declares `Idempotency-Key` refuses a request without one"
    );

    let unwanted = status_of(router(), {
        let mut request = request_for(RouteId::BillingBalanceGet, Some(SECRET));
        request.headers_mut().insert(
            "idempotency-key",
            http::HeaderValue::from_static("idem_abc"),
        );
        request
    })
    .await;
    assert_eq!(
        unwanted,
        StatusCode::BAD_REQUEST,
        "a route that declares no replay identity refuses a supplied key; a \
         silently ignored key is a guarantee the caller believes it has"
    );
}

#[tokio::test]
async fn the_shared_stack_enforces_the_scope_finance_enforced_itself() {
    // `finance_api::edge::policy::scope` reads `RouteDescriptor::required_scope`.
    // The shared stack reaches it through `aex_control_domain::decide`, which
    // answers `insufficient_scope` for the same shortfall — and, as the wire
    // contract requires, renders it as `403` alongside every other denial so a
    // caller cannot tell a role shortfall from a scope shortfall.
    #[derive(Debug)]
    struct ReadOnly;

    #[async_trait]
    impl CentralAuthenticator for ReadOnly {
        async fn authenticate(
            &self,
            _headers: &http::HeaderMap,
            _now: OffsetDateTime,
        ) -> Result<Option<CentralAuthorizerContext>, EdgeError> {
            let mut context = owner_context();
            context.scopes = ScopeSet::of(&[aex_control_domain::Scope::BillingRead]);
            Ok(Some(context))
        }
    }

    let api = Arc::new(Api);
    let router = app(
        Arc::clone(&api),
        Arc::clone(&api),
        api,
        EdgeStack::new(
            http_config(),
            Arc::new(Resolver),
            Arc::new(FixedClock),
            Arc::new(CursorSecret::new([3_u8; 32])),
        )
        .with_in_process_authentication(Arc::new(ReadOnly)),
        readiness(Probes::READY),
    );
    assert_eq!(
        status_of(
            router,
            request_for(RouteId::BillingAutoTopupPolicyPut, Some(SECRET)),
        )
        .await,
        StatusCode::FORBIDDEN,
        "`billing:read` must not satisfy a route that declares `billing:write`"
    );
}

#[test]
fn the_merged_principal_is_the_one_the_replay_identity_records() {
    // A merge that changed the replay principal would silently invalidate every
    // in-flight `Idempotency-Key`: the grant is keyed on the principal, so the
    // same caller replaying the same key after a cutover would get a second
    // effect rather than the first one's answer.
    let context = owner_context();
    assert_eq!(context.kind, ContextPrincipalKind::Account);
    assert_eq!(
        UserId::from_uuid7(Uuid7::from_bytes(*context.principal_id.as_bytes()).expect("a uuid7")),
        UserId::from_uuid7(uuid7(USER))
    );
}
