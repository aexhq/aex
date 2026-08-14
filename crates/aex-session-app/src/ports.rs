//! The ports a use case reads through.
//!
//! Every port is read-only except [`AuthorityCommitter`], and a use case never
//! calls that one: it returns a plan and the deployable commits it (D-03). The
//! clock and the id factory live here and only here (D-01), so every domain
//! function stays trivially deterministic.

use std::collections::BTreeSet;

use aex_internal_contracts::RunId;
use aex_operation_domain::Operation;
use aex_operation_domain::operation::OperationVersion;
use aex_session_domain::{
    AccountProjection, AgentControl, AgentRevision, EffectiveLimits, IdempotencyIdentity,
    IdempotencyReceipt, JournalPage, JournalSeq, Run, Session,
};
use aex_wire::ids::{
    AgentId, GenerationId, OperationId, OrganizationId, SessionId, UploadId, Uuid7, WorkspaceId,
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

/// Everything a session-wide cancellation needs to know about one agent.
///
/// Narrower than [`AgentControl`] on purpose. The physical `agent_control` row
/// is owned by `aex-brain-store-dynamodb` and carries that crate's `AgentHead`
/// schema; `AgentControl` is a third vocabulary that no adapter persists. These
/// three facts are the ones a real row can answer without inventing anything,
/// and they are exactly what [`crate::plan::Write::CancelAgent`] conditions on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct AgentCancelTarget {
    /// Which agent.
    pub agent: AgentId,
    /// Its optimistic revision.
    pub revision: aex_session_domain::AgentRevision,
    /// Whether it still admits work, i.e. it has not finished.
    pub active: bool,
}

/// One bounded page of cancellation targets, in canonical agent order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentCancelPage {
    /// The targets examined by this page, ordered.
    pub targets: Vec<AgentCancelTarget>,
    /// The first agent the next page starts at, when one remains.
    ///
    /// Carried rather than derived: a page whose last row is terminal still has
    /// to advance the cursor, and deriving "next" from the last *cancelled*
    /// agent would loop forever on a page of already-terminal agents.
    pub next: Option<AgentId>,
}

/// One stored operation together with the store's optimistic version.
///
/// The version travels with the record because a **step commit** has to name
/// the version it observed: the already-served public cancellation writes the
/// same row under its own optimistic loop, so a step that could not name a
/// version would have to guess one. Returning them separately would let a
/// caller pair a record with a version it did not read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VersionedOperation {
    /// The durable record.
    pub operation: Operation,
    /// The version the read observed.
    pub version: OperationVersion,
}

/// The Brain-owned root facts needed to admit one new session message.
///
/// Deliberately narrower than [`AgentControl`]: the physical control row belongs
/// to the Brain store and does not persist that third vocabulary. Message
/// admission needs only the exact revision/tail and whether the root is at
/// rest; the transaction rechecks all three before it writes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RootAdmissionState {
    /// Root agent identity.
    pub agent: AgentId,
    /// Optimistic control revision.
    pub revision: AgentRevision,
    /// Immutable journal tail.
    pub journal_tail: JournalSeq,
    /// Effective-limit revision pinned by the root `AgentStarted` record.
    pub limits_revision: u64,
    /// Maximum duration of one message under that revision.
    pub max_run_duration_ms: u64,
    /// Whether the stored phase and lease facts admit a new message.
    pub idle: bool,
}

/// The exact session and Brain-root projection used by message admission.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MessageAdmissionSnapshot {
    /// The session head.
    pub session: Session,
    /// Its Brain-owned root admission facts.
    pub root: RootAdmissionState,
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
    /// The named session crossed its irreversible deletion fence.
    #[error("{kind} was deleted")]
    Deleted {
        /// What was deleted.
        kind: &'static str,
    },
    /// The named session has crossed the irreversible fence but its cascade is incomplete.
    #[error("{kind} is being deleted by operation {operation}")]
    Deleting {
        /// What is being deleted.
        kind: &'static str,
        /// Exact operation that owns the irreversible fence.
        operation: OperationId,
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
    /// The replay key already committed a different canonical create intent.
    #[error("idempotency key was reused for a different intent")]
    IdempotencyConflict,
    /// The stored record could not be decoded into a domain value.
    #[error("{kind} record is corrupt: {reason}")]
    Corrupt {
        /// What was being read.
        kind: &'static str,
        /// Why.
        reason: &'static str,
    },
    /// No adapter in the tree can answer this read faithfully.
    ///
    /// Distinct from every other arm because it is a **composition** gap, not a
    /// runtime condition: a retry cannot help, and the operator has to be told
    /// which seam is open rather than shown a corrupt-row diagnostic for a row
    /// that is not corrupt. A deployable must never mount a route that can
    /// reach this (RS-18); it exists so that wiring one by mistake fails loudly
    /// instead of returning invented data.
    #[error("{kind} cannot be read: {seam}")]
    Unowned {
        /// What was being read.
        kind: &'static str,
        /// Which seam owns it.
        seam: &'static str,
    },
}

/// Reads the session authority.
#[async_trait::async_trait]
pub trait SessionReader: Send + Sync {
    /// One session head.
    ///
    /// Split from [`SessionReader::load_message_snapshot`] on purpose: lifecycle
    /// commands need the head and nothing else, and bundling Brain state into
    /// every head read would make them depend on an unrelated decode.
    async fn load_session(
        &self,
        workspace: WorkspaceId,
        session: SessionId,
    ) -> Result<Session, PortError>;

    /// One session together with its narrow Brain-root admission projection.
    async fn load_message_snapshot(
        &self,
        workspace: WorkspaceId,
        session: SessionId,
    ) -> Result<MessageAdmissionSnapshot, PortError>;

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

    /// A bounded, resumable page of session-wide cancellation targets.
    ///
    /// `from` is inclusive, so a step that fails its cursor guard re-reads
    /// exactly the batch it was going to write and cannot skip an agent.
    async fn list_agent_cancel_targets(
        &self,
        session: SessionId,
        from: Option<AgentId>,
        budget: PageBudget,
    ) -> Result<AgentCancelPage, PortError>;

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
        workspace: WorkspaceId,
        scope: &str,
        identity: &IdempotencyIdentity,
        now: Timestamp,
    ) -> Result<Option<IdempotencyReceipt>, PortError>;

    /// One durable operation and the version it is stored at, when it exists.
    async fn load_operation(
        &self,
        workspace: WorkspaceId,
        operation: OperationId,
    ) -> Result<Option<VersionedOperation>, PortError>;
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

/// Whether a provider-credential binding may still be selected.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum CredentialState {
    /// Selectable.
    Ready,
    /// Revoked; a session holding it fails closed at first send.
    Revoked,
}

/// The facts a session admission decides on about one provider credential.
///
/// Deliberately narrower than the stored binding, which also carries a label, a
/// fingerprint and two lifecycle instants. Those are `provider_credential_get`'s
/// business; an admission that could read them would be able to decide on them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderCredentialBinding {
    /// Its identity.
    pub credential: aex_wire::ids::ProviderCredentialId,
    /// Which provider it is for.
    pub provider: aex_wire::provider::ProviderId,
    /// The secret generation bound at registration.
    pub source_generation: u64,
    /// The binding's monotone concurrency token.
    pub revision: u64,
    /// Whether it may still be selected.
    pub state: CredentialState,
}

/// Reads dedicated BYOK provider credentials.
#[async_trait::async_trait]
pub trait ProviderCredentialReader: Send + Sync {
    /// Encrypts a write-only session API key into regional custody and returns
    /// only the non-secret binding facts the durable session may retain.
    async fn bind_session_api_key(
        &self,
        workspace: WorkspaceId,
        organization: OrganizationId,
        credential: aex_wire::ids::ProviderCredentialId,
        provider: aex_wire::provider::ProviderId,
        api_key: &str,
        identity: &IdempotencyIdentity,
        now: Timestamp,
    ) -> Result<ProviderCredentialBinding, PortError>;

    /// Seals the complete write-only MCP definitions under a distinct
    /// session-scoped custody purpose. Implementations must not reuse the
    /// provider-key receipt or secret name.
    async fn bind_session_mcp_config(
        &self,
        _workspace: WorkspaceId,
        _organization: OrganizationId,
        _session: SessionId,
        _servers: &[aex_wire::models::McpServer],
        _identity: &IdempotencyIdentity,
        _now: Timestamp,
    ) -> Result<(), PortError> {
        Err(PortError::Unowned {
            kind: "session MCP config",
            seam: "session.mcp_config custody",
        })
    }

    /// One provider-credential binding, when the workspace has it.
    ///
    /// `Ok(None)` is "this workspace has no such binding" and becomes
    /// `provider_credential_not_found`; a revoked binding is found and refused
    /// separately, because "you never registered this" and "you revoked this"
    /// are different things for a caller to fix.
    async fn read_provider_credential(
        &self,
        workspace: WorkspaceId,
        credential: aex_wire::ids::ProviderCredentialId,
    ) -> Result<Option<ProviderCredentialBinding>, PortError>;
}

/// One revision-bound read of every effective limit a workspace has.
///
/// The revision travels with the values because a session pins it into its
/// immutable generation definition: a generation is defined by, among other
/// things, the limits policy in force when it was allocated, so reading the
/// values without the revision they came from would leave the pin naming a
/// policy nothing observed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LimitsBundle {
    /// The capacity authority's revision for the whole projection.
    pub revision: u64,
    /// Every scalar limit resolved for the workspace.
    ///
    /// Map-shaped limits are absent by construction: `EffectiveLimits` holds
    /// `u64` values only, and [`EffectiveLimits::require`] answers
    /// `LimitUnresolved` for anything it does not hold, which is the loud
    /// outcome rather than a silent zero.
    pub limits: EffectiveLimits,
    /// Revisioned Brain execution ceilings for every message in the session.
    pub agent_execution: AgentExecutionLimits,
    /// Revisioned per-message budget ceilings pinned into the root authority.
    pub run_budget: RunBudgetLimits,
}

/// The `session.agent_execution` effective map.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AgentExecutionLimits {
    /// One turn's wall-clock fence.
    pub turn_deadline_ms: u32,
    /// Deepest admitted child lineage, root at zero.
    pub max_depth: u16,
    /// Largest one-decision fanout.
    pub max_fanout: u32,
}

/// The `session.run_budget` effective map.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RunBudgetLimits {
    /// Longest one message may remain active.
    pub max_run_duration_ms: u64,
    /// Children ever created below the root.
    pub total_children_created: u64,
    /// Provider calls admitted for one message.
    pub provider_calls: u64,
    /// Hands calls admitted for one message.
    pub hands_calls: u64,
    /// Children durably queued at once.
    pub queued_children: u64,
    /// Retained child-result bytes.
    pub retained_result_bytes: u64,
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

    /// Every effective limit plus the revision they were all read at.
    ///
    /// One read rather than "read the values, then read the revision": two
    /// reads could straddle a capacity change and pin a revision that never
    /// produced the values beside it.
    async fn bundle(&self, workspace: WorkspaceId) -> Result<LimitsBundle, PortError>;
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

/// What kind of thing a live path names.
///
/// The guest's own `lstat` vocabulary, carried through unchanged. A symlink is
/// reported, never followed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum LiveEntryKind {
    /// A regular file.
    File,
    /// A directory.
    Directory,
    /// A symbolic link.
    Symlink,
    /// A socket, a fifo, a device.
    Other,
}

/// One entry in a live workspace, exactly as the running `MicroVM` reports it.
///
/// **There is no digest here, and that is the point.** A live listing is one
/// `lstat` per entry: mode, mtime, size, and for a symlink its target.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct LiveEntry {
    /// The absolute path inside the guest root.
    pub path: String,
    /// What kind of thing it is.
    pub kind: LiveEntryKind,
    /// Size in bytes.
    pub size_bytes: u64,
    /// POSIX mode bits.
    pub mode: u32,
    /// Modification time.
    pub mtime: Timestamp,
    /// The link target, when the entry is a symlink.
    pub target: Option<String>,
}

/// What a live listing asks for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LiveListQuery {
    /// The subtree to list. Absent means the guest root.
    pub path: Option<String>,
    /// Whether to descend.
    pub recursive: bool,
    /// How many entries at most.
    pub limit: u16,
    /// Resume strictly after this path.
    pub after: Option<String>,
}

/// One page of live entries.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LiveListing {
    /// The entries, ascending by path.
    pub entries: Vec<LiveEntry>,
    /// Where the next page resumes, when one remains.
    ///
    /// Carried rather than derived from the last entry: "this page is full" and
    /// "there is more" are different facts, and only the reader knows the
    /// second one.
    pub next_after: Option<String>,
}

/// Reads the live workspace of a running generation.
///
/// These are same-generation observations. They never hash or persist the guest filesystem,
/// and a missing generation is returned as missing rather than replaced from a snapshot.
#[async_trait::async_trait]
pub trait LiveWorkspaceReader: Send + Sync {
    /// One page of a live directory listing. No file content is read.
    async fn list(
        &self,
        session: SessionId,
        generation: GenerationId,
        query: &LiveListQuery,
    ) -> Result<LiveListing, PortError>;

    /// One live entry, without following a symlink. No file content is read.
    async fn stat(
        &self,
        session: SessionId,
        generation: GenerationId,
        path: &str,
    ) -> Result<LiveEntry, PortError>;
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
    /// The provider's answer did not say whether the transaction landed.
    ///
    /// Rows 6, 7 and 8 of the unknown-outcome matrix (D-10) are exactly this
    /// case, and without this arm they are *inexpressible*: a timeout, a reset
    /// or a mixed `TransactionCanceled` would otherwise have to be reported as
    /// a definite failure, which is the one thing it is not. `targets` names
    /// every item the transaction would have written, so the resolver reads
    /// them and never guesses.
    #[error("commit outcome is unknown across {} target(s)", targets.len())]
    Ambiguous {
        /// Every item the plan would have written, in plan order.
        targets: Vec<crate::plan::ItemKey>,
    },
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

/// A provider and model pair the compiled catalog admitted, and the release
/// that admitted it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QualifiedModel {
    /// The qualified provider.
    pub provider: aex_wire::provider::ProviderId,
    /// The qualified provider-native model id.
    pub model: String,
    /// The `mc1_<hex>` rendering of the catalog release, pinned into the
    /// session's resolved-configuration document so the session records exactly
    /// which catalog admitted it.
    pub catalog_revision: String,
}

/// Why a catalog refused a pair.
///
/// Two arms because the compiled table admits every row it carries, so the
/// only refusals are the unknown provider and the unknown model
/// (model-provider simplification 2026-08-13).
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum QualificationRefusal {
    /// The catalog has no such provider.
    #[error("the catalog has no such provider")]
    UnknownProvider,
    /// The catalog has the provider but not the model.
    #[error("the catalog has no such model for this provider")]
    UnknownModel,
}

impl QualificationRefusal {
    /// The stable public code.
    #[must_use]
    pub const fn code(self) -> aex_wire::error::ErrorCode {
        match self {
            Self::UnknownProvider => aex_wire::error::ErrorCode::UnknownProvider,
            Self::UnknownModel => aex_wire::error::ErrorCode::UnknownModel,
        }
    }
}

/// Qualifies a provider and model pair against the compiled model catalog.
///
/// **Synchronous, and that is the point** (A D-11). The catalog is the
/// compiled models.dev admit table, so an `async fn` would add a `.await` and
/// a failure mode to what is a function call, and a per-request read would put
/// a network dependency on the admission path.
pub trait ModelQualifier: Send + Sync {
    /// Resolves a pair for a **new** session: every catalog gate applies.
    ///
    /// # Errors
    ///
    /// Returns [`QualificationRefusal`] for an unknown provider or an unknown
    /// model.
    fn admit(
        &self,
        provider: aex_wire::provider::ProviderId,
        model: &str,
    ) -> Result<QualifiedModel, QualificationRefusal>;
}

/// The deployment facts a session create decides `invalid_network_policy` and
/// `unsupported_package_ecosystem` from (A D-12).
///
/// Values, not ports. Both are properties of the plane the process is running
/// on, asserted once at start-up, and a per-request read of either would be a
/// network call whose failure mode is a silent default. A process whose
/// configuration cannot answer these **fails to start**.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeploymentFacts {
    /// Whether this plane has a managed `INTERNET_EGRESS` connector at all.
    ///
    /// The only way a network policy is *invalid* is that the caller asked for
    /// egress a deployment cannot give. Accepting the request and quietly
    /// giving the guest no egress is the silent fallback the standing policy
    /// forbids outright.
    pub public_internet_egress: bool,
    /// The validated closed image catalog every generation is pinned from.
    pub images: aex_runtime_control::catalog::HandsImageCatalog,
    /// The package ecosystems every published image variant carries.
    ///
    /// A set rather than a per-variant capability because the check a create
    /// runs is "can the image this session pinned pre-install this?", and the
    /// answer is only honest if it was asserted against the published images at
    /// boot.
    pub package_ecosystems: BTreeSet<aex_wire::models::PackageEcosystem>,
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
    /// Dedicated BYOK provider credentials.
    pub credentials: &'a dyn ProviderCredentialReader,
    /// The signed model catalog, behind a synchronous seam. A command-only
    /// composition may omit it; session creation refuses that composition
    /// before reading any request-selected authority.
    pub catalog: Option<&'a dyn ModelQualifier>,
    /// The plane's own deployment facts. As with the catalog, absence is an
    /// explicit unowned create seam rather than a fabricated default.
    pub deployment: Option<&'a DeploymentFacts>,
    /// Effective limits.
    pub limits: &'a dyn LimitsReader,
    /// The account projection.
    pub accounts: &'a dyn AccountStateReader,
    /// The live workspace.
    pub live: &'a dyn LiveWorkspaceReader,
}
