//! The ports a use case reads through.
//!
//! Every port is read-only except [`AuthorityCommitter`], and a use case never
//! calls that one: it returns a plan and the deployable commits it (D-03). The
//! clock and the id factory live here and only here (D-01), so every domain
//! function stays trivially deterministic.

use aex_content_domain::{ContentOutcome, ContentRoot, PageDigest, TreeNode, TreeView};
use aex_operation_domain::Operation;
use aex_secret_domain::{SecretName, SessionCustody, TrueIdle, WorkspaceSecret};
use aex_session_domain::{
    AccountProjection, AgentControl, EffectiveLimits, IdempotencyIdentity, IdempotencyReceipt,
    JournalPage, JournalSeq, ReservationId, Run, Session,
};
use aex_wire::ids::{
    AgentId, GenerationId, OperationId, OrganizationId, RunId, SessionId, UploadId, Uuid7,
    WorkspaceId,
};
use aex_wire::limits::LimitId;
use aex_wire::types::Timestamp;
use aex_workspace_domain::{RegistryPointer, RegistrySelector, Upload};

use crate::plan::{ConditionId, PlanError, SessionTransaction};

/// The only clock in the regional pure stack.
pub trait Clock: Send + Sync {
    /// The current instant.
    fn now(&self) -> Timestamp;
}

/// The only identifier source in the regional pure stack.
pub trait IdFactory: Send + Sync {
    /// A fresh time-ordered identifier payload.
    fn next_uuid_v7(&self) -> Uuid7;
}

/// How much one page may read.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PageBudget {
    /// Largest number of items to return.
    pub limit: u16,
}

/// One page of agents.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentPage {
    /// The agents, in canonical order.
    pub agents: Vec<AgentControl>,
    /// Whether more remain.
    pub more: bool,
}

/// Everything a command needs to know about a session in one read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionSnapshot {
    /// The session head.
    pub session: Session,
    /// Its root agent.
    pub root_agent: AgentControl,
    /// Its non-terminal agents, for the materialized ceiling.
    pub materialized: Vec<AgentControl>,
}

/// Why a port read failed.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PortError {
    /// The named record does not exist.
    #[error("{kind} not found")]
    NotFound {
        /// What was being read.
        kind: &'static str,
    },
    /// The provider refused the read for now.
    #[error("{kind} is temporarily unavailable")]
    Unavailable {
        /// What was being read.
        kind: &'static str,
    },
    /// The provider throttled the read.
    #[error("{kind} read was throttled")]
    Throttled {
        /// What was being read.
        kind: &'static str,
    },
    /// The stored record could not be decoded into a domain value.
    #[error("{kind} record is corrupt: {reason}")]
    Corrupt {
        /// What was being read.
        kind: &'static str,
        /// Why.
        reason: &'static str,
    },
}

/// Reads the session authority.
#[async_trait::async_trait]
pub trait SessionReader: Send + Sync {
    /// One session and the agents a command needs.
    async fn load_session(
        &self,
        workspace: WorkspaceId,
        session: SessionId,
    ) -> Result<SessionSnapshot, PortError>;

    /// One run record.
    async fn load_run(&self, session: SessionId, run: RunId) -> Result<Run, PortError>;

    /// One agent control record.
    async fn load_agent(
        &self,
        session: SessionId,
        agent: AgentId,
    ) -> Result<AgentControl, PortError>;

    /// A bounded page of agents.
    async fn list_agents(
        &self,
        session: SessionId,
        budget: PageBudget,
    ) -> Result<AgentPage, PortError>;

    /// A bounded page of one agent's journal.
    async fn load_journal_page(
        &self,
        session: SessionId,
        agent: AgentId,
        from: JournalSeq,
        budget: PageBudget,
    ) -> Result<JournalPage, PortError>;

    /// The receipt filed under an identity, when one exists.
    async fn load_receipt(
        &self,
        identity: &IdempotencyIdentity,
    ) -> Result<Option<IdempotencyReceipt>, PortError>;

    /// One durable operation, when it exists.
    async fn load_operation(
        &self,
        workspace: WorkspaceId,
        operation: OperationId,
    ) -> Result<Option<Operation>, PortError>;
}

/// Reads the named registry and uploads.
#[async_trait::async_trait]
pub trait RegistryReader: Send + Sync {
    /// Several registry pointers in one read.
    async fn read_many(
        &self,
        workspace: WorkspaceId,
        selectors: &[RegistrySelector],
    ) -> Result<Vec<RegistryPointer>, PortError>;

    /// One staged upload.
    async fn read_upload(
        &self,
        workspace: WorkspaceId,
        upload: UploadId,
    ) -> Result<Upload, PortError>;
}

/// Reads content descriptors and Merkle pages.
#[async_trait::async_trait]
pub trait ContentReader: Send + Sync {
    /// Whether a body is present and usable.
    async fn describe(
        &self,
        workspace: WorkspaceId,
        digest: aex_content_domain::ContentDigest,
    ) -> Result<ContentOutcome, PortError>;

    /// One Merkle page.
    async fn load_page(
        &self,
        workspace: WorkspaceId,
        page: PageDigest,
    ) -> Result<TreeNode, PortError>;
}

/// Reads secret custody.
#[async_trait::async_trait]
pub trait SecretCustodyReader: Send + Sync {
    /// Several workspace secrets in one read.
    async fn read_secrets(
        &self,
        workspace: WorkspaceId,
        names: &[SecretName],
    ) -> Result<Vec<WorkspaceSecret>, PortError>;

    /// One session's custody, when it has any.
    async fn read_custody(&self, session: SessionId) -> Result<Option<SessionCustody>, PortError>;
}

/// Reads the workspace's effective limits.
#[async_trait::async_trait]
pub trait LimitsReader: Send + Sync {
    /// The named limits, resolved for the workspace.
    async fn effective(
        &self,
        workspace: WorkspaceId,
        ids: &[LimitId],
    ) -> Result<EffectiveLimits, PortError>;
}

/// Reads the organization's account projection.
#[async_trait::async_trait]
pub trait AccountStateReader: Send + Sync {
    /// The projection in force.
    async fn projection(
        &self,
        organization: OrganizationId,
    ) -> Result<AccountProjection, PortError>;
}

/// What a run asks the finance plane to reserve.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReservationRequest {
    /// Which organization pays.
    pub organization: OrganizationId,
    /// Which workspace spends.
    pub workspace: WorkspaceId,
    /// The ceiling, in cents.
    pub max_spend_cents: u64,
}

/// A granted reservation.
///
/// The domain models only "a finite reservation exists and is bound to this run
/// or operation"; the grant and release protocol belongs to the finance and
/// usage streams.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReservationGrant {
    /// The reservation's identity.
    pub reservation: ReservationId,
    /// What it actually reserved.
    pub granted_cents: u64,
}

/// Prepares a spend reservation.
#[async_trait::async_trait]
pub trait ReservationAuthority: Send + Sync {
    /// Reserves spend for a run.
    async fn prepare(&self, request: ReservationRequest) -> Result<ReservationGrant, PortError>;
}

/// What `aex-runtime-control` says about a session's workspace generation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WorkspaceContinuity {
    /// The generation in force, when one is.
    pub generation: Option<GenerationId>,
    /// Whether the session's continuity is intact.
    pub intact: bool,
}

/// Reads runtime continuity and idleness.
///
/// `WorkspaceContinuity`, the idle predicate and the exact idle window are
/// `aex-runtime-control`'s; this port consumes its verdict so the two cannot
/// each compute a different answer.
#[async_trait::async_trait]
pub trait ContinuityReader: Send + Sync {
    /// The session's continuity.
    async fn continuity(&self, session: SessionId) -> Result<WorkspaceContinuity, PortError>;

    /// The session's idle evidence.
    async fn true_idle(&self, session: SessionId) -> Result<TrueIdle, PortError>;
}

/// Scans the live workspace of a running generation.
#[async_trait::async_trait]
pub trait LiveWorkspaceReader: Send + Sync {
    /// The live tree of one generation.
    async fn scan(
        &self,
        session: SessionId,
        generation: GenerationId,
    ) -> Result<TreeView, PortError>;

    /// The live root of one generation.
    async fn root(
        &self,
        session: SessionId,
        generation: GenerationId,
    ) -> Result<ContentRoot, PortError>;
}

/// Why a commit failed.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CommitError {
    /// One or more conditions did not hold.
    #[error("{} condition(s) failed", failed.len())]
    ConditionFailed {
        /// Exactly which ones, so the application maps a typed customer error.
        failed: Vec<ConditionId>,
    },
    /// The provider throttled the commit.
    #[error("commit was throttled")]
    Throttled,
    /// The provider was unavailable.
    #[error("commit provider is unavailable")]
    Unavailable,
    /// The plan itself was not submittable. This is an internal fault, never a
    /// customer error.
    #[error(transparent)]
    PlanRejected(#[from] PlanError),
}

/// What a successful commit reports.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CommitOutcome {
    /// When the provider accepted it.
    pub committed_at: Timestamp,
}

/// Submits one plan as one provider transaction.
///
/// Implementations must not reorder, split, weaken or drop a condition, and must
/// emit every hint strictly after the commit succeeds.
#[async_trait::async_trait]
pub trait AuthorityCommitter: Send + Sync {
    /// Commits the plan.
    async fn commit(&self, plan: &SessionTransaction) -> Result<CommitOutcome, CommitError>;
}

/// Everything a use case reads through.
///
/// The committer is deliberately **absent**: a use case cannot commit even by
/// accident, because it has no reference to anything that could.
pub struct AppContext<'a> {
    /// The clock.
    pub clock: &'a dyn Clock,
    /// The identifier source.
    pub ids: &'a dyn IdFactory,
    /// The session authority.
    pub sessions: &'a dyn SessionReader,
    /// The named registry.
    pub registry: &'a dyn RegistryReader,
    /// Content.
    pub content: &'a dyn ContentReader,
    /// Secret custody.
    pub secrets: &'a dyn SecretCustodyReader,
    /// Effective limits.
    pub limits: &'a dyn LimitsReader,
    /// The account projection.
    pub accounts: &'a dyn AccountStateReader,
    /// The reservation authority.
    pub reservations: &'a dyn ReservationAuthority,
    /// Runtime continuity.
    pub continuity: &'a dyn ContinuityReader,
    /// The live workspace.
    pub live: &'a dyn LiveWorkspaceReader,
}
