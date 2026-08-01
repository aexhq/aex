//! Typed facts established by the edge before a handler runs.

use aex_wire::idempotency::PrincipalScope;
use aex_wire::ids::{OperationId, OrganizationId, WorkspaceId};
use aex_wire::routes::RouteId;
use aex_wire::scopes::ScopeSet;
use aex_wire::types::{ETag, Region, RequestId};
use time::OffsetDateTime;

use crate::idempotency::IdempotencyIdentity;

/// Monotonic authorization revisions carried by a central assertion.
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
#[derive(Clone, PartialEq, Eq)]
pub struct RegionalAuthorization {
    /// Stable principal scope, never credential material.
    pub principal: PrincipalScope,
    /// Digest of the presented credential.
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
    /// Assertion issue time.
    pub issued_at: OffsetDateTime,
    /// Hard assertion expiry.
    pub expires_at: OffsetDateTime,
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
            .field("issued_at", &self.issued_at)
            .field("expires_at", &self.expires_at)
            .finish()
    }
}

/// Workspace safety limits resolved before request decoding.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EffectiveLimits {
    /// Effective encoded JSON body bound.
    pub json_body_bytes: usize,
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
