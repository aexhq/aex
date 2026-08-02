//! The `axum` composition: one mount per generated central route group.
//!
//! Every mount is a **loop over the group's generated route slice**, never a
//! hand-written list of templates. A route that is authored and not mounted is
//! therefore a failing composition test rather than a runtime `404`, and the
//! mounted set cannot drift from the contract bundle.
//!
//! The middleware reads the route table, not the handler:
//! `RouteDescriptor::{required_scope, alt_principal, idempotency, body_class,
//! etag, pause_exempt}` drive precedence stages 1–9, and the generated
//! `dispatch_*` runs stages 7 and 12–13.

use std::collections::BTreeMap;
use std::sync::Arc;

use aex_control_app::ports::Clock;
use aex_control_domain::{
    AccountState, Action, CursorSecret, Granted, PrincipalKindTag, requirement,
};
use aex_wire::dispatch::{DispatchOutcome, RawRequest, RawResponse, RequestLimits};
use aex_wire::error::{ApiError, WireError};
use aex_wire::idempotency::PrincipalScope;
use aex_wire::ids::{ApiKeyId, OrganizationId, PrefixedId, UserId, Uuid7, WorkspaceId};
use aex_wire::routes::{Plane, RouteId, match_route, route};
use aex_wire::server::{
    ApiKeysApi, AuthApi, BillingApi, BootstrapApi, CentralOperationsApi, IdentityApi,
    OrganizationsApi, RequestContext, RouteGroup, WorkspacesApi, dispatch_api_keys, dispatch_auth,
    dispatch_billing, dispatch_bootstrap, dispatch_central_operations, dispatch_identity,
    dispatch_organizations, dispatch_workspaces,
};
use aex_wire::types::HttpMethod;
use aex_wire::types::RequestId;
use axum::extract::{Request, State};
use axum::http::{HeaderMap, StatusCode, Uri, header};
use axum::response::{IntoResponse as _, Response};
use axum::routing::{MethodFilter, MethodRouter, on};
use uuid::Uuid;

use crate::authorizer::{CentralAuthorizerContext, ContextError, ContextPrincipalKind};
use crate::config::HttpConfig;
use crate::error::EdgeError;
use crate::headers;
use crate::target::{TargetPath, TargetResolver, admit_request};

/// The shared services every mounted group runs behind.
#[derive(Clone)]
pub struct EdgeStack {
    config: HttpConfig,
    resolver: Arc<dyn TargetResolver>,
    clock: Arc<dyn Clock>,
    cursor_secret: Arc<CursorSecret>,
}

impl std::fmt::Debug for EdgeStack {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("EdgeStack")
            .field("config", &self.config)
            .finish_non_exhaustive()
    }
}

impl EdgeStack {
    /// Builds the stack a composition root hands to every mount.
    #[must_use]
    pub fn new(
        config: HttpConfig,
        resolver: Arc<dyn TargetResolver>,
        clock: Arc<dyn Clock>,
        cursor_secret: Arc<CursorSecret>,
    ) -> Self {
        Self {
            config,
            resolver,
            clock,
            cursor_secret,
        }
    }

    /// The configuration this stack was built with.
    #[must_use]
    pub const fn config(&self) -> &HttpConfig {
        &self.config
    }

    /// The cursor signing secret, for a handler that mints a continuation.
    #[must_use]
    pub fn cursor_secret(&self) -> &CursorSecret {
        &self.cursor_secret
    }

    /// The instant this request is evaluated against, in epoch milliseconds.
    #[must_use]
    pub fn now_ms(&self) -> i64 {
        i64::try_from(
            self.clock
                .now()
                .unix_timestamp_nanos()
                .div_euclid(1_000_000),
        )
        .unwrap_or(i64::MAX)
    }

    /// The body bounds this composition enforces.
    #[must_use]
    pub const fn limits(&self) -> RequestLimits {
        self.config.limits
    }
}

/// What the edge established before the generated dispatcher ran.
#[derive(Debug, Clone)]
pub struct Admitted {
    /// The context the generated handler receives.
    pub context: RequestContext,
    /// What authorization granted.
    pub granted: Granted,
}

/// The per-group axum state.
pub struct GroupState<A> {
    api: Arc<A>,
    edge: EdgeStack,
}

impl<A> Clone for GroupState<A> {
    fn clone(&self) -> Self {
        Self {
            api: Arc::clone(&self.api),
            edge: self.edge.clone(),
        }
    }
}

/// Every template and method one group mounts, for the composition test.
#[must_use]
pub fn mounted_routes(group: RouteGroup) -> Vec<(&'static str, HttpMethod)> {
    group
        .routes()
        .iter()
        .map(|id| {
            let descriptor = route(*id);
            (descriptor.template, descriptor.method)
        })
        .collect()
}

/// The `401` an absent authorizer context renders as.
const fn unauthenticated() -> EdgeError {
    EdgeError::Context(ContextError::Missing("aex.principalKind"))
}

/// Whether `id` admits a request that carries no credential at all.
fn admits_anonymous(id: RouteId) -> bool {
    Action::central(id).is_ok_and(|action| {
        requirement(action)
            .principal_kinds
            .admits(PrincipalKindTag::Anonymous)
    })
}

/// The credential-free context the two device-flow routes run under.
///
/// The window is one millisecond wide because there is nothing to cache: no
/// credential was resolved, so nothing about it can go stale. `[now, now+1)`
/// therefore admits exactly the instant it was minted for, which is why
/// [`admit_edge`] reads the clock **once** and verifies against that same
/// reading. A second reading would lapse the window whenever the millisecond
/// happened to tick between the two, refusing every anonymous request — which
/// is to say both public device-flow routes, intermittently.
fn anonymous_context(now_ms: i64) -> CentralAuthorizerContext {
    CentralAuthorizerContext {
        request_id: fallback_request_id(),
        kind: ContextPrincipalKind::Anonymous,
        principal_id: Uuid::nil(),
        credential_id: None,
        workspace_id: None,
        organization_id: None,
        region: None,
        memberships: Vec::new(),
        scopes: aex_control_domain::ScopeSet::EMPTY,
        account_state: AccountState::Active,
        issued_at_ms: now_ms,
        expires_at_ms: now_ms.saturating_add(1),
    }
}

/// The replay principal an anonymous request is recorded under.
///
/// Every credential-free caller shares it, so two anonymous callers presenting
/// the same `Idempotency-Key` for the same intent share one grant.
/// TODO(cross-stream): the accepted design puts a per-IP API Gateway usage-plan
/// throttle in front of the two device-flow routes for exactly this reason;
/// infrastructure owns that rule.
fn anonymous_principal() -> Uuid7 {
    Uuid7::compose(0, [0; 10])
}

/// Runs precedence stages 1–9.
///
/// Each stage is one early return in the wire contract's order, so the ordering
/// is readable rather than implied by nesting. A failure carries the request id
/// it had established by then, because an error envelope with no request id is
/// an error nobody can trace.
async fn admit_edge(
    edge: &EdgeStack,
    context: Option<CentralAuthorizerContext>,
    method: HttpMethod,
    uri: &Uri,
    headers: &HeaderMap,
    body_len: usize,
) -> Result<(RouteId, Admitted), (RequestId, EdgeError)> {
    let fallback = fallback_request_id();
    // One reading, for the whole request. Every window comparison below is
    // against this instant, so a request cannot be inside its assertion's
    // window at one stage and outside it at the next — and a credential-free
    // context cannot lapse in the microseconds between being minted and being
    // verified.
    let now_ms = edge.now_ms();

    // 1. Transport envelope: the route comes from the one table.
    let Some((id, binding)) = match_route(Plane::Central, method, uri.path()) else {
        return Err((fallback, EdgeError::NoRoute));
    };
    let path = TargetPath::from_binding(&binding);

    // 2. Authentication: the authorizer context, verified against its own window.
    //
    // The two device-flow routes admit no credential at all, so an absent
    // context is their normal case rather than a failure. Every other route
    // refuses: an absent context is exactly what an unauthenticated request
    // looks like.
    let context = match context {
        Some(context) => context,
        None if admits_anonymous(id) => anonymous_context(now_ms),
        None => return Err((fallback, unauthenticated())),
    };
    let request_id = context.request_id.clone();
    if let Err(error) = context.verify(now_ms) {
        return Err((request_id, EdgeError::Context(error)));
    }

    // 3. The body bound, before anything parses it.
    if body_len > edge.limits().max_json_body_bytes {
        return Err((
            request_id,
            EdgeError::PayloadTooLarge {
                found: body_len,
                bound: edge.limits().max_json_body_bytes,
            },
        ));
    }

    // 4. Declared headers, strict in both directions.
    let declared = match headers::declared(id, headers) {
        Ok(declared) => declared,
        Err(error) => return Err((request_id, error)),
    };

    // 5–6. Resolve the target, decide, then gate on the current account state.
    let Ok(action) = Action::central(id) else {
        return Err((request_id, EdgeError::Internal("a regional route matched")));
    };
    let principal = context.principal();
    let granted = match admit_request(edge.resolver.as_ref(), &principal, action, &path).await {
        Ok(granted) => granted,
        Err(error) => return Err((request_id, error)),
    };

    // 7. The context the generated handler receives.
    let scope = match principal_scope(&context, &granted) {
        Ok(scope) => scope,
        Err(error) => return Err((request_id, error)),
    };
    Ok((
        id,
        Admitted {
            context: RequestContext {
                request_id,
                route: id,
                principal: scope,
                granted_scopes: granted.effective_scopes.to_wire(),
                idempotency_key: declared.idempotency_key,
                operation_id: declared.operation_id,
                if_match: declared.if_match,
                accept: declared.accept,
            },
            granted,
        },
    ))
}

/// The request id an envelope carries when authentication never established one.
///
/// # Panics
///
/// Never: the literal satisfies the `RequestId` grammar.
fn fallback_request_id() -> RequestId {
    RequestId::parse("unattributed").expect("a valid request id literal")
}

/// The replay identity's view of the principal.
fn principal_scope(
    context: &CentralAuthorizerContext,
    granted: &Granted,
) -> Result<PrincipalScope, EdgeError> {
    match context.kind {
        ContextPrincipalKind::WorkspaceKey => Ok(PrincipalScope::WorkspaceKey {
            key: wire_id::<ApiKeyId>(context.principal_id)?,
            workspace: wire_id::<WorkspaceId>(context.workspace_id.unwrap_or_default())?,
            organization: wire_id::<OrganizationId>(context.organization_id.unwrap_or_default())?,
        }),
        ContextPrincipalKind::Anonymous => Ok(PrincipalScope::Account {
            user: UserId::from_uuid7(anonymous_principal()),
            organization: None,
        }),
        ContextPrincipalKind::Account | ContextPrincipalKind::UserSession => {
            Ok(PrincipalScope::Account {
                user: wire_id::<UserId>(context.principal_id)?,
                organization: granted
                    .organization_id
                    .map(wire_id::<OrganizationId>)
                    .transpose()?,
            })
        }
    }
}

/// Turns a stored identifier into its wire form.
///
/// Every AEX row id is a `UUIDv7`. One that is not is a schema defect, never
/// caller input, so it renders as a `500` rather than a `400`.
fn wire_id<T: PrefixedId>(id: Uuid) -> Result<T, EdgeError> {
    Uuid7::from_bytes(*id.as_bytes())
        .map(T::from_uuid7)
        .map_err(|_| EdgeError::Internal("a stored identifier is not a UUIDv7"))
}

/// Renders one dispatched response.
fn render(response: RawResponse) -> Response {
    let mut builder = Response::builder().status(response.status);
    if let Some(content_type) = response.content_type {
        builder = builder.header(header::CONTENT_TYPE, content_type);
    }
    if let Some(etag) = response.etag {
        builder = builder.header(header::ETAG, etag.as_str());
    }
    if let Some(location) = response.location {
        builder = builder.header(header::LOCATION, location);
    }
    builder
        .body(axum::body::Body::from(response.body))
        .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
}

/// Renders one wire failure.
fn render_wire(request_id: &RequestId, failure: WireError) -> Response {
    let (status, envelope, retry_after) = failure.into_response_parts(request_id, None);
    render_envelope(status, &envelope, retry_after)
}

/// Renders one edge failure.
fn render_edge(request_id: &RequestId, failure: EdgeError) -> Response {
    let (status, envelope, retry_after) = failure.render(request_id, None);
    render_envelope(status, &envelope, retry_after)
}

fn render_envelope(
    status: u16,
    envelope: &ApiError,
    retry_after: Option<core::time::Duration>,
) -> Response {
    let mut builder = Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, "application/json");
    if let Some(after) = retry_after {
        builder = builder.header(header::RETRY_AFTER, after.as_secs());
    }
    let body = serde_json::to_vec(envelope).unwrap_or_else(|_| b"{}".to_vec());
    builder
        .body(axum::body::Body::from(body))
        .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
}

/// Emits one `mount_*` and its handler for a generated group.
///
/// The eight expansions differ only in the trait and the dispatcher; the mount
/// loop, the precedence stages and the rendering are one body.
macro_rules! mount_group {
    ($mount:ident, $handler:ident, $trait:ident, $dispatch:ident, $group:expr, $doc:literal) => {
        #[doc = $doc]
        ///
        /// The mounted set is `RouteGroup::routes()` and nothing else, so a
        /// route the contract adds is mounted without a source change here.
        pub fn $mount<A: $trait>(api: Arc<A>, edge: EdgeStack) -> axum::Router {
            let mut by_template: BTreeMap<&'static str, MethodRouter<GroupState<A>>> =
                BTreeMap::new();
            for id in $group.routes() {
                let descriptor = route(*id);
                let filter = match descriptor.method {
                    HttpMethod::Get => MethodFilter::GET,
                    HttpMethod::Put => MethodFilter::PUT,
                    HttpMethod::Post => MethodFilter::POST,
                    HttpMethod::Delete => MethodFilter::DELETE,
                };
                let installed = on(filter, $handler::<A>);
                match by_template.remove(descriptor.template) {
                    Some(existing) => {
                        by_template.insert(descriptor.template, existing.merge(installed));
                    }
                    None => {
                        by_template.insert(descriptor.template, installed);
                    }
                }
            }
            let mut router = axum::Router::new();
            for (template, method_router) in by_template {
                // `descriptor.template` is already axum 0.8 path syntax.
                router = router.route(template, method_router);
            }
            router.with_state(GroupState { api, edge })
        }

        async fn $handler<A: $trait>(
            State(state): State<GroupState<A>>,
            request: Request,
        ) -> Response {
            let method = request.method().clone();
            let uri = request.uri().clone();
            let headers = request.headers().clone();
            let context = request
                .extensions()
                .get::<CentralAuthorizerContext>()
                .cloned();
            let Some(parsed) = HttpMethod::parse(method.as_str()) else {
                return render_edge(&fallback_request_id(), EdgeError::NoRoute);
            };
            let bound = state.edge.limits().max_json_body_bytes;
            // Reading is bounded at one byte over the limit, so an oversize body
            // is refused after a bounded read rather than buffered whole.
            let Ok(body) = axum::body::to_bytes(request.into_body(), bound.saturating_add(1)).await
            else {
                return render_edge(
                    &fallback_request_id(),
                    EdgeError::PayloadTooLarge {
                        found: bound.saturating_add(1),
                        bound,
                    },
                );
            };
            let admitted =
                admit_edge(&state.edge, context, parsed, &uri, &headers, body.len()).await;
            let (id, admitted) = match admitted {
                Ok(resolved) => resolved,
                Err((request_id, failure)) => return render_edge(&request_id, failure),
            };
            let Some((_, binding)) = match_route(Plane::Central, parsed, uri.path()) else {
                return render_edge(&admitted.context.request_id, EdgeError::NoRoute);
            };
            let query = uri.query().unwrap_or_default().to_owned();
            let raw = RawRequest {
                route: id,
                path: binding,
                query: &query,
                body: &body,
            };

            // Stages 8 and 9: the replay identity, strict in both directions.
            if let Err(failure) = aex_wire::dispatch::request_identity(&admitted.context, &raw) {
                return render_wire(&admitted.context.request_id, failure);
            }

            match $dispatch(
                state.api.as_ref(),
                &admitted.context,
                raw,
                state.edge.limits(),
            )
            .await
            {
                Ok(DispatchOutcome::Unary(response)) => render(response),
                // No central group declares an NDJSON route, so `NoStream` is
                // uninhabited and this arm is unconstructible.
                Ok(DispatchOutcome::Ndjson(never)) => match never.0 {},
                Err(failure) => render_wire(&admitted.context.request_id, failure),
            }
        }
    };
}

mount_group!(
    mount_api_keys_api,
    handle_api_keys,
    ApiKeysApi,
    dispatch_api_keys,
    RouteGroup::ApiKeys,
    "Mounts `central:api-keys`: three routes."
);
mount_group!(
    mount_auth_api,
    handle_auth,
    AuthApi,
    dispatch_auth,
    RouteGroup::Auth,
    "Mounts `central:auth`: the two public device-flow routes."
);
mount_group!(
    mount_billing_api,
    handle_billing,
    BillingApi,
    dispatch_billing,
    RouteGroup::Billing,
    "Mounts `central:billing`: eight routes, served by `finance-api`."
);
mount_group!(
    mount_bootstrap_api,
    handle_bootstrap,
    BootstrapApi,
    dispatch_bootstrap,
    RouteGroup::Bootstrap,
    "Mounts `central:bootstrap`: the one-request dashboard shell read."
);
mount_group!(
    mount_central_operations_api,
    handle_central_operations,
    CentralOperationsApi,
    dispatch_central_operations,
    RouteGroup::CentralOperations,
    "Mounts `central:operations`: three routes."
);
mount_group!(
    mount_identity_api,
    handle_identity,
    IdentityApi,
    dispatch_identity,
    RouteGroup::Identity,
    "Mounts `central:identity`: the account read."
);
mount_group!(
    mount_organizations_api,
    handle_organizations,
    OrganizationsApi,
    dispatch_organizations,
    RouteGroup::Organizations,
    "Mounts `central:organizations`: five routes."
);
mount_group!(
    mount_workspaces_api,
    handle_workspaces,
    WorkspacesApi,
    dispatch_workspaces,
    RouteGroup::Workspaces,
    "Mounts `central:workspaces`: four routes."
);
