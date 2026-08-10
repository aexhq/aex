//! The coarse ports the control application depends on.

use async_trait::async_trait;
use time::{Duration, OffsetDateTime};
use uuid::Uuid;

use aex_control_domain::{
    AccountProfile, ApiKey, AuditEvent, Epoch, EpochSubjectKind, IntentHash, Invitation, Membership,
    Operation, OperationStatus, OrgRole, Organization, OutboxMessage, ScopeSet, Slug, Workspace,
};

pub use aex_identity_app::ports::{
    Clock, IdFactory, ReconcileIdentity, RequestId, StoreError, TxOutcome, UnknownCommit,
};

/// Everything a control use case knows about its caller.
#[derive(Debug, Clone)]
pub struct RequestContext {
    /// Which request.
    pub request_id: RequestId,
    /// The instant the whole request is evaluated against.
    pub now: OffsetDateTime,
    /// Which principal, already authorized.
    pub principal_kind: aex_control_domain::PrincipalKindTag,
    /// Which principal.
    pub principal_id: Uuid,
    /// The organization the request acts in, once resolved.
    pub organization_id: Option<Uuid>,
    /// The effective scopes admission granted.
    pub effective_scopes: ScopeSet,
}

/// One page of a keyset-paged read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Page<T> {
    /// The rows, in `(created_at, id)` order.
    pub items: Vec<T>,
    /// The `(created_at_ms, id)` to continue from, when the page filled its
    /// limit. `None` means the read reached the end, so no continuation exists.
    ///
    /// A keyset **position**, not an opaque cursor: this port holds no signing
    /// secret, and a store that minted its own token would be a second cursor
    /// authority. The `HTTP` boundary signs the position into the `cur_`
    /// envelope and is the only place that can.
    pub next: Option<(i64, Uuid)>,
}

/// The largest page any control read may return.
pub const MAX_PAGE_LIMIT: u32 = 1_000;

/// The page size a read uses when the caller states none.
///
/// The ceiling stays reachable on request; defaulting to it made every
/// limit-less list read scan and serialize a thousand rows for callers that
/// render a screenful. A default is not a ceiling: an explicit `limit` above
/// this is honoured up to [`MAX_PAGE_LIMIT`].
pub const DEFAULT_PAGE_LIMIT: u32 = 50;

/// Why a cross-plane effect did not complete.
///
/// `Unknown` is deliberately distinct from `Unavailable`: the first means the
/// effect may have happened, the second means it definitely did not. Collapsing
/// them is how a workspace gets provisioned twice.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum EffectError {
    /// The peer refused, and said why.
    #[error("the effect was rejected: {code}")]
    Rejected {
        /// A stable machine code.
        code: &'static str,
        /// Whether the same call may be retried.
        retryable: bool,
    },
    /// The peer could not be reached. Nothing happened.
    #[error("the peer is unavailable")]
    Unavailable,
    /// The response was lost. The effect may or may not have happened.
    #[error("the effect outcome is unknown")]
    Unknown,
}

/// Create an organization, its first owner membership and its finance account.
#[derive(Debug, Clone)]
pub struct CreateOrganizationTx {
    /// The organization id.
    pub preassigned_id: Uuid,
    /// The first membership's id.
    pub preassigned_membership_id: Uuid,
    /// Display name.
    pub name: String,
    /// Globally unique slug.
    pub slug: Slug,
    /// Who is creating it, and becomes its first owner.
    pub created_by_user_id: Uuid,
    /// The replay identity this command was admitted under.
    pub idempotency: IdempotencyRecordKey,
    /// The audit row committed alongside.
    pub audit: AuditEvent,
    /// When it happened.
    pub now: OffsetDateTime,
}

/// The eight-field replay identity, as the adapter keys its record by.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IdempotencyRecordKey {
    /// The record's own id.
    pub id: Uuid,
    /// Which header carried the key.
    pub key_kind: aex_control_domain::IdempotencyKeyKind,
    /// The caller-supplied key.
    pub key_value: String,
    /// Which kind of principal.
    pub principal_kind: aex_control_domain::PrincipalKindTag,
    /// Which principal.
    pub principal_id: Uuid,
    /// Which kind of scope.
    pub scope_kind: aex_control_domain::ScopeKind,
    /// Which scope.
    pub scope_id: Uuid,
    /// The HTTP method.
    pub method: aex_wire::types::HttpMethod,
    /// The canonical route template.
    pub route: String,
    /// The canonical intent.
    pub intent_hash: IntentHash,
    /// When the record expires.
    pub expires_at: OffsetDateTime,
}

/// Invite somebody to an organization.
#[derive(Debug, Clone)]
pub struct CreateInvitationTx {
    /// The invitation id.
    pub preassigned_id: Uuid,
    /// Which organization.
    pub organization_id: Uuid,
    /// The normalized address.
    pub email: String,
    /// The offered role. Never `owner`.
    pub role: OrgRole,
    /// Who invited.
    pub invited_by_user_id: Uuid,
    /// When it lapses.
    pub expires_at: OffsetDateTime,
    /// The replay identity.
    pub idempotency: IdempotencyRecordKey,
    /// The outbox row committed alongside.
    pub outbox: OutboxMessage,
    /// The audit row committed alongside.
    pub audit: AuditEvent,
    /// When it happened.
    pub now: OffsetDateTime,
}

/// Redeem every pending invitation matching a verified address.
#[derive(Debug, Clone)]
pub struct AcceptInvitationsTx {
    /// Who is accepting.
    pub user_id: Uuid,
    /// Their normalized address.
    pub email: String,
    /// Whether their address is verified. Acceptance requires `true`.
    pub email_verified: bool,
    /// Membership ids to use, one per invitation, preassigned.
    pub preassigned_membership_ids: Vec<Uuid>,
    /// When it happened.
    pub now: OffsetDateTime,
}

/// Open the first half of workspace provisioning.
#[derive(Debug, Clone)]
pub struct BeginWorkspaceProvisionTx {
    /// The workspace id.
    pub preassigned_workspace_id: Uuid,
    /// The internal operation id.
    pub preassigned_operation_id: Uuid,
    /// Which organization.
    pub organization_id: Uuid,
    /// Display name.
    pub name: String,
    /// Slug, unique inside the organization.
    pub slug: Slug,
    /// Placement, immutable from here on.
    pub region: aex_wire::types::Region,
    /// Who created it.
    pub created_by_user_id: Uuid,
    /// The replay identity.
    pub idempotency: IdempotencyRecordKey,
    /// The outbox row committed alongside.
    pub outbox: OutboxMessage,
    /// The audit row committed alongside.
    pub audit: AuditEvent,
    /// When it happened.
    pub now: OffsetDateTime,
}

/// Close workspace provisioning once both halves are durable.
#[derive(Debug, Clone)]
pub struct FinishWorkspaceProvisionTx {
    /// Which workspace.
    pub workspace_id: Uuid,
    /// Which operation.
    pub operation_id: Uuid,
    /// The fence the regional half completed under.
    pub fence: aex_control_domain::Fence,
    /// The replay record to complete.
    pub idempotency_id: Uuid,
    /// The response body to record for a replay.
    pub response_body: serde_json::Value,
    /// The audit row committed alongside.
    pub audit: AuditEvent,
    /// When it happened.
    pub now: OffsetDateTime,
}

/// Accept a workspace deletion, closing admission immediately.
#[derive(Debug, Clone)]
pub struct BeginWorkspaceDeletionTx {
    /// Which workspace.
    pub workspace_id: Uuid,
    /// Which organization it belongs to.
    pub organization_id: Uuid,
    /// The public operation id, which is the caller's `Aex-Operation-Id`.
    pub operation_id: Uuid,
    /// The replay identity.
    pub idempotency: IdempotencyRecordKey,
    /// The outbox row committed alongside.
    pub outbox: OutboxMessage,
    /// The audit row committed alongside.
    pub audit: AuditEvent,
    /// When it happened.
    pub now: OffsetDateTime,
}

/// Close a workspace deletion once the regional half is gone.
#[derive(Debug, Clone)]
pub struct CompleteWorkspaceDeletionTx {
    /// Which workspace.
    pub workspace_id: Uuid,
    /// Which operation.
    pub operation_id: Uuid,
    /// The fence the regional half completed under.
    pub fence: aex_control_domain::Fence,
    /// The audit row committed alongside.
    pub audit: AuditEvent,
    /// When it happened.
    pub now: OffsetDateTime,
}

/// Mint a workspace API key.
#[derive(Debug, Clone)]
pub struct CreateApiKeyTx {
    /// The key id, which is also the credential lookup key.
    pub preassigned_id: Uuid,
    /// Which workspace.
    pub workspace_id: Uuid,
    /// Which organization.
    pub organization_id: Uuid,
    /// Display name.
    pub name: String,
    /// The scopes, already narrowed to the mintable ceiling.
    pub scopes: ScopeSet,
    /// The region, matching the workspace.
    pub region: aex_wire::types::Region,
    /// The keyed verifier.
    pub verifier: [u8; 32],
    /// Which pepper it was computed under.
    pub pepper_version: u16,
    /// Who minted it.
    pub created_by_user_id: Uuid,
    /// The regional key-authorization projection, committed with the key.
    ///
    /// A region cannot admit a request against a key it has never heard of, so
    /// announcing the key is part of minting it rather than a second write that
    /// can fail on its own.
    pub outbox: OutboxMessage,
    /// The replay identity.
    pub idempotency: IdempotencyRecordKey,
    /// The audit row committed alongside.
    pub audit: AuditEvent,
    /// When it happened.
    pub now: OffsetDateTime,
}

/// Revoke a workspace API key.
#[derive(Debug, Clone)]
pub struct RevokeApiKeyTx {
    /// Which key.
    pub key_id: Uuid,
    /// Which workspace it must belong to.
    pub workspace_id: Uuid,
    /// The `If-Match` revision the caller asserted, when they supplied one.
    pub expected_revision: Option<u64>,
    /// The regional revocation projection committed with the epoch advance.
    pub outbox: OutboxMessage,
    /// The audit row committed alongside.
    pub audit: AuditEvent,
    /// When it happened.
    pub now: OffsetDateTime,
}

/// A bounded keyset page request.
///
/// The continuation is a **decoded, already authenticated** `(created_at_ms,
/// id)` position rather than the opaque token the caller sent. Passing the raw
/// token would leave the store with two bad options: ignore it, which silently
/// repeats the first page, or decode it without the signing secret, which
/// accepts attacker-controlled ordering state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PageRequest {
    /// The `(created_at_ms, id)` the previous page ended at.
    pub after: Option<(i64, Uuid)>,
    /// How many rows, capped at [`MAX_PAGE_LIMIT`].
    pub limit: u32,
}

/// List an actor's organizations.
#[derive(Debug, Clone)]
pub struct ListOrganizations {
    /// Whose organizations.
    pub user_id: Uuid,
    /// Paging.
    pub page: PageRequest,
}

/// List an organization's workspaces.
#[derive(Debug, Clone)]
pub struct ListWorkspaces {
    /// Whose workspaces, by membership.
    pub user_id: Uuid,
    /// Restrict to one organization, when the caller asked for one.
    pub organization_id: Option<Uuid>,
    /// Paging.
    pub page: PageRequest,
}

/// List a workspace's API keys.
#[derive(Debug, Clone)]
pub struct ListApiKeys {
    /// Which workspace.
    pub workspace_id: Uuid,
    /// Paging.
    pub page: PageRequest,
}

/// List an organization's public operations.
#[derive(Debug, Clone)]
pub struct ListOperations {
    /// Which organization.
    pub organization_id: Uuid,
    /// Restrict to one public kind.
    pub kind: Option<String>,
    /// Restrict to one lifecycle state.
    pub status: Option<OperationStatus>,
    /// Paging.
    pub page: PageRequest,
}

/// An organization together with the requesting user's current role.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OrganizationView {
    /// The durable organization.
    pub organization: Organization,
    /// The requesting user's active role.
    pub caller_role: OrgRole,
}

/// A membership together with the identity-owned email projection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MembershipView {
    /// The durable membership.
    pub membership: Membership,
    /// The member's current normalized email.
    pub email: String,
}

/// The published account fact together with the revocation epoch beside it.
///
/// The profile is `aex_control_domain`'s, because it is the sole input to the
/// published operational state and both planes project it through the same
/// function. The epoch is a `control.authorization_epoch` row read in the same
/// statement: it is projected onto the placement item and never published, so it
/// travels beside the profile rather than inside it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccountProjection {
    /// What every route publishes.
    pub profile: AccountProfile,
    /// The monotone account revocation epoch read with the state.
    pub epoch: u64,
}

impl AccountProjection {
    /// The account state, without reaching through the profile.
    #[must_use]
    pub const fn state(&self) -> aex_control_domain::AccountState {
        self.profile.state
    }

    /// The finance revision the state was read at.
    #[must_use]
    pub const fn revision(&self) -> u64 {
        self.profile.revision
    }
}

/// A workspace together with the account state it inherits.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceView {
    /// The durable workspace.
    pub workspace: Workspace,
    /// The owning organization's current account projection.
    pub account: AccountProjection,
    /// The workspace revocation epoch projected with its lifecycle.
    pub workspace_epoch: u64,
}

/// A public operation together with facts needed for its typed result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OperationView {
    /// The durable operation.
    pub operation: Operation,
    /// The workspace tombstone instant, once deletion succeeded.
    pub workspace_deleted_at: Option<OffsetDateTime>,
}

/// Read projections used only by the public control boundary.
#[async_trait]
pub trait ControlViewStore: Send + Sync {
    /// Lists the caller's organizations with the role used to admit each row.
    async fn list_organization_views(
        &self,
        query: &ListOrganizations,
    ) -> Result<Page<OrganizationView>, StoreError>;

    /// Reads one organization only when the caller has an active membership.
    async fn get_organization_view(
        &self,
        organization_id: Uuid,
        user_id: Uuid,
    ) -> Result<Option<OrganizationView>, StoreError>;

    /// Lists memberships with identity-owned email addresses.
    async fn list_membership_views(
        &self,
        organization_id: Uuid,
        page: &PageRequest,
    ) -> Result<Page<MembershipView>, StoreError>;

    /// Lists visible workspaces with their inherited account state.
    async fn list_workspace_views(
        &self,
        query: &ListWorkspaces,
    ) -> Result<Page<WorkspaceView>, StoreError>;

    /// Reads one visible workspace and its inherited account state.
    async fn get_workspace_view(&self, id: Uuid) -> Result<Option<WorkspaceView>, StoreError>;

    /// Reads one public operation with result projection facts.
    async fn get_operation_view(&self, id: Uuid) -> Result<Option<OperationView>, StoreError>;

    /// Lists public operations with bound kind and status filters.
    async fn list_operation_views(
        &self,
        query: &ListOperations,
    ) -> Result<Page<OperationView>, StoreError>;

    /// Reads one user's current normalized email.
    async fn user_email(&self, user_id: Uuid) -> Result<Option<String>, StoreError>;

    /// Reads the full public account projection. Absence means unavailable.
    async fn account_profile(
        &self,
        organization_id: Uuid,
    ) -> Result<Option<AccountProjection>, StoreError>;

    /// Resolves the replay row attached to a durable operation. The direct
    /// operation-recovery lane needs this to close a provision even if its
    /// outbox message is unavailable.
    async fn idempotency_id_for_operation(
        &self,
        operation_id: Uuid,
    ) -> Result<Option<Uuid>, StoreError>;
}

/// Claim due operations for a worker.
#[derive(Debug, Clone)]
pub struct ClaimDueOperations {
    /// Which worker.
    pub owner: String,
    /// How long the lease lasts.
    pub lease: Duration,
    /// How many to claim.
    pub batch: u32,
    /// When the claim happened.
    pub now: OffsetDateTime,
}

/// Claim outbox rows for dispatch.
#[derive(Debug, Clone)]
pub struct ClaimOutbox {
    /// Which worker.
    pub owner: String,
    /// How long the lease lasts.
    pub lease: Duration,
    /// How many to claim.
    pub batch: u32,
    /// When the claim happened.
    pub now: OffsetDateTime,
}

/// Sweep expired replay records and dispatched outbox rows.
#[derive(Debug, Clone)]
pub struct GcExpired {
    /// The instant expiry is evaluated against.
    pub now: OffsetDateTime,
    /// How many rows per sweep.
    pub batch: u32,
}

/// What one sweep reclaimed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct GcReport {
    /// How many replay records were removed.
    pub idempotency_records: u64,
    /// How many dispatched outbox rows were removed.
    pub outbox_messages: u64,
}

/// The control authority.
#[async_trait]
pub trait ControlStore: Send + Sync {
    /// Creates an organization, its first owner and its finance account.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError`] for a transport, privilege or constraint failure.
    async fn create_organization(
        &self,
        command: &CreateOrganizationTx,
    ) -> Result<TxOutcome<Organization>, StoreError>;

    /// Lists an actor's organizations.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError`] for a transport or privilege failure.
    async fn list_organizations(
        &self,
        query: &ListOrganizations,
    ) -> Result<Page<Organization>, StoreError>;

    /// Reads one organization.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError`] for a transport or privilege failure.
    async fn get_organization(&self, id: Uuid) -> Result<Option<Organization>, StoreError>;

    /// Lists an organization's memberships.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError`] for a transport or privilege failure.
    async fn list_memberships(
        &self,
        organization_id: Uuid,
        page: &PageRequest,
    ) -> Result<Page<Membership>, StoreError>;

    /// Records an invitation and its notification request.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError`] for a transport, privilege or constraint failure.
    async fn create_invitation(
        &self,
        command: &CreateInvitationTx,
    ) -> Result<TxOutcome<Invitation>, StoreError>;

    /// Redeems every pending invitation matching a verified address.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError`] for a transport, privilege or constraint failure.
    async fn accept_invitations_for_email(
        &self,
        command: &AcceptInvitationsTx,
    ) -> Result<TxOutcome<Vec<Membership>>, StoreError>;

    /// Opens the first half of provisioning.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError`] for a transport, privilege or constraint failure.
    async fn begin_workspace_provision(
        &self,
        command: &BeginWorkspaceProvisionTx,
    ) -> Result<TxOutcome<(Workspace, Operation)>, StoreError>;

    /// Closes provisioning once the regional half is durable.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError`] for a transport, privilege or constraint failure.
    async fn finish_workspace_provision(
        &self,
        command: &FinishWorkspaceProvisionTx,
    ) -> Result<TxOutcome<Workspace>, StoreError>;

    /// Lists workspaces.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError`] for a transport or privilege failure.
    async fn list_workspaces(&self, query: &ListWorkspaces) -> Result<Page<Workspace>, StoreError>;

    /// Reads one workspace.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError`] for a transport or privilege failure.
    async fn get_workspace(&self, id: Uuid) -> Result<Option<Workspace>, StoreError>;

    /// Accepts a deletion, revoking every key for the workspace in the same
    /// transaction.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError`] for a transport, privilege or constraint failure.
    async fn begin_workspace_deletion(
        &self,
        command: &BeginWorkspaceDeletionTx,
    ) -> Result<TxOutcome<(Workspace, Operation)>, StoreError>;

    /// Closes a deletion once the regional half is gone.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError`] for a transport, privilege or constraint failure.
    async fn complete_workspace_deletion(
        &self,
        command: &CompleteWorkspaceDeletionTx,
    ) -> Result<TxOutcome<Workspace>, StoreError>;

    /// Mints a workspace API key.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError`] for a transport, privilege or constraint failure.
    async fn create_api_key(
        &self,
        command: &CreateApiKeyTx,
    ) -> Result<TxOutcome<ApiKey>, StoreError>;

    /// Reads one API key's metadata, never its verifier.
    ///
    /// The `HTTP` edge needs this before it can decide anything about a key: a
    /// key names a workspace and an organization, and both must come from the
    /// key's own row rather than from the path, or a caller who could name a
    /// foreign workspace could authorize itself against it.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError`] for a transport or privilege failure.
    async fn get_api_key(&self, id: Uuid) -> Result<Option<ApiKey>, StoreError>;

    /// Lists a workspace's API keys.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError`] for a transport or privilege failure.
    async fn list_api_keys(&self, query: &ListApiKeys) -> Result<Page<ApiKey>, StoreError>;

    /// Revokes a workspace API key, advancing its epoch in the same transaction.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError`] for a transport, privilege or constraint failure.
    async fn revoke_api_key(&self, command: &RevokeApiKeyTx) -> Result<TxOutcome<()>, StoreError>;

    /// Reads one operation.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError`] for a transport or privilege failure.
    async fn get_operation(&self, id: Uuid) -> Result<Option<Operation>, StoreError>;

    /// Lists an organization's public operations.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError`] for a transport or privilege failure.
    async fn list_operations(&self, query: &ListOperations) -> Result<Page<Operation>, StoreError>;

    /// Claims operations whose lease has lapsed or which have never run.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError`] for a transport or privilege failure.
    async fn claim_due_operations(
        &self,
        command: &ClaimDueOperations,
    ) -> Result<Vec<Operation>, StoreError>;

    /// Claims outbox rows for dispatch.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError`] for a transport or privilege failure.
    async fn claim_outbox(&self, command: &ClaimOutbox) -> Result<Vec<OutboxMessage>, StoreError>;

    /// Records one outbox message on its own, outside any aggregate's
    /// transaction.
    ///
    /// Every other outbox row this crate writes is committed *inside* the
    /// transaction that creates the aggregate it describes, which is what makes
    /// the outbox transactional at all. This method is the one exception, and
    /// it exists for exactly one caller: a notification whose aggregate is
    /// already durable and whose message id that transaction preassigned. It is
    /// therefore a retry of a message the database already agreed to, not a new
    /// intent, which is why a conflict on `(topic, dedupe_key)` is the promise
    /// already kept rather than a failure.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError`] for a transport, privilege or constraint failure.
    /// A duplicate is reported as [`StoreError::Conflict`] and left for the
    /// caller to interpret; this port does not decide that a conflict is
    /// success.
    async fn enqueue_outbox(&self, message: &OutboxMessage) -> Result<(), StoreError>;

    /// Marks an outbox row dispatched.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError`] for a transport or privilege failure.
    async fn mark_outbox_dispatched(&self, id: Uuid, now: OffsetDateTime)
    -> Result<(), StoreError>;

    /// Releases an outbox claim after a failure, backing off.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError`] for a transport or privilege failure.
    async fn release_outbox(
        &self,
        id: Uuid,
        available_at: OffsetDateTime,
        error: &str,
    ) -> Result<(), StoreError>;

    /// Sweeps expired replay records and dispatched outbox rows.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError`] for a transport or privilege failure.
    async fn gc_expired(&self, command: &GcExpired) -> Result<GcReport, StoreError>;

    /// The **current** account state of one organization, coarsely.
    ///
    /// This is the read the `HTTP` edge runs at precedence stage 6 for every
    /// route that is not pause-exempt. It is deliberately its own method rather
    /// than a field of some larger row: the edge has resolved an organization
    /// and nothing else, and a read that also returned a credential or a
    /// workspace would be answering a question it was not asked.
    ///
    /// An organization with no finance row resolves to
    /// [`aex_control_domain::AccountState::Unavailable`] and **never** to
    /// `Active`. `Active` is a claim that the account may spend; absence is a
    /// statement that nobody could establish whether it may, and the two are
    /// answered differently — `402` against `503` — for exactly that reason.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError`] for a transport or privilege failure. A failure
    /// is never softened into a state: an unreadable account rejects admission.
    async fn account_state(
        &self,
        organization_id: Uuid,
    ) -> Result<aex_control_domain::AccountState, StoreError>;
}

/// A workspace API key, as the authorization read sees it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceKeyState {
    /// Which key.
    pub key_id: Uuid,
    /// Which workspace.
    pub workspace_id: Uuid,
    /// Which organization.
    pub organization_id: Uuid,
    /// The scopes the row carries, before the mintable ceiling.
    pub scopes: ScopeSet,
    /// The stored keyed verifier.
    pub verifier: [u8; 32],
    /// Which pepper it was computed under.
    pub pepper_version: u16,
    /// Whether the key was revoked.
    pub key_revoked: bool,
    /// The workspace's region.
    pub region: aex_wire::types::Region,
    /// The workspace's status.
    pub workspace_status: aex_control_domain::WorkspaceStatus,
    /// The organization's status.
    pub organization_status: aex_control_domain::OrganizationStatus,
    /// The account state, `unavailable` when the finance row is absent.
    pub account_state: aex_control_domain::AccountState,
    /// The key's epoch.
    pub epoch_key: Epoch,
    /// The workspace's epoch.
    pub epoch_workspace: Epoch,
    /// The account's epoch.
    pub epoch_account: Epoch,
}

impl WorkspaceKeyState {
    /// The epoch subjects an assertion for this key must carry.
    #[must_use]
    pub fn epoch_subjects(&self) -> [(EpochSubjectKind, Uuid, Epoch); 3] {
        [
            (EpochSubjectKind::Key, self.key_id, self.epoch_key),
            (
                EpochSubjectKind::Workspace,
                self.workspace_id,
                self.epoch_workspace,
            ),
            (
                EpochSubjectKind::Account,
                self.organization_id,
                self.epoch_account,
            ),
        ]
    }
}

/// A person's credential, as the workspace-scoped authorization read sees it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccountActorState {
    /// Which credential.
    pub credential_id: Uuid,
    /// Which person.
    pub user_id: Uuid,
    /// Which membership.
    pub membership_id: Uuid,
    /// Their role.
    pub role: OrgRole,
    /// The scopes the credential carries.
    pub scopes: ScopeSet,
    /// The stored keyed verifier.
    pub verifier: [u8; 32],
    /// Which pepper it was computed under.
    pub pepper_version: u16,
    /// Whether the credential was revoked.
    pub credential_revoked: bool,
    /// Whether the credential lapsed.
    pub credential_expired: bool,
    /// Whether the person may authenticate.
    pub user_active: bool,
    /// Which workspace.
    pub workspace_id: Uuid,
    /// Which organization.
    pub organization_id: Uuid,
    /// The workspace's region.
    pub region: aex_wire::types::Region,
    /// The account state.
    pub account_state: aex_control_domain::AccountState,
    /// The person's epoch.
    pub epoch_user: Epoch,
    /// The membership's epoch.
    pub epoch_membership: Epoch,
    /// The workspace's epoch.
    pub epoch_workspace: Epoch,
    /// The account's epoch.
    pub epoch_account: Epoch,
}

/// A person's credential, as the central authorizer sees it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CentralActorState {
    /// Which credential.
    pub credential_id: Uuid,
    /// Which person.
    pub user_id: Uuid,
    /// The scopes the credential carries.
    pub scopes: ScopeSet,
    /// The stored keyed verifier.
    pub verifier: [u8; 32],
    /// Which pepper it was computed under.
    pub pepper_version: u16,
    /// Whether the credential was revoked.
    pub credential_revoked: bool,
    /// Whether the credential lapsed.
    pub credential_expired: bool,
    /// Whether the person may authenticate.
    pub user_active: bool,
    /// Every active membership.
    pub memberships: Vec<aex_control_domain::OrgMembership>,
}

/// The Ed25519 signing key a region will accept.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SigningKeyRecord {
    /// Which key.
    pub kid: Uuid,
    /// The public half.
    pub public_key: [u8; 32],
    /// Where the private half lives.
    pub secret_ref: String,
    /// Its lifecycle state.
    pub state: String,
    /// When a region stops accepting it.
    pub retires_at: OffsetDateTime,
}

/// The authentication material a regional key-authorization projection carries.
///
/// Deliberately narrower than [`WorkspaceKeyState`]: the projection publisher
/// needs the four facts a regional edge authenticates from and nothing about
/// workspace status, organization status, account state or epochs, which the
/// placement row already carries. A wider read here would be a second authority
/// for values the placement projection already owns.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceKeyMaterial {
    /// Which key the row belongs to.
    pub key_id: Uuid,
    /// The workspace it authorizes, so a publisher can cross-check the
    /// announcement it is acting on against the row it is replicating.
    pub workspace_id: Uuid,
    /// `HMAC-SHA256(pepper_v, SHA-256(token))`, exactly as stored.
    pub verifier: [u8; 32],
    /// Which pepper version the verifier was computed under.
    pub pepper_version: u16,
    /// The scopes the row carries, before the mintable ceiling.
    pub scopes: ScopeSet,
}

/// The read of a key's authentication material, for regional replication.
///
/// Separate from [`AuthorizationReader`] because the caller is different: this
/// one is the projection publisher, which reads a key once per authorization
/// event rather than once per request, and holds no credential-liveness or
/// account-state read at all.
#[async_trait]
pub trait KeyMaterialReader: Send + Sync {
    /// Reads one key's replicable authentication material.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError`] for a transport or privilege failure. `Ok(None)`
    /// means no such key, which a publisher must treat as "publish nothing"
    /// rather than "publish a row without a verifier".
    async fn workspace_key_material(
        &self,
        key_id: Uuid,
    ) -> Result<Option<WorkspaceKeyMaterial>, StoreError>;
}

/// The read-only authorization surface.
///
/// Every method performs zero writes, which the adapter proves by running it as
/// a role that holds no write privilege anywhere.
#[async_trait]
pub trait AuthorizationReader: Send + Sync {
    /// Resolves a workspace key in exactly one statement and no transaction.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError`] for a transport or privilege failure.
    async fn resolve_workspace_key(
        &self,
        key_id: Uuid,
    ) -> Result<Option<WorkspaceKeyState>, StoreError>;

    /// Resolves an account token scoped to one workspace.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError`] for a transport or privilege failure.
    async fn resolve_account_token_for_workspace(
        &self,
        token_id: Uuid,
        workspace_id: Uuid,
        now: OffsetDateTime,
    ) -> Result<Option<AccountActorState>, StoreError>;

    /// Resolves a browser session scoped to one workspace.
    ///
    /// This is what makes every regional dashboard panel implementable: a
    /// browser session resolves through the same 30-second assertion as an
    /// account token, not a second credential mechanism.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError`] for a transport or privilege failure.
    async fn resolve_session_for_workspace(
        &self,
        session_id: Uuid,
        workspace_id: Uuid,
        now: OffsetDateTime,
    ) -> Result<Option<AccountActorState>, StoreError>;

    /// Resolves an account token for the central plane.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError`] for a transport or privilege failure.
    async fn resolve_account_token_central(
        &self,
        token_id: Uuid,
        now: OffsetDateTime,
    ) -> Result<Option<CentralActorState>, StoreError>;

    /// Resolves a browser session for the central plane.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError`] for a transport or privilege failure.
    async fn resolve_dashboard_session_central(
        &self,
        session_id: Uuid,
        now: OffsetDateTime,
    ) -> Result<Option<CentralActorState>, StoreError>;

    /// The keys a region will accept right now.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError`] for a transport or privilege failure.
    async fn verification_key_set(&self) -> Result<Vec<SigningKeyRecord>, StoreError>;

    /// The one active signing key.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::NotFound`] when no key is active, which is a
    /// readiness failure rather than a reason to serve unsigned artifacts.
    async fn active_signing_key(&self) -> Result<SigningKeyRecord, StoreError>;
}

/// Ask a region to create or remove the regional half of a workspace.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProvisionWorkspaceRequest {
    /// Which workspace.
    pub workspace_id: Uuid,
    /// Which organization.
    pub organization_id: Uuid,
    /// Which region.
    pub region: aex_wire::types::Region,
    /// The fence this attempt runs under.
    pub fence: aex_control_domain::Fence,
    /// The canonical intent, so a replay is recognisable.
    pub intent_hash: IntentHash,
}

/// What a region answered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProvisionWorkspaceResponse {
    /// Which workspace the region created or already had.
    pub workspace_id: Uuid,
    /// Whether this call created it.
    pub created: bool,
}

/// Ask a region to remove the regional half of a workspace.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeleteWorkspaceRequest {
    /// Which workspace.
    pub workspace_id: Uuid,
    /// Which region.
    pub region: aex_wire::types::Region,
    /// The fence this attempt runs under.
    pub fence: aex_control_domain::Fence,
}

/// What a region answered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeleteWorkspaceResponse {
    /// Whether the regional half is gone.
    pub removed: bool,
}

/// The regional control authority.
#[async_trait]
pub trait RegionalControlPort: Send + Sync {
    /// Creates the regional half of a workspace, idempotently under its fence.
    ///
    /// # Errors
    ///
    /// Returns [`EffectError`]; `Unknown` means the region may have created it
    /// and the caller must reconcile rather than retry blindly.
    async fn provision_workspace(
        &self,
        request: &ProvisionWorkspaceRequest,
    ) -> Result<ProvisionWorkspaceResponse, EffectError>;

    /// Removes the regional half of a workspace, idempotently under its fence.
    ///
    /// # Errors
    ///
    /// Identical to [`RegionalControlPort::provision_workspace`].
    async fn delete_workspace(
        &self,
        request: &DeleteWorkspaceRequest,
    ) -> Result<DeleteWorkspaceResponse, EffectError>;
}

/// One invitation notification.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InvitationEmail {
    /// The outbox row this delivery is keyed by, so a retry is one message.
    pub message_id: Uuid,
    /// The recipient.
    pub to: String,
    /// The organization's display name.
    pub organization_name: String,
    /// The role offered.
    pub role: OrgRole,
}

/// The notification sender.
#[async_trait]
pub trait MailerPort: Send + Sync {
    /// Sends an invitation notification.
    ///
    /// # Errors
    ///
    /// Returns [`EffectError`]; the email is a notification and never a
    /// credential, so a lost one costs a resend and nothing else.
    async fn send_invitation(&self, mail: &InvitationEmail) -> Result<(), EffectError>;
}
