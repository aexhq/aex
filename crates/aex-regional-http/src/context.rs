//! Typed facts established by the edge before a handler runs.

use aex_wire::idempotency::PrincipalScope;
use aex_wire::ids::{OperationId, OrganizationId, WorkspaceId};
use aex_wire::routes::RouteId;
use aex_wire::scopes::ScopeSet;
use aex_wire::server::{AcceptKind, RequestContext as WireContext};
use aex_wire::types::{ETag, Region, RequestId, Timestamp};
use time::OffsetDateTime;

use crate::idempotency::IdempotencyIdentity;

/// Monotonic authorization revisions, as the regional projection published them.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct AuthorizationEpochs {
    /// Workspace-key revision.
    pub key: u64,
    /// Membership revision.
    pub membership: u64,
    /// Workspace revision.
    pub workspace: u64,
    /// Account-state revision.
    pub account: u64,
}

/// Account policy frozen into one verified request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AccountState {
    /// Paid work is admitted.
    Active,
    /// Only generated pause-exempt routes are admitted.
    Paused,
}

/// Verified, credential-bound regional authority.
///
/// # Why there is no issue or expiry instant
///
/// Both were the central assertion's: it was minted at one instant and hard
/// expired thirty seconds later, and this record carried that window so a
/// handler could see it. There is no assertion any more and therefore no window
/// to carry — every fact here was read from the regional projection during the
/// request that produced it, and a long-lived transport re-reads them through
/// [`crate::edge::RegionalEdge::revalidate`] rather than trusting a lifetime.
/// A field holding "now" and "now" would have been a lifetime nobody enforced.
#[derive(Clone, PartialEq, Eq)]
pub struct RegionalAuthorization {
    /// Stable principal scope, never credential material.
    pub principal: PrincipalScope,
    /// The stable, domain-separated identity of the presented credential.
    pub credential_binding: [u8; 32],
    /// Owning organization.
    pub organization_id: OrganizationId,
    /// Authorized workspace.
    pub workspace_id: WorkspaceId,
    /// Immutable placement.
    pub placement: Region,
    /// Effective scopes.
    pub scopes: ScopeSet,
    /// Current account policy.
    pub account_state: AccountState,
    /// Monotonic revisions.
    pub epochs: AuthorizationEpochs,
}

impl std::fmt::Debug for RegionalAuthorization {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("RegionalAuthorization")
            .field("principal", &self.principal)
            .field("credential_binding", &"<redacted>")
            .field("organization_id", &self.organization_id)
            .field("workspace_id", &self.workspace_id)
            .field("placement", &self.placement)
            .field("scopes", &self.scopes)
            .field("account_state", &self.account_state)
            .field("epochs", &self.epochs)
            .finish()
    }
}

/// Workspace safety limits resolved before request decoding.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EffectiveLimits {
    /// Effective encoded JSON body bound.
    pub json_body_bytes: usize,
    /// Effective encoded OTLP body bound from `telemetry.batch.encoded_bytes`.
    pub otlp_body_bytes: usize,
    /// Effective list item bound.
    pub query_page_items: usize,
    /// Effective serialized page byte bound.
    pub query_page_bytes: usize,
}

/// Everything an application handler may trust about one request.
#[derive(Debug, Clone)]
pub struct RequestContext {
    /// Diagnostic identity echoed in errors.
    pub request_id: RequestId,
    /// Generated operation id.
    pub route: RouteId,
    /// Verified regional authorization.
    pub auth: RegionalAuthorization,
    /// Effective limits.
    pub limits: EffectiveLimits,
    /// Caller-minted durable operation identity.
    pub operation_id: Option<OperationId>,
    /// Ordinary replay identity.
    pub idempotency: Option<IdempotencyIdentity>,
    /// Strong conditional precondition.
    pub if_match: Option<ETag>,
    /// Edge receipt time.
    pub received_at: OffsetDateTime,
}

impl RequestContext {
    /// The edge receipt time as the workspace timestamp type.
    ///
    /// One conversion, at the boundary where the edge's clock becomes the value
    /// every handler and every authority row shares. A receipt instant outside
    /// the representable range is a broken clock, not a customer condition, so
    /// it is reported rather than clamped.
    ///
    /// # Errors
    ///
    /// [`aex_wire::types::ValueError`] when the instant is outside the
    /// representable range.
    pub fn now(&self) -> Result<Timestamp, aex_wire::types::ValueError> {
        let millis = self.received_at.unix_timestamp_nanos() / 1_000_000;
        Timestamp::from_unix_millis(i64::try_from(millis).unwrap_or(i64::MAX))
    }

    /// Projects the richer regional record onto the context the generated
    /// dispatchers take.
    ///
    /// The wire context is deliberately the smaller of the two: a handler is
    /// told who is asking and what it may replay, and nothing about how the edge
    /// established it. Everything the regional record adds — placement, epochs,
    /// account state, effective limits, credential binding — has already been
    /// used by the stage that produced it.
    #[must_use]
    pub fn to_wire(&self, accept: AcceptKind) -> WireContext {
        WireContext {
            request_id: self.request_id.clone(),
            route: self.route,
            principal: self.auth.principal,
            granted_scopes: self.auth.scopes.clone(),
            idempotency_key: self
                .idempotency
                .as_ref()
                .map(|identity| identity.key.clone()),
            operation_id: self.operation_id,
            if_match: self.if_match.clone(),
            accept,
        }
    }
}
