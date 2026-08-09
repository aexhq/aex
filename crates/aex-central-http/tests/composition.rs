//! The mounted central surface, end to end through `axum`.
//!
//! The load-bearing assertion is that the mounted set **is** the generated one:
//! every route the contract bundle declares for the central plane answers, and a
//! path outside the table does not. A route that is authored and never mounted
//! is a failure here rather than a runtime `404` a customer discovers.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use aex_central_http::authorizer::{CentralAuthorizerContext, ContextPrincipalKind};
use aex_central_http::config::{CentralServiceId, HttpConfig, central_groups};
use aex_central_http::error::EdgeError;
use aex_central_http::router::{
    EdgeStack, mount_api_keys_api, mount_auth_api, mount_billing_api, mount_bootstrap_api,
    mount_central_operations_api, mount_identity_api, mount_organizations_api,
    mount_workspaces_api,
};
use aex_central_http::target::{TargetPath, TargetResolver};
use aex_control_app::ports::Clock;
use aex_control_domain::{
    AccountState, Action, OrgMembership, OrgRole, Resource, ResourceClass, Scope,
    ScopeSet as ControlScopeSet, requirement,
};
use aex_wire::error::{ErrorCode, WireError, WireResult};
use aex_wire::ids::{
    ApiKeyId, OperationId, OrganizationId, PrefixedId, StatementId, UserId, Uuid7, WorkspaceId,
};
use aex_wire::routes::{Plane, ROUTES, RouteId, route};
use aex_wire::server::{
    Accepted, ApiKeysApi, AuthApi, BillingApi, BootstrapApi, CentralOperationsApi, Created,
    IdentityApi, NoContent, OrganizationsApi, RequestContext, WithETag, WorkspacesApi,
};
use aex_wire::types::Region;
use async_trait::async_trait;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt as _;
use time::OffsetDateTime;
use tower::ServiceExt as _;
use uuid::Uuid;

// ---------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------

const NOW_MS: i64 = 1_767_225_600_000;

const ORGANIZATION: u8 = 0x11;
const WORKSPACE: u8 = 0x22;
const API_KEY: u8 = 0x33;
const OPERATION: u8 = 0x44;
const USER: u8 = 0x55;
const CREDENTIAL: u8 = 0x66;
const MEMBERSHIP: u8 = 0x77;
const STATEMENT: u8 = 0x88;

/// A `UUIDv7` with a fixed timestamp, so every id in this suite is stable.
fn uuid7(tag: u8) -> Uuid7 {
    Uuid7::compose(1_767_225_600_000, [tag; 10])
}

fn raw_uuid(tag: u8) -> Uuid {
    Uuid::from_bytes(*uuid7(tag).as_bytes())
}

#[derive(Debug)]
struct FixedClock;

impl Clock for FixedClock {
    fn now(&self) -> OffsetDateTime {
        OffsetDateTime::from_unix_timestamp_nanos(i128::from(NOW_MS) * 1_000_000)
            .expect("a representable instant")
    }
}

#[derive(Debug)]
struct Resolver {
    state: AccountState,
    state_reads: AtomicUsize,
}

impl Resolver {
    fn new(state: AccountState) -> Arc<Self> {
        Arc::new(Self {
            state,
            state_reads: AtomicUsize::new(0),
        })
    }
}

#[async_trait]
impl TargetResolver for Resolver {
    async fn resolve(
        &self,
        id: RouteId,
        _path: &TargetPath,
    ) -> Result<Option<Resource>, EdgeError> {
        // The double resolves whatever class the route's own requirement
        // declares, so the fixture cannot disagree with the rule table.
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
        self.state_reads.fetch_add(1, Ordering::SeqCst);
        match self.state {
            AccountState::Unavailable => Err(EdgeError::AccountStateUnavailable),
            state => Ok(state),
        }
    }
}

/// A context for an owner carrying `scopes`.
fn owner_context(scopes: ControlScopeSet) -> CentralAuthorizerContext {
    CentralAuthorizerContext {
        request_id: aex_wire::types::RequestId::parse("req-fixture").expect("a request id"),
        kind: ContextPrincipalKind::Account,
        principal_id: raw_uuid(USER),
        credential_id: Some(raw_uuid(CREDENTIAL)),
        workspace_id: None,
        organization_id: None,
        region: None,
        memberships: vec![OrgMembership {
            organization_id: raw_uuid(ORGANIZATION),
            membership_id: raw_uuid(MEMBERSHIP),
            role: OrgRole::Owner,
        }],
        scopes,
        account_state: AccountState::Active,
        issued_at_ms: NOW_MS - 1_000,
        expires_at_ms: NOW_MS + 10_000,
    }
}

fn edge(resolver: Arc<Resolver>) -> EdgeStack {
    EdgeStack::new(
        HttpConfig::resolve("dev", "eu-west-1", "central-control-api", 4_096, 5_000)
            .expect("a valid configuration"),
        resolver,
        Arc::new(FixedClock),
        Arc::new(aex_control_domain::CursorSecret::new([3_u8; 32])),
    )
}

// ---------------------------------------------------------------------------
// The double: one implementation of all eight generated central traits
// ---------------------------------------------------------------------------

/// What the double answers with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Answer {
    /// A declared refusal, so a mounted route is provably reached.
    Declared,
    /// A code the route does not declare, to exercise the dispatch boundary.
    Undeclared,
    /// The route's own success shape, where this suite constructs one.
    Success,
}

#[derive(Debug)]
struct Api {
    answer: Answer,
    calls: AtomicUsize,
}

impl Api {
    fn new(answer: Answer) -> Arc<Self> {
        Arc::new(Self {
            answer,
            calls: AtomicUsize::new(0),
        })
    }

    fn refuse<T>(&self) -> WireResult<T> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        match self.answer {
            Answer::Undeclared => Err(WireError::new(ErrorCode::SessionNotIdle)),
            Answer::Declared | Answer::Success => Err(WireError::new(ErrorCode::NotFound)),
        }
    }
}

impl ApiKeysApi for Api {
    async fn api_key_create(
        &self,
        _cx: &RequestContext,
        _body: aex_wire::models::ApiKeyCreateRequest,
    ) -> WireResult<Created<aex_wire::models::NewApiKey>> {
        self.refuse()
    }

    async fn api_key_revoke(
        &self,
        _cx: &RequestContext,
        _api_key_id: ApiKeyId,
    ) -> WireResult<NoContent> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        match self.answer {
            Answer::Success => Ok(NoContent),
            Answer::Undeclared => Err(WireError::new(ErrorCode::SessionNotIdle)),
            Answer::Declared => Err(WireError::new(ErrorCode::NotFound)),
        }
    }

    async fn api_keys_list(
        &self,
        _cx: &RequestContext,
        _query: aex_wire::models::ApiKeysListQuery,
    ) -> WireResult<aex_wire::models::ApiKeyPage> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        match self.answer {
            Answer::Success => Ok(aex_wire::models::ApiKeyPage {
                items: Vec::new(),
                next_cursor: None,
            }),
            Answer::Undeclared => Err(WireError::new(ErrorCode::SessionNotIdle)),
            Answer::Declared => Err(WireError::new(ErrorCode::NotFound)),
        }
    }
}

impl AuthApi for Api {
    async fn device_authorization_create(
        &self,
        _cx: &RequestContext,
        _body: aex_wire::models::DeviceAuthorizationRequest,
    ) -> WireResult<Created<aex_wire::models::DeviceAuthorization>> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Err(WireError::new(ErrorCode::RateLimited))
    }

    async fn device_token_create(
        &self,
        _cx: &RequestContext,
        _body: aex_wire::models::DeviceTokenRequest,
    ) -> WireResult<aex_wire::models::DeviceToken> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Err(WireError::new(ErrorCode::RateLimited))
    }
}

impl BillingApi for Api {
    async fn billing_auto_topup_policy_get(
        &self,
        _cx: &RequestContext,
        _organization_id: OrganizationId,
    ) -> WireResult<WithETag<aex_wire::models::AutoTopupPolicy>> {
        self.refuse()
    }

    async fn billing_auto_topup_policy_put(
        &self,
        _cx: &RequestContext,
        _organization_id: OrganizationId,
        _body: aex_wire::models::AutoTopupPolicyRequest,
    ) -> WireResult<WithETag<aex_wire::models::AutoTopupPolicy>> {
        self.refuse()
    }

    async fn billing_balance_get(
        &self,
        _cx: &RequestContext,
        _query: aex_wire::models::BillingBalanceGetQuery,
    ) -> WireResult<aex_wire::models::BillingBalance> {
        self.refuse()
    }

    async fn billing_portal_session_create(
        &self,
        _cx: &RequestContext,
        _organization_id: OrganizationId,
        _body: aex_wire::models::PortalSessionRequest,
    ) -> WireResult<Created<aex_wire::models::HostedSession>> {
        self.refuse()
    }

    async fn billing_statement_download_create(
        &self,
        _cx: &RequestContext,
        _organization_id: OrganizationId,
        _statement_id: StatementId,
        _body: aex_wire::models::EmptyRequest,
    ) -> WireResult<Created<aex_wire::models::DownloadGrant>> {
        self.refuse()
    }

    async fn billing_statement_get(
        &self,
        _cx: &RequestContext,
        _organization_id: OrganizationId,
        _statement_id: StatementId,
    ) -> WireResult<aex_wire::models::Statement> {
        self.refuse()
    }

    async fn billing_statements_list(
        &self,
        _cx: &RequestContext,
        _organization_id: OrganizationId,
        _query: aex_wire::models::BillingStatementsListQuery,
    ) -> WireResult<aex_wire::models::StatementSummaryPage> {
        self.refuse()
    }

    async fn billing_top_up_checkout_create(
        &self,
        _cx: &RequestContext,
        _organization_id: OrganizationId,
        _body: aex_wire::models::TopUpCheckoutRequest,
    ) -> WireResult<Created<aex_wire::models::HostedSession>> {
        self.refuse()
    }
}

impl BootstrapApi for Api {
    async fn dashboard_bootstrap_get(
        &self,
        _cx: &RequestContext,
    ) -> WireResult<aex_wire::models::DashboardBootstrap> {
        self.refuse()
    }
}

impl CentralOperationsApi for Api {
    async fn central_operation_cancel(
        &self,
        _cx: &RequestContext,
        _operation_id: OperationId,
        _body: aex_wire::models::EmptyRequest,
    ) -> WireResult<aex_wire::models::Operation> {
        self.refuse()
    }

    async fn central_operation_get(
        &self,
        _cx: &RequestContext,
        _operation_id: OperationId,
    ) -> WireResult<aex_wire::models::Operation> {
        self.refuse()
    }

    async fn central_operations_list(
        &self,
        _cx: &RequestContext,
        _query: aex_wire::models::CentralOperationsListQuery,
    ) -> WireResult<aex_wire::models::OperationPage> {
        self.refuse()
    }
}

impl IdentityApi for Api {
    async fn account_get(
        &self,
        _cx: &RequestContext,
        _query: aex_wire::models::AccountGetQuery,
    ) -> WireResult<aex_wire::models::AccountOperationalState> {
        self.refuse()
    }
}

impl OrganizationsApi for Api {
    async fn invitation_create(
        &self,
        _cx: &RequestContext,
        _organization_id: OrganizationId,
        _body: aex_wire::models::InvitationCreateRequest,
    ) -> WireResult<Created<aex_wire::models::Invitation>> {
        self.refuse()
    }

    async fn memberships_list(
        &self,
        _cx: &RequestContext,
        _organization_id: OrganizationId,
        _query: aex_wire::models::MembershipsListQuery,
    ) -> WireResult<aex_wire::models::MembershipPage> {
        self.refuse()
    }

    async fn organization_create(
        &self,
        _cx: &RequestContext,
        _body: aex_wire::models::OrganizationCreateRequest,
    ) -> WireResult<Created<aex_wire::models::Organization>> {
        self.refuse()
    }

    async fn organization_get(
        &self,
        _cx: &RequestContext,
        _organization_id: OrganizationId,
    ) -> WireResult<aex_wire::models::Organization> {
        self.refuse()
    }

    async fn organizations_list(
        &self,
        _cx: &RequestContext,
        _query: aex_wire::models::OrganizationsListQuery,
    ) -> WireResult<aex_wire::models::OrganizationPage> {
        self.refuse()
    }
}

impl WorkspacesApi for Api {
    async fn workspace_create(
        &self,
        _cx: &RequestContext,
        _body: aex_wire::models::WorkspaceCreateRequest,
    ) -> WireResult<Created<aex_wire::models::Workspace>> {
        self.refuse()
    }

    async fn workspace_delete(
        &self,
        _cx: &RequestContext,
        _workspace_id: WorkspaceId,
        _body: aex_wire::models::WorkspaceDeleteRequest,
    ) -> WireResult<Accepted> {
        self.refuse()
    }

    async fn workspace_get(
        &self,
        _cx: &RequestContext,
        _workspace_id: WorkspaceId,
    ) -> WireResult<aex_wire::models::Workspace> {
        self.refuse()
    }

    async fn workspaces_list(
        &self,
        _cx: &RequestContext,
        _query: aex_wire::models::WorkspacesListQuery,
    ) -> WireResult<aex_wire::models::WorkspacePage> {
        self.refuse()
    }
}

// ---------------------------------------------------------------------------
// Harness
// ---------------------------------------------------------------------------

/// Every mounted group behind one router.
fn plane(api: Arc<Api>, edge: EdgeStack) -> axum::Router {
    axum::Router::new()
        .merge(mount_api_keys_api(Arc::clone(&api), edge.clone()))
        .merge(mount_auth_api(Arc::clone(&api), edge.clone()))
        .merge(mount_billing_api(Arc::clone(&api), edge.clone()))
        .merge(mount_bootstrap_api(Arc::clone(&api), edge.clone()))
        .merge(mount_central_operations_api(Arc::clone(&api), edge.clone()))
        .merge(mount_identity_api(Arc::clone(&api), edge.clone()))
        .merge(mount_organizations_api(Arc::clone(&api), edge.clone()))
        .merge(mount_workspaces_api(api, edge))
}

/// A concrete path for one route template.
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
        .replace(
            "{userId}",
            UserId::from_uuid7(uuid7(USER)).encode().as_str(),
        )
}

struct Sent {
    status: StatusCode,
    content_type: Option<String>,
    body: String,
}

async fn send(router: axum::Router, request: Request<Body>) -> Sent {
    let response = router.oneshot(request).await.expect("the router answers");
    let status = response.status();
    let content_type = response
        .headers()
        .get(axum::http::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("a complete body")
        .to_bytes();
    Sent {
        status,
        content_type,
        body: String::from_utf8_lossy(&bytes).into_owned(),
    }
}

/// Builds a request for `id` carrying whatever the route declares.
fn request_for(id: RouteId, context: Option<CentralAuthorizerContext>) -> Request<Body> {
    let descriptor = route(id);
    let mut builder = Request::builder()
        .method(descriptor.method.as_str())
        .uri(with_required_query(id, concrete_path(id)));
    match descriptor.idempotency {
        aex_wire::idempotency::IdempotencyKind::IdempotencyKey => {
            builder = builder.header("idempotency-key", "fixture-key");
        }
        aex_wire::idempotency::IdempotencyKind::OperationId => {
            builder = builder.header(
                "aex-operation-id",
                OperationId::from_uuid7(uuid7(OPERATION)).encode().as_str(),
            );
        }
        aex_wire::idempotency::IdempotencyKind::None => {}
    }
    if matches!(
        descriptor.etag,
        aex_wire::routes::EtagPolicy::RequiredIfMatch
    ) {
        builder = builder.header("if-match", "\"1\"");
    }
    if let Some(context) = context {
        builder = builder.extension(context);
    }
    let body = if descriptor.body_class == aex_wire::routes::BodyClass::None {
        Body::empty()
    } else {
        Body::from(valid_body(id))
    };
    builder.body(body).expect("a valid request")
}

/// A body every strict model accepts, per route.
///
/// `{}` is enough wherever the request schema has no required member; the two
/// device-flow routes declare theirs, so they get a real one.
fn valid_body(id: RouteId) -> &'static str {
    match id {
        RouteId::DeviceAuthorizationCreate => "{\"clientId\":\"aex-cli\",\"scopes\":[]}",
        RouteId::DeviceTokenCreate => "{\"clientId\":\"aex-cli\",\"deviceCode\":\"dvc_fixture\"}",
        _ => "{}",
    }
}

/// Fills in the query parameters a route declares as required.
fn with_required_query(id: RouteId, path: String) -> String {
    match id {
        RouteId::ApiKeysList => format!(
            "{path}?workspaceId={}",
            WorkspaceId::from_uuid7(uuid7(WORKSPACE)).encode()
        ),
        RouteId::AccountGet | RouteId::BillingBalanceGet => format!(
            "{path}?organizationId={}",
            OrganizationId::from_uuid7(uuid7(ORGANIZATION)).encode()
        ),
        _ => path,
    }
}

fn central_routes() -> Vec<RouteId> {
    ROUTES
        .iter()
        .filter(|descriptor| descriptor.plane == Plane::Central)
        .map(|descriptor| descriptor.id)
        .collect()
}

// ---------------------------------------------------------------------------
// The mounted surface
// ---------------------------------------------------------------------------

#[tokio::test]
async fn every_generated_central_route_is_mounted() {
    let declared = central_routes();
    assert_eq!(declared.len(), 27, "the central plane declares 27 routes");
    for id in declared {
        let router = plane(
            Api::new(Answer::Declared),
            edge(Resolver::new(AccountState::Active)),
        );
        let sent = send(router, request_for(id, None)).await;
        assert_eq!(
            sent.content_type.as_deref(),
            Some("application/json"),
            "`{}` answered without an AEX envelope, so it is not mounted: {} {}",
            route(id).operation_id,
            sent.status,
            sent.body
        );
    }
}

#[tokio::test]
async fn a_path_outside_the_generated_table_is_an_unrouted_four_hundred_and_four() {
    let router = plane(
        Api::new(Answer::Declared),
        edge(Resolver::new(AccountState::Active)),
    );
    let sent = send(
        router,
        Request::builder()
            .method("GET")
            .uri("/api/not-a-route")
            .body(Body::empty())
            .expect("a valid request"),
    )
    .await;
    assert_eq!(sent.status, StatusCode::NOT_FOUND);
    assert_eq!(
        sent.content_type, None,
        "an unmounted path never renders an AEX envelope"
    );
}

#[tokio::test]
async fn a_method_the_template_does_not_declare_is_refused_by_the_router() {
    let router = plane(
        Api::new(Answer::Declared),
        edge(Resolver::new(AccountState::Active)),
    );
    let sent = send(
        router,
        Request::builder()
            .method("DELETE")
            .uri("/api/organizations")
            .body(Body::empty())
            .expect("a valid request"),
    )
    .await;
    assert_eq!(sent.status, StatusCode::METHOD_NOT_ALLOWED);
}

#[test]
fn the_eight_central_groups_partition_the_central_route_table() {
    let groups = central_groups();
    assert_eq!(groups.len(), 8);
    let mut mounted: Vec<RouteId> = groups
        .iter()
        .flat_map(|group| group.routes().iter().copied())
        .collect();
    let count = mounted.len();
    mounted.sort_unstable();
    mounted.dedup();
    assert_eq!(count, mounted.len(), "a route belongs to two groups");
    assert_eq!(
        mounted.into_iter().collect::<BTreeSet<_>>(),
        central_routes().into_iter().collect::<BTreeSet<_>>()
    );
}

#[test]
fn each_deployable_mounts_a_disjoint_slice_of_the_actually_served_plane() {
    let mut seen: BTreeSet<RouteId> = BTreeSet::new();
    for service in CentralServiceId::ALL {
        for id in service.routes() {
            assert!(seen.insert(id), "{id:?} is served twice");
        }
    }
    let all = central_routes().into_iter().collect::<BTreeSet<_>>();
    assert_eq!(
        all.difference(&seen).copied().collect::<Vec<_>>(),
        vec![RouteId::AccountGet]
    );
}

// ---------------------------------------------------------------------------
// The precedence stages
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_credentialed_route_without_an_authorizer_context_is_unauthenticated() {
    let api = Api::new(Answer::Declared);
    let router = plane(Arc::clone(&api), edge(Resolver::new(AccountState::Active)));
    let sent = send(router, request_for(RouteId::OrganizationsList, None)).await;
    assert_eq!(sent.status, StatusCode::UNAUTHORIZED);
    assert!(sent.body.contains("unauthenticated"), "{}", sent.body);
    assert_eq!(
        api.calls.load(Ordering::SeqCst),
        0,
        "no handler runs before authentication"
    );
}

#[tokio::test]
async fn the_two_device_flow_routes_admit_a_request_with_no_credential_at_all() {
    for id in [
        RouteId::DeviceAuthorizationCreate,
        RouteId::DeviceTokenCreate,
    ] {
        let api = Api::new(Answer::Declared);
        let router = plane(Arc::clone(&api), edge(Resolver::new(AccountState::Active)));
        let sent = send(router, request_for(id, None)).await;
        assert_eq!(
            sent.status,
            StatusCode::TOO_MANY_REQUESTS,
            "`{}` reached its handler: {}",
            route(id).operation_id,
            sent.body
        );
        assert_eq!(api.calls.load(Ordering::SeqCst), 1);
    }
}

#[tokio::test]
async fn a_context_outside_its_window_is_refused_with_no_stale_fallback() {
    let api = Api::new(Answer::Declared);
    let mut context = owner_context(ControlScopeSet::CENTRAL);
    context.issued_at_ms = NOW_MS - 60_000;
    context.expires_at_ms = NOW_MS - 30_000;
    let router = plane(Arc::clone(&api), edge(Resolver::new(AccountState::Active)));
    let sent = send(
        router,
        request_for(RouteId::OrganizationsList, Some(context)),
    )
    .await;
    assert_eq!(sent.status, StatusCode::UNAUTHORIZED);
    assert_eq!(api.calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn a_credential_without_the_route_scope_is_insufficient_scope() {
    let api = Api::new(Answer::Declared);
    let context = owner_context(ControlScopeSet::of(&[Scope::AccountRead]));
    let router = plane(Arc::clone(&api), edge(Resolver::new(AccountState::Active)));
    let sent = send(router, request_for(RouteId::MembershipsList, Some(context))).await;
    assert_eq!(sent.status, StatusCode::FORBIDDEN);
    assert!(sent.body.contains("insufficient_scope"), "{}", sent.body);
    assert_eq!(api.calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn a_paused_account_is_refused_on_a_non_exempt_route() {
    let api = Api::new(Answer::Declared);
    let context = owner_context(ControlScopeSet::CENTRAL);
    let router = plane(
        Arc::clone(&api),
        edge(Resolver::new(AccountState::PausedTopUpRequired)),
    );
    let sent = send(router, request_for(RouteId::MembershipsList, Some(context))).await;
    assert_eq!(sent.status, StatusCode::PAYMENT_REQUIRED);
    assert!(sent.body.contains("account_paused"), "{}", sent.body);
    assert_eq!(api.calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn an_unreadable_account_state_is_a_retryable_five_hundred_and_three() {
    let api = Api::new(Answer::Declared);
    let context = owner_context(ControlScopeSet::CENTRAL);
    let router = plane(
        Arc::clone(&api),
        edge(Resolver::new(AccountState::Unavailable)),
    );
    let sent = send(router, request_for(RouteId::MembershipsList, Some(context))).await;
    assert_eq!(sent.status, StatusCode::SERVICE_UNAVAILABLE);
    assert!(
        sent.body.contains("account_state_unavailable"),
        "{}",
        sent.body
    );
    assert!(sent.body.contains("\"retryable\":true"), "{}", sent.body);
    assert_eq!(api.calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn an_oversize_body_is_refused_before_it_is_parsed() {
    let api = Api::new(Answer::Declared);
    let context = owner_context(ControlScopeSet::CENTRAL);
    let router = plane(Arc::clone(&api), edge(Resolver::new(AccountState::Active)));
    let oversize = format!("{{\"name\":\"{}\"}}", "x".repeat(8_192));
    let request = Request::builder()
        .method("POST")
        .uri("/api/organizations")
        .header("idempotency-key", "fixture-key")
        .extension(context)
        .body(Body::from(oversize))
        .expect("a valid request");
    let sent = send(router, request).await;
    assert_eq!(sent.status, StatusCode::PAYLOAD_TOO_LARGE);
    assert_eq!(api.calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn a_route_that_declares_a_replay_key_refuses_a_request_without_one() {
    let api = Api::new(Answer::Declared);
    let context = owner_context(ControlScopeSet::CENTRAL);
    let router = plane(Arc::clone(&api), edge(Resolver::new(AccountState::Active)));
    let request = Request::builder()
        .method("POST")
        .uri("/api/organizations")
        .extension(context)
        .body(Body::from("{}"))
        .expect("a valid request");
    let sent = send(router, request).await;
    assert_eq!(sent.status, StatusCode::BAD_REQUEST);
    assert!(sent.body.contains("Idempotency-Key"), "{}", sent.body);
    assert_eq!(api.calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn a_route_that_declares_no_replay_key_refuses_one_that_is_supplied() {
    let api = Api::new(Answer::Declared);
    let context = owner_context(ControlScopeSet::CENTRAL);
    let router = plane(Arc::clone(&api), edge(Resolver::new(AccountState::Active)));
    let request = Request::builder()
        .method("GET")
        .uri("/api/organizations")
        .header("idempotency-key", "fixture-key")
        .extension(context)
        .body(Body::empty())
        .expect("a valid request");
    let sent = send(router, request).await;
    assert_eq!(sent.status, StatusCode::BAD_REQUEST);
    assert_eq!(api.calls.load(Ordering::SeqCst), 0);
}

// ---------------------------------------------------------------------------
// Dispatch
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_handler_answer_renders_the_status_the_route_declares() {
    let api = Api::new(Answer::Success);
    let context = owner_context(ControlScopeSet::CENTRAL);
    let router = plane(Arc::clone(&api), edge(Resolver::new(AccountState::Active)));
    let sent = send(router, request_for(RouteId::ApiKeyRevoke, Some(context))).await;
    assert_eq!(sent.status, StatusCode::NO_CONTENT);
    assert!(sent.body.is_empty());
    assert_eq!(api.calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn a_bounded_page_read_reaches_its_handler_and_renders_json() {
    let api = Api::new(Answer::Success);
    let context = owner_context(ControlScopeSet::CENTRAL);
    let router = plane(Arc::clone(&api), edge(Resolver::new(AccountState::Active)));
    let sent = send(router, request_for(RouteId::ApiKeysList, Some(context))).await;
    assert_eq!(sent.status, StatusCode::OK, "{}", sent.body);
    assert_eq!(sent.body, "{\"items\":[]}");
}

#[tokio::test]
async fn api_key_list_decodes_its_query_target_after_edge_admission() {
    for uri in ["/api/api-keys", "/api/api-keys?workspaceId=not-a-workspace"] {
        let api = Api::new(Answer::Success);
        let context = owner_context(ControlScopeSet::CENTRAL);
        let router = plane(Arc::clone(&api), edge(Resolver::new(AccountState::Active)));
        let request = Request::builder()
            .method("GET")
            .uri(uri)
            .extension(context)
            .body(Body::empty())
            .expect("a valid request");
        let sent = send(router, request).await;
        assert_eq!(sent.status, StatusCode::BAD_REQUEST, "{uri}: {}", sent.body);
        assert!(
            sent.body.contains("invalid_request"),
            "{uri}: {}",
            sent.body
        );
        assert_eq!(api.calls.load(Ordering::SeqCst), 0, "{uri}");
    }
}

#[tokio::test]
async fn api_key_create_decodes_its_body_target_after_edge_admission() {
    let api = Api::new(Answer::Success);
    let context = owner_context(ControlScopeSet::CENTRAL);
    let router = plane(Arc::clone(&api), edge(Resolver::new(AccountState::Active)));
    let request = Request::builder()
        .method("POST")
        .uri("/api/api-keys")
        .header("idempotency-key", "fixture-key")
        .extension(context)
        .body(Body::from("{}"))
        .expect("a valid request");
    let sent = send(router, request).await;
    assert_eq!(sent.status, StatusCode::BAD_REQUEST, "{}", sent.body);
    assert!(sent.body.contains("invalid_request"), "{}", sent.body);
    assert_eq!(api.calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn an_undeclared_error_code_never_reaches_the_wire() {
    let api = Api::new(Answer::Undeclared);
    let context = owner_context(ControlScopeSet::CENTRAL);
    let router = plane(Arc::clone(&api), edge(Resolver::new(AccountState::Active)));
    let sent = send(router, request_for(RouteId::ApiKeyRevoke, Some(context))).await;
    assert_eq!(sent.status, StatusCode::INTERNAL_SERVER_ERROR);
    assert!(
        sent.body.contains("\"code\":\"internal_error\""),
        "{}",
        sent.body
    );
    assert!(
        !sent.body.contains("\"code\":\"session_not_idle\""),
        "the undeclared code is never the rendered code: {}",
        sent.body
    );
}

#[tokio::test]
async fn an_undeclared_query_parameter_is_refused() {
    let api = Api::new(Answer::Success);
    let context = owner_context(ControlScopeSet::CENTRAL);
    let router = plane(Arc::clone(&api), edge(Resolver::new(AccountState::Active)));
    let request = Request::builder()
        .method("GET")
        .uri("/api/organizations?sortBy=name")
        .extension(context)
        .body(Body::empty())
        .expect("a valid request");
    let sent = send(router, request).await;
    assert_eq!(sent.status, StatusCode::BAD_REQUEST);
    assert!(sent.body.contains("invalid_request"), "{}", sent.body);
}

#[tokio::test]
async fn every_error_envelope_carries_the_authorizer_request_id() {
    let api = Api::new(Answer::Declared);
    let context = owner_context(ControlScopeSet::CENTRAL);
    let router = plane(
        Arc::clone(&api),
        edge(Resolver::new(AccountState::PausedTopUpRequired)),
    );
    let sent = send(router, request_for(RouteId::MembershipsList, Some(context))).await;
    assert!(sent.body.contains("req-fixture"), "{}", sent.body);
}

#[tokio::test]
async fn a_workspace_key_may_not_reach_a_route_that_admits_only_a_person() {
    let api = Api::new(Answer::Declared);
    let mut context = owner_context(ControlScopeSet::ALL);
    context.kind = ContextPrincipalKind::WorkspaceKey;
    context.principal_id = raw_uuid(API_KEY);
    context.workspace_id = Some(raw_uuid(WORKSPACE));
    context.organization_id = Some(raw_uuid(ORGANIZATION));
    context.region = Some(Region::EuWest1);
    context.memberships = Vec::new();
    let router = plane(Arc::clone(&api), edge(Resolver::new(AccountState::Active)));
    let sent = send(router, request_for(RouteId::MembershipsList, Some(context))).await;
    assert_eq!(sent.status, StatusCode::FORBIDDEN);
    assert_eq!(api.calls.load(Ordering::SeqCst), 0);
}

/// A clock that advances one millisecond on every read.
///
/// This is not a pathological fixture: `SystemClock` truncates to whole
/// milliseconds, so two reads inside one request differ whenever the
/// millisecond happens to tick between them. Under a wall clock that is an
/// intermittent failure; here it is every time.
#[derive(Debug)]
struct TickingClock {
    reads: AtomicUsize,
}

impl Clock for TickingClock {
    fn now(&self) -> OffsetDateTime {
        let tick = self.reads.fetch_add(1, Ordering::SeqCst);
        OffsetDateTime::from_unix_timestamp_nanos(
            (i128::from(NOW_MS) + i128::try_from(tick).unwrap_or(0)) * 1_000_000,
        )
        .expect("a representable instant")
    }
}

#[tokio::test]
async fn an_anonymous_request_is_admitted_even_when_the_millisecond_ticks_mid_request() {
    // The credential-free context is minted with a one-millisecond window and
    // then verified. Reading the clock twice made that window lapse whenever
    // the two readings straddled a millisecond, which refused both public
    // device-flow routes with `401 unauthenticated` — the two routes an
    // unauthenticated CLI has nothing else to call.
    let api = Api::new(Answer::Declared);
    let stack = EdgeStack::new(
        HttpConfig::resolve("dev", "eu-west-1", "central-identity-api", 4_096, 5_000)
            .expect("a valid configuration"),
        Resolver::new(AccountState::Active),
        Arc::new(TickingClock {
            reads: AtomicUsize::new(0),
        }),
        Arc::new(aex_control_domain::CursorSecret::new([3_u8; 32])),
    );
    let router = plane(Arc::clone(&api), stack);
    for id in [
        RouteId::DeviceAuthorizationCreate,
        RouteId::DeviceTokenCreate,
    ] {
        let sent = send(router.clone(), request_for(id, None)).await;
        assert_ne!(
            sent.status,
            StatusCode::UNAUTHORIZED,
            "`{id:?}` refused an anonymous caller it declares as anonymous: {}",
            sent.body
        );
        assert!(
            !sent.body.contains("\"code\":\"unauthenticated\""),
            "`{id:?}`: {}",
            sent.body
        );
    }
    assert_eq!(
        api.calls.load(Ordering::SeqCst),
        2,
        "both anonymous routes must reach their handler"
    );
}

// ---------------------------------------------------------------------------
// In-process authentication
//
// The gateway that used to verify credentials cannot integrate an ECS service,
// so a central composition reachable through a load balancer verifies them
// itself. These drive the whole router: nothing below constructs a context, so
// what decides the outcome is the credential — or its absence, or its forgery.
// ---------------------------------------------------------------------------

use aex_central_http::admission::{CredentialAdmission, CredentialPeppers};
use aex_control_app::ports::{
    AccountActorState, AuthorizationReader, CentralActorState, SigningKeyRecord, StoreError,
    WorkspaceKeyState,
};
use aex_identity_domain::credential::{
    CredentialKind, Pepper, PepperVersion, SecretRng, mint, verifier,
};

/// A deterministic secret source, so a minted credential is stable.
#[derive(Debug)]
struct FixedRng;

impl SecretRng for FixedRng {
    fn fill(&self, out: &mut [u8]) {
        out.fill(23);
    }
}

const PEPPER_BYTES: [u8; 32] = [17; 32];

/// One pepper, version 1, for whichever purpose is asked.
#[derive(Debug, Clone, Copy)]
struct OnePepper;

#[async_trait]
impl CredentialPeppers for OnePepper {
    async fn pepper(&self, _kind: CredentialKind, version: PepperVersion) -> Option<Pepper> {
        (version.get() == 1).then(|| Pepper::new(PEPPER_BYTES))
    }
}

/// The one account-token row this suite resolves.
#[derive(Debug, Clone)]
struct OneToken {
    state: CentralActorState,
}

#[async_trait]
impl AuthorizationReader for OneToken {
    async fn resolve_workspace_key(
        &self,
        _key_id: Uuid,
    ) -> Result<Option<WorkspaceKeyState>, StoreError> {
        Ok(None)
    }

    async fn resolve_account_token_for_workspace(
        &self,
        _token_id: Uuid,
        _workspace_id: Uuid,
        _now: OffsetDateTime,
    ) -> Result<Option<AccountActorState>, StoreError> {
        Ok(None)
    }

    async fn resolve_session_for_workspace(
        &self,
        _session_id: Uuid,
        _workspace_id: Uuid,
        _now: OffsetDateTime,
    ) -> Result<Option<AccountActorState>, StoreError> {
        Ok(None)
    }

    async fn resolve_account_token_central(
        &self,
        _token_id: Uuid,
        _now: OffsetDateTime,
    ) -> Result<Option<CentralActorState>, StoreError> {
        Ok(Some(self.state.clone()))
    }

    async fn resolve_dashboard_session_central(
        &self,
        _session_id: Uuid,
        _now: OffsetDateTime,
    ) -> Result<Option<CentralActorState>, StoreError> {
        Ok(Some(self.state.clone()))
    }

    async fn verification_key_set(&self) -> Result<Vec<SigningKeyRecord>, StoreError> {
        Ok(Vec::new())
    }

    async fn active_signing_key(&self) -> Result<SigningKeyRecord, StoreError> {
        Err(StoreError::NotFound)
    }
}

/// A minted account token and the row that verifies it.
fn minted_token() -> (String, CentralActorState) {
    let (secret, digest) = mint(
        CredentialKind::AccountToken,
        None,
        raw_uuid(CREDENTIAL),
        &FixedRng,
    );
    let state = CentralActorState {
        credential_id: raw_uuid(CREDENTIAL),
        user_id: raw_uuid(USER),
        scopes: ControlScopeSet::CENTRAL,
        verifier: *verifier(&Pepper::new(PEPPER_BYTES), &digest).as_bytes(),
        pepper_version: 1,
        credential_revoked: false,
        credential_expired: false,
        user_active: true,
        memberships: vec![OrgMembership {
            organization_id: raw_uuid(ORGANIZATION),
            membership_id: raw_uuid(MEMBERSHIP),
            role: OrgRole::Owner,
        }],
    };
    (secret.expose().to_owned(), state)
}

/// An edge that verifies the credential itself, over `state`.
fn verifying_edge(state: CentralActorState) -> EdgeStack {
    edge(Resolver::new(AccountState::Active)).with_in_process_authentication(Arc::new(
        CredentialAdmission::new(OneToken { state }, OnePepper, 30_000),
    ))
}

/// Adds a bearer credential to a request built for `id`.
fn bearing(id: RouteId, secret: &str) -> Request<Body> {
    let (mut parts, body) = request_for(id, None).into_parts();
    parts.headers.insert(
        axum::http::header::AUTHORIZATION,
        format!("Bearer {secret}").parse().expect("a header value"),
    );
    Request::from_parts(parts, body)
}

#[tokio::test]
async fn a_verified_credential_reaches_the_handler_with_no_gateway_in_front() {
    let (secret, state) = minted_token();
    let api = Api::new(Answer::Declared);
    let router = plane(Arc::clone(&api), verifying_edge(state));
    let sent = send(router, bearing(RouteId::OrganizationsList, &secret)).await;
    assert_ne!(
        sent.status,
        StatusCode::UNAUTHORIZED,
        "a valid credential was refused: {}",
        sent.body
    );
    assert_eq!(
        api.calls.load(Ordering::SeqCst),
        1,
        "the handler must run for a verified credential"
    );
}

#[tokio::test]
async fn a_forged_credential_is_refused_in_process_and_reaches_no_handler() {
    // The row exists and its id echoes; only the secret is wrong. Behind the
    // gateway this request never reached the service at all. It must now be
    // refused here, by this process, before anything else runs.
    let (secret, mut state) = minted_token();
    state.verifier = [99; 32];
    let api = Api::new(Answer::Declared);
    let router = plane(Arc::clone(&api), verifying_edge(state));
    let sent = send(router, bearing(RouteId::OrganizationsList, &secret)).await;
    assert_eq!(sent.status, StatusCode::UNAUTHORIZED, "{}", sent.body);
    assert!(sent.body.contains("unauthenticated"), "{}", sent.body);
    assert_eq!(
        api.calls.load(Ordering::SeqCst),
        0,
        "no handler runs for a credential that does not verify"
    );
}

#[tokio::test]
async fn a_credentialed_route_with_no_credential_at_all_is_refused_in_process() {
    let (_, state) = minted_token();
    let api = Api::new(Answer::Declared);
    let router = plane(Arc::clone(&api), verifying_edge(state));
    let sent = send(router, request_for(RouteId::OrganizationsList, None)).await;
    assert_eq!(sent.status, StatusCode::UNAUTHORIZED, "{}", sent.body);
    assert_eq!(api.calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn an_injected_context_never_admits_a_request_that_verifies_no_credential() {
    // This is the bypass a load balancer creates: behind an ALB every header
    // and every extension is caller-authored. A composition that verifies
    // credentials itself must ignore an ambient context entirely, or its
    // authentication can be skipped by asserting the principal one wants.
    let (_, state) = minted_token();
    let api = Api::new(Answer::Declared);
    let router = plane(Arc::clone(&api), verifying_edge(state));
    let sent = send(
        router,
        request_for(
            RouteId::OrganizationsList,
            Some(owner_context(ControlScopeSet::ALL)),
        ),
    )
    .await;
    assert_eq!(
        sent.status,
        StatusCode::UNAUTHORIZED,
        "an asserted principal was admitted without a credential: {}",
        sent.body
    );
    assert_eq!(api.calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn the_two_device_flow_routes_still_admit_a_caller_with_no_credential() {
    let (_, state) = minted_token();
    let api = Api::new(Answer::Declared);
    let router = plane(Arc::clone(&api), verifying_edge(state));
    for id in [
        RouteId::DeviceAuthorizationCreate,
        RouteId::DeviceTokenCreate,
    ] {
        let sent = send(router.clone(), request_for(id, None)).await;
        assert_ne!(
            sent.status,
            StatusCode::UNAUTHORIZED,
            "`{id:?}` refused an anonymous caller it declares as anonymous: {}",
            sent.body
        );
    }
    assert_eq!(api.calls.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn a_rejected_credential_never_becomes_an_anonymous_one() {
    // A device-flow route admits *no credential*. It must not admit a rejected
    // one as though none had been sent.
    let (_, state) = minted_token();
    let api = Api::new(Answer::Declared);
    let router = plane(Arc::clone(&api), verifying_edge(state));
    let sent = send(
        router,
        bearing(RouteId::DeviceTokenCreate, "not-an-aex-credential"),
    )
    .await;
    assert_eq!(sent.status, StatusCode::UNAUTHORIZED, "{}", sent.body);
    assert_eq!(api.calls.load(Ordering::SeqCst), 0);
}
