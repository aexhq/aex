//! The declarative transaction plan.
//!
//! This is the stream's load-bearing cross-stream artifact. A use case returns a
//! [`SessionTransaction`] and **never commits**; the deployable submits it. That
//! is what makes "one command = one transaction, no external call inside it" a
//! unit-testable property rather than a review convention.
//!
//! The [`Condition`], [`Write`] and [`Hint`] vocabularies are **closed** (D-04).
//! An adapter maps each condition onto exactly one provider condition expression
//! and may not drop, weaken, merge or reorder one; each write carries its own
//! table family and the adapter routes by that alone; a hint is emitted strictly
//! after a successful commit and never influences it.

use std::collections::BTreeSet;

use aex_content_domain::{ContentDigest, GrantId, Pin, RegistryKind};
use aex_internal_contracts::RunId;
use aex_operation_domain::{DeletionEpoch, DeletionState, Fence, Operation};
use aex_secret_domain::{
    CustodyRevision, OwnerKeyEdgeId, RevocationEpoch, SecretName, SessionCustody, WorkspaceSecret,
};
use aex_session_domain::{
    AccountRevision, AgentControl, AgentFence, AgentRevision, Approval, AuthorizationEpoch,
    CancellationEpoch, IdempotencyReceipt, JournalPage, JournalSeq, Message, OutboxEvent, Run,
    Session, SessionRevision, SessionTombstone, WorkAdmission,
};
use aex_wire::ids::{
    AgentId, ObservationId, OperationId, OrganizationId, ProviderCredentialId, SessionId, UploadId,
    WorkspaceId,
};
use aex_wire::provider::ProviderId;
use aex_wire::types::{ETag, Timestamp};
use aex_workspace_domain::{DownloadGrant, RegistryPointer, RegistrySelector, Upload, UploadState};

/// Largest number of actions one transaction may carry.
pub const MAX_ACTIONS: usize = 100;

/// Largest byte size one transaction may carry.
pub const MAX_BYTES: usize = 4 * 1024 * 1024;

/// Which command produced a plan. Carried so an adapter can attribute a
/// provider failure without parsing the writes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum TransactionIntent {
    /// Create a session.
    CreateSession,
    /// Admit a message and its run.
    AdmitMessage,
    /// Cancel the session's current work.
    CancelSession,
    /// Suspend the exact retained generation.
    SuspendSession,
    /// Resume the exact retained generation.
    ResumeSession,
    /// Permanently terminate compute and live files.
    TerminateSession,
    /// Irreversibly delete session user content.
    DeleteSession,
    /// Atomically publish a lifecycle effect, operation result and work retirement.
    SettleLifecycle,
    /// Start an admitted run.
    StartRun,
    /// Settle a run.
    CommitTerminal,
    /// Stop a session's work.
    StopSession,
    /// Clone a session.
    CloneSession,
    /// Discard the live workspace.
    DiscardWorkspace,
    /// Rebind credentials.
    RebindCredentials,
    /// Trash a session.
    TrashSession,
    /// Restore a session.
    RestoreSession,
    /// Purge a session.
    PurgeSession,
    /// Decide an approval.
    RespondApproval,
    /// Advance a continued operation.
    ContinueOperation,
    /// Mint a registry-file download grant and its replay receipt.
    RegistryDownload,
}

/// The identity of one item a write targets.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ItemKey {
    /// The table family the item lives in.
    pub family: TableFamily,
    /// The partition.
    pub partition: String,
    /// The sort position.
    pub sort: String,
}

/// Which regional table family a write belongs to.
///
/// The adapter routes by this alone, so adding a table never means teaching the
/// domain about it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum TableFamily {
    /// Read-only workspace/account admission projection.
    AuthorizationProjection,
    /// The session authority.
    SessionAuthority,
    /// Public asynchronous operation records stored with the session authority.
    OperationAuthority,
    /// Runnable regional work records.
    WorkAuthority,
    /// Content descriptors, pins and owner edges.
    ContentAuthority,
    /// The named registry and uploads.
    Registry,
    /// Session credential custody.
    SecretCustody,
    /// Idempotency receipts.
    Idempotency,
    /// The native outbox.
    Outbox,
}

/// The stable identity of one condition inside a plan.
///
/// `CommitError::ConditionFailed` returns these, so the application maps a
/// provider rejection onto a typed customer error rather than guessing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ConditionId(pub u16);

/// One precondition the transaction commits under.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Condition {
    /// The session head is at this revision.
    SessionRevision {
        /// Which session.
        session: SessionId,
        /// The expected revision.
        expected: SessionRevision,
    },
    /// The session is in one of these statuses.
    SessionStatusIn {
        /// Which session.
        session: SessionId,
        /// The allowed set.
        allowed: BTreeSet<aex_session_domain::SessionStatus>,
    },
    /// The session's active run is exactly this.
    SessionActiveRun {
        /// Which session.
        session: SessionId,
        /// The expected run.
        expected: Option<RunId>,
    },
    /// The session admits work in exactly this way.
    WorkAdmission {
        /// Which session.
        session: SessionId,
        /// The expected admission.
        expected: WorkAdmission,
    },
    /// The session's deletion state and epoch are exactly these.
    DeletionState {
        /// Which session.
        session: SessionId,
        /// The expected state.
        expected: DeletionState,
        /// The expected epoch.
        epoch: DeletionEpoch,
    },
    /// The session's cancellation epoch is exactly this.
    CancellationEpoch {
        /// Which session.
        session: SessionId,
        /// The expected epoch.
        expected: CancellationEpoch,
    },
    /// No whole-session command holds the exclusion.
    MutationGuardFree {
        /// Which session.
        session: SessionId,
    },
    /// This operation holds the exclusion.
    MutationGuardHeldBy {
        /// Which session.
        session: SessionId,
        /// The expected holder.
        holder: OperationId,
    },
    /// The agent control record is at this revision.
    AgentRevision {
        /// The owning session, required to locate the physical agent partition.
        session: SessionId,
        /// Which agent.
        agent: AgentId,
        /// The expected revision.
        expected: AgentRevision,
    },
    /// The agent's fence is at least this.
    AgentFence {
        /// The owning session, required to locate the physical agent partition.
        session: SessionId,
        /// Which agent.
        agent: AgentId,
        /// The floor.
        at_least: AgentFence,
    },
    /// The agent's journal tail is exactly this.
    JournalTail {
        /// The owning session, required to locate the physical agent partition.
        session: SessionId,
        /// Which agent.
        agent: AgentId,
        /// The expected tail.
        expected: JournalSeq,
    },
    /// The root control row is at rest and carries no owner or terminal.
    RootAgentIdle {
        /// Owning session.
        session: SessionId,
        /// Root agent.
        agent: AgentId,
    },
    /// The agent's journal tail has this exact content identity.
    JournalTailHash {
        /// The owning session, required to locate the physical agent partition.
        session: SessionId,
        /// Which agent.
        agent: AgentId,
        /// Exact BLAKE3 identity stored beside `journalTail`.
        expected: [u8; 32],
    },
    /// The run has not settled.
    RunNonTerminal {
        /// The owning session, required to locate the physical run row.
        session: SessionId,
        /// Which run.
        run: RunId,
    },
    /// The account projection has reached at least this revision.
    AccountRevisionAtLeast {
        /// Workspace placement row carrying the account epoch.
        workspace: WorkspaceId,
        /// Which organization.
        organization: OrganizationId,
        /// The floor.
        at_least: AccountRevision,
    },
    /// The exact dedicated BYOK binding remains ready at commit time.
    ProviderCredentialReady {
        /// Owning workspace.
        workspace: WorkspaceId,
        /// Exact provider selected by the session.
        provider: ProviderId,
        /// Exact binding identity.
        credential: ProviderCredentialId,
        /// Immutable sealed-source generation pinned by the session.
        source_generation: u64,
        /// Binding revision observed by admission.
        revision: u64,
    },
    /// The workspace authorization epoch has reached at least this.
    AuthorizationEpochAtLeast {
        /// Which workspace.
        workspace: WorkspaceId,
        /// The floor.
        at_least: AuthorizationEpoch,
    },
    /// The registry pointer carries exactly this tag.
    RegistryEtag {
        /// Which pointer.
        selector: RegistrySelector,
        /// The expected tag.
        expected: ETag,
    },
    /// The upload is in exactly this state.
    UploadState {
        /// Which upload.
        upload: UploadId,
        /// The expected state.
        expected: UploadState,
    },
    /// The workspace owns this body.
    ContentOwned {
        /// Which workspace.
        workspace: WorkspaceId,
        /// Which body.
        digest: ContentDigest,
    },
    /// The grant has not lapsed.
    GrantUnexpired {
        /// Which grant.
        grant: GrantId,
        /// The instant to evaluate against.
        now: Timestamp,
    },
    /// The work item's fence is exactly this.
    OperationFence {
        /// Which operation.
        operation: OperationId,
        /// The expected fence.
        fence: Fence,
    },
    /// The operation is still parked at exactly this cursor (D-4).
    ///
    /// This is what makes a step commit idempotent against a duplicate
    /// delivery: the condition names the cursor the step advances *from*, so
    /// the second delivery fails its condition and writes nothing.
    OperationCursorAt {
        /// Which operation.
        operation: OperationId,
        /// The cursor it must still carry; `None` means "no cursor yet", which
        /// is the first step of a continued operation.
        expected: Option<Box<aex_operation_domain::cursor::ContinuationCursor>>,
    },
    /// The operation row is at exactly this optimistic version.
    ///
    /// A resumed step **must** carry this alongside
    /// [`Condition::OperationCursorAt`], because the operation row has a second
    /// writer: the already-served public cancellation runs its own optimistic
    /// loop over `version` on the same item. Without a way to name the version
    /// the step observed, an adapter would have to guess one, and any guess
    /// either resets the counter or lets two writers believe they hold the row.
    ///
    /// It costs no extra action: it targets the same item as the cursor guard
    /// and the operation write, so the three merge into one physical action.
    OperationVersion {
        /// Which operation.
        operation: OperationId,
        /// The version the step read. The write advances it to
        /// [`aex_operation_domain::operation::OperationVersion::next`].
        expected: aex_operation_domain::operation::OperationVersion,
    },
    /// The secret's revocation epoch is exactly this.
    SecretRevocationEpoch {
        /// Which workspace.
        workspace: WorkspaceId,
        /// Which name.
        name: SecretName,
        /// The expected epoch.
        expected: RevocationEpoch,
    },
    /// The session's custody revision is exactly this.
    CustodyRevision {
        /// Which session.
        session: SessionId,
        /// The expected revision.
        expected: CustodyRevision,
    },
    /// The item does not exist.
    ItemAbsent(ItemKey),
    /// The item exists.
    ItemPresent(ItemKey),
}

impl Condition {
    /// The item family the condition reads, for adapter routing.
    #[must_use]
    pub const fn family(&self) -> TableFamily {
        match self {
            Self::SessionRevision { .. }
            | Self::SessionStatusIn { .. }
            | Self::SessionActiveRun { .. }
            | Self::WorkAdmission { .. }
            | Self::DeletionState { .. }
            | Self::CancellationEpoch { .. }
            | Self::MutationGuardFree { .. }
            | Self::MutationGuardHeldBy { .. }
            | Self::AgentRevision { .. }
            | Self::AgentFence { .. }
            | Self::JournalTail { .. }
            | Self::RootAgentIdle { .. }
            | Self::JournalTailHash { .. }
            | Self::RunNonTerminal { .. }
            | Self::AuthorizationEpochAtLeast { .. } => TableFamily::SessionAuthority,
            Self::AccountRevisionAtLeast { .. } => TableFamily::AuthorizationProjection,
            Self::ProviderCredentialReady { .. } => TableFamily::SecretCustody,
            Self::RegistryEtag { .. } | Self::UploadState { .. } => TableFamily::Registry,
            Self::OperationFence { .. }
            | Self::OperationCursorAt { .. }
            | Self::OperationVersion { .. } => TableFamily::OperationAuthority,
            Self::ContentOwned { .. } | Self::GrantUnexpired { .. } => {
                TableFamily::ContentAuthority
            }
            Self::SecretRevocationEpoch { .. } | Self::CustodyRevision { .. } => {
                TableFamily::SecretCustody
            }
            Self::ItemAbsent(key) | Self::ItemPresent(key) => key.family,
        }
    }

    /// The logical item this guard reads.
    ///
    /// Conditions sharing this identity are joined into one physical
    /// expression. If a write has the same identity, the expression belongs on
    /// that `Put`/`Update`/`Delete`; `DynamoDB` rejects a separate
    /// `ConditionCheck` against the same item.
    #[must_use]
    pub fn target(&self) -> ItemKey {
        let (partition, sort) = match self {
            Self::SessionRevision { session, .. }
            | Self::SessionStatusIn { session, .. }
            | Self::SessionActiveRun { session, .. }
            | Self::WorkAdmission { session, .. }
            | Self::DeletionState { session, .. }
            | Self::CancellationEpoch { session, .. }
            | Self::MutationGuardFree { session }
            | Self::MutationGuardHeldBy { session, .. } => (session.to_string(), "HEAD".to_owned()),
            Self::AgentRevision { session, agent, .. }
            | Self::AgentFence { session, agent, .. }
            | Self::JournalTail { session, agent, .. }
            | Self::RootAgentIdle { session, agent }
            | Self::JournalTailHash { session, agent, .. } => {
                (format!("{session}#{agent}"), "CONTROL".to_owned())
            }
            Self::RunNonTerminal { session, run } => (session.to_string(), format!("RUN#{run}")),
            Self::AccountRevisionAtLeast { workspace, .. } => {
                (workspace.to_string(), "ACCOUNT_ADMISSION".to_owned())
            }
            Self::ProviderCredentialReady {
                workspace,
                provider,
                credential,
                ..
            } => (
                workspace.to_string(),
                format!("PROVIDER_CREDENTIAL#{}#{credential}", provider.as_str()),
            ),
            Self::AuthorizationEpochAtLeast { workspace, .. } => {
                (workspace.to_string(), "AUTHORIZATION".to_owned())
            }
            Self::RegistryEtag { selector, .. } => (
                selector.workspace.to_string(),
                format!(
                    "REG#{}#{}",
                    registry_kind_tag(selector.kind),
                    selector.name.as_str()
                ),
            ),
            Self::UploadState { upload, .. } => (upload.to_string(), "UPLOAD".to_owned()),
            Self::ContentOwned { workspace, digest } => {
                (workspace.to_string(), format!("CONTENT#{digest}"))
            }
            Self::GrantUnexpired { grant, .. } => (grant.0.to_string(), "GRANT".to_owned()),
            Self::OperationFence { operation, .. }
            | Self::OperationCursorAt { operation, .. }
            | Self::OperationVersion { operation, .. } => {
                (operation.to_string(), "OPERATION".to_owned())
            }
            Self::SecretRevocationEpoch {
                workspace, name, ..
            } => (
                workspace.to_string(),
                format!("SECRET_REVOCATION#{}", name.as_str()),
            ),
            Self::CustodyRevision { session, .. } => (session.to_string(), "CUSTODY".to_owned()),
            Self::ItemAbsent(key) | Self::ItemPresent(key) => return key.clone(),
        };
        ItemKey {
            family: self.family(),
            partition,
            sort,
        }
    }
}

/// One exact regional-work claim retired by the authority that decided its
/// application outcome.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkCompletion {
    /// Canonical `regional-work` identity.
    pub work_id: String,
    /// Monotonic claim fence.
    pub fence: u64,
    /// Exact claim owner.
    pub owner: String,
    /// Settlement instant.
    pub at: Timestamp,
}

/// One immediate, deduplicated root-agent wake admitted with a message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentWake {
    /// Stable regional-work row identity.
    pub work_id: String,
    /// Hash-keyed one-outstanding-wake claim.
    pub dedupe_key: String,
    /// Owning session.
    pub session: SessionId,
    /// Root agent.
    pub agent: AgentId,
    /// First journal sequence the activation must observe.
    pub from: JournalSeq,
    /// Cancellation epoch the activation must still observe.
    pub cancellation: CancellationEpoch,
    /// Admission instant and due time.
    pub at: Timestamp,
}

/// The immutable customer-observable fact emitted with message admission.
///
/// Internal Brain run and agent identities are intentionally absent. Public
/// clients observe a session accepting a message, then follow the session and
/// message resources rather than an internal run resource.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MessageAdmittedEvent {
    /// Event identity.
    pub id: ObservationId,
    /// Owning session.
    pub session: SessionId,
    /// Stable event order derived from the journal sequence.
    pub sequence: u64,
    /// Canonical inline event body.
    pub body: Vec<u8>,
    /// Admission instant.
    pub at: Timestamp,
}

/// One durable change the transaction makes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Write {
    /// Replace the session head.
    PutSessionHead(Box<Session>),
    /// Replace a complete message authority row.
    PutMessage(Box<Message>),
    /// Append the immutable, seal-ordered public projection of a message.
    ///
    /// This is a distinct row from [`Write::PutMessage`]. The base row may be
    /// open and mutable; this projection is written only when the message is
    /// sealed, in the same transaction as that transition. Readers therefore
    /// never advance a cursor past an open row that might become visible later.
    PutSealedMessage(Box<Message>),
    /// Replace a run record.
    PutRun(Box<Run>),
    /// Replace an agent control record.
    PutAgentControl(Box<AgentControl>),
    /// Advance only the Brain-owned root control facts established by message admission.
    ///
    /// A whole-record `PutAgentControl` cannot safely represent the production
    /// Brain row. This narrow update preserves its leases, budget vectors and
    /// every other owner-specific attribute.
    AdmitRootRun {
        /// Owning session.
        session: SessionId,
        /// Root agent.
        agent: AgentId,
        /// Revision observed before admission.
        from_revision: AgentRevision,
        /// Revision after admission.
        to_revision: AgentRevision,
        /// Journal tail observed before admission.
        from_tail: JournalSeq,
        /// Tail after the one `run_admitted` append.
        to_tail: JournalSeq,
        /// Blake3 identity of the appended canonical record.
        entry_identity: aex_session_domain::EntryIdentity,
        /// Admission instant.
        at: Timestamp,
    },
    /// Fence the Brain root into cancelling its current run.
    ///
    /// The physical control row remains Brain-owned. This narrow update only
    /// advances the cancellation epoch and control revision and sets the
    /// durable stop latch; the successor activation consumes that latch by
    /// committing the canonical `RunFinished(cancelled)` boundary.
    RequestRootCancellation {
        /// Owning session.
        session: SessionId,
        /// Root agent.
        agent: AgentId,
        /// Control revision observed by admission.
        from_revision: AgentRevision,
        /// Control revision after installing the stop latch.
        to_revision: AgentRevision,
        /// Cancellation epoch the control row must still carry.
        from_cancellation: CancellationEpoch,
        /// Cancellation epoch installed with the latch.
        to_cancellation: CancellationEpoch,
        /// Admission instant.
        at: Timestamp,
    },
    /// Settle exactly one agent under a session-wide cancellation.
    ///
    /// Deliberately narrower than [`Write::PutAgentControl`]. The physical
    /// `agent_control` row is owned by `aex-brain-store-dynamodb` and its
    /// schema is that crate's `AgentHead`; `aex_session_domain::AgentControl`
    /// has **no** row codec anywhere in the tree, so a whole-record put from
    /// this side would have to invent every attribute it cannot know and would
    /// silently drop the ones it does not model. This arm names exactly the
    /// facts the domain cancellation establishes, and the adapter renders it as
    /// one conditional update that touches nothing else.
    CancelAgent {
        /// The owning session, required to locate the physical partition.
        session: SessionId,
        /// Which agent.
        agent: AgentId,
        /// The revision the row must still carry.
        from_revision: AgentRevision,
        /// The revision it moves to.
        to_revision: AgentRevision,
        /// When the settlement happened.
        at: Timestamp,
    },
    /// Append a journal page.
    AppendJournalPage {
        /// The owning session, required to locate the physical agent partition.
        session: SessionId,
        /// The complete immutable journal page.
        page: Box<JournalPage>,
    },
    /// Replace an approval.
    PutApproval(Box<Approval>),
    /// Write an idempotency receipt.
    PutIdempotencyReceipt(Box<IdempotencyReceipt>),
    /// Replace an operation record.
    PutOperation(Box<Operation>),
    /// Strip a purged session's operation result.
    RedactOperationResult(OperationId),
    /// Replace a durable work item.
    PutWorkItem(Box<aex_operation_domain::WorkItem>),
    /// Put the immediate root-agent wake row.
    PutAgentWake(Box<AgentWake>),
    /// Put the wake's one-outstanding dedupe claim as a distinct physical item.
    PutAgentWakeDedupe(Box<AgentWake>),
    /// Retire one exact fenced regional-work claim.
    CompleteWorkItem(Box<WorkCompletion>),
    /// Append a native outbox event.
    PutOutboxEvent(Box<OutboxEvent>),
    /// Append the public message-admitted event and outbox state.
    PutMessageAdmittedEvent(Box<MessageAdmittedEvent>),
    /// Add a pin.
    PutPin(Box<Pin>),
    /// Remove a pin.
    DeletePin(Box<Pin>),
    /// Replace a registry pointer.
    PutRegistryPointer(Box<RegistryPointer>),
    /// Replace an upload record.
    PutUpload(Box<Upload>),
    /// Write a download grant.
    PutGrant(Box<DownloadGrant>),
    /// Replace a session's credential custody.
    PutCustody(Box<SessionCustody>),
    /// Replace a workspace secret.
    PutSecret(Box<WorkspaceSecret>),
    /// Write a session tombstone.
    PutTombstone(Box<SessionTombstone>),
    /// Delete an item outright.
    DeleteItem(ItemKey),
}

impl Write {
    /// The item family the write targets.
    #[must_use]
    pub const fn family(&self) -> TableFamily {
        match self {
            Self::PutSessionHead(_)
            | Self::PutMessage(_)
            | Self::PutSealedMessage(_)
            | Self::PutRun(_)
            | Self::PutAgentControl(_)
            | Self::AdmitRootRun { .. }
            | Self::RequestRootCancellation { .. }
            | Self::AppendJournalPage { .. }
            | Self::PutApproval(_)
            | Self::CancelAgent { .. }
            | Self::PutTombstone(_) => TableFamily::SessionAuthority,
            Self::PutIdempotencyReceipt(_) => TableFamily::Idempotency,
            Self::PutOperation(_) | Self::RedactOperationResult(_) => {
                TableFamily::OperationAuthority
            }
            Self::PutWorkItem(_)
            | Self::PutAgentWake(_)
            | Self::PutAgentWakeDedupe(_)
            | Self::CompleteWorkItem(_) => TableFamily::WorkAuthority,
            Self::PutOutboxEvent(_) | Self::PutMessageAdmittedEvent(_) => TableFamily::Outbox,
            Self::PutPin(_) | Self::DeletePin(_) | Self::PutGrant(_) => {
                TableFamily::ContentAuthority
            }
            Self::PutRegistryPointer(_) | Self::PutUpload(_) => TableFamily::Registry,
            Self::PutCustody(_) | Self::PutSecret(_) => TableFamily::SecretCustody,
            Self::DeleteItem(key) => key.family,
        }
    }

    /// The item the write targets, for duplicate detection.
    #[must_use]
    pub fn target(&self) -> ItemKey {
        let (partition, sort) = match self {
            Self::PutSessionHead(session) => (session.id.to_string(), "HEAD".to_owned()),
            Self::PutMessage(message) => (
                message.session.to_string(),
                format!("MESSAGE#{}", message.id),
            ),
            Self::PutSealedMessage(message) => (
                message.session.to_string(),
                format!(
                    "SEALED_MESSAGE#{}#{}",
                    message
                        .sealed_at
                        .map_or_else(|| "UNSEALED".to_owned(), Timestamp::to_wire),
                    message.id
                ),
            ),
            Self::PutRun(run) => (run.session.to_string(), format!("RUN#{}", run.id)),
            Self::PutAgentControl(agent) => (
                format!("{}#{}", agent.session, agent.id),
                "CONTROL".to_owned(),
            ),
            Self::AdmitRootRun { session, agent, .. } => {
                (format!("{session}#{agent}"), "CONTROL".to_owned())
            }
            Self::RequestRootCancellation { session, agent, .. } => {
                (format!("{session}#{agent}"), "CONTROL".to_owned())
            }
            Self::CancelAgent { session, agent, .. } => {
                (format!("{session}#{agent}"), "CONTROL".to_owned())
            }
            Self::AppendJournalPage { session, page } => (
                format!("{session}#{}", page.agent),
                format!("JOURNAL#{}", page.first.0),
            ),
            Self::PutApproval(approval) => (
                approval.binding.session.to_string(),
                format!("APPROVAL#{}", approval.id),
            ),
            // `(scope, key_sha256)` and never the intent digest. Keyed by the
            // intent, two callers who asked for the same thing under different
            // keys would collide on one item, and a replay could not find its
            // own receipt without already knowing the value the receipt exists
            // to compare against — which is what made `idempotency_conflict`
            // unreachable.
            Self::PutIdempotencyReceipt(receipt) => (
                receipt.key.scope().to_owned(),
                receipt.key.key_sha256().to_owned(),
            ),
            Self::PutOperation(operation) => (operation.id.to_string(), "OPERATION".to_owned()),
            Self::RedactOperationResult(id) => (id.to_string(), "OPERATION".to_owned()),
            Self::PutWorkItem(item) => (item.id.to_string(), "STATE".to_owned()),
            Self::PutAgentWake(wake) => (wake.work_id.clone(), "STATE".to_owned()),
            Self::PutAgentWakeDedupe(wake) => (wake.dedupe_key.clone(), "DEDUPE".to_owned()),
            Self::CompleteWorkItem(item) => (item.work_id.clone(), "STATE".to_owned()),
            Self::PutOutboxEvent(event) => {
                (event.session.to_string(), format!("OUTBOX#{}", event.run))
            }
            Self::PutMessageAdmittedEvent(event) => (
                event.session.to_string(),
                format!("EVENT#{:020}", event.sequence),
            ),
            Self::PutPin(pin) | Self::DeletePin(pin) => ("PIN".to_owned(), format!("{pin:?}")),
            Self::PutRegistryPointer(pointer) => (
                pointer.row.workspace.to_string(),
                format!(
                    "REG#{}#{}",
                    registry_kind_tag(pointer.row.kind),
                    pointer.row.name.as_str()
                ),
            ),
            Self::PutUpload(upload) => (upload.id.to_string(), "UPLOAD".to_owned()),
            Self::PutGrant(grant) => (grant.id.0.to_string(), "GRANT".to_owned()),
            Self::PutCustody(custody) => (custody.session.to_string(), "CUSTODY".to_owned()),
            Self::PutSecret(secret) => (
                secret.workspace.to_string(),
                format!("SECRET#{}", secret.name.as_str()),
            ),
            Self::PutTombstone(tombstone) => {
                // The minimal tombstone replaces the public HEAD item. Sharing
                // its logical target lets the revision and mutation-guard
                // conditions compile onto the same conditional put; DynamoDB
                // transactions forbid a separate check and write for one key.
                (tombstone.session.to_string(), "HEAD".to_owned())
            }
            Self::DeleteItem(key) => return key.clone(),
        };
        ItemKey {
            family: self.family(),
            partition,
            sort,
        }
    }

    /// A conservative byte estimate, used only by the envelope check.
    #[must_use]
    pub fn estimated_bytes(&self) -> usize {
        match self {
            Self::AppendJournalPage { page, .. } => page
                .entries
                .iter()
                .map(|entry| {
                    usize::try_from(entry.body.inline_len().unwrap_or(32)).unwrap_or(usize::MAX)
                        + 128
                })
                .sum(),
            Self::PutMessage(message) | Self::PutSealedMessage(message) => {
                256 + message.parts.len() * 256
            }
            // A receipt carries the canonical response inline up to
            // `ResponseBody::MAX_INLINE_BYTES`, so a flat estimate would let a
            // transaction carrying several of them pass the 4 MiB envelope check
            // and fail at the provider instead.
            Self::PutIdempotencyReceipt(receipt) => {
                512 + match &receipt.outcome {
                    aex_session_domain::ReceiptOutcome::Resource { response, .. } => {
                        response.inline().map_or(0, <[u8]>::len)
                    }
                    aex_session_domain::ReceiptOutcome::Operation(_) => 0,
                }
            }
            _ => 512,
        }
    }
}

const fn registry_kind_tag(kind: RegistryKind) -> u8 {
    kind.discriminant()
}

const fn create_error(detail: &'static str) -> PlanError {
    PlanError::CreateAuthority { detail }
}

/// One after-commit notification.
///
/// A hint is never truth and never a condition. It says "there may be work", and
/// the authority says what the work is; a lost hint is a latency problem, never
/// a correctness one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Hint {
    /// An agent may be runnable.
    WakeAgent {
        /// Which agent.
        agent: AgentId,
        /// Why.
        reason: aex_internal_contracts::wake::WakeHint,
    },
    /// An operation may be due.
    OperationDue {
        /// Which operation.
        operation: OperationId,
        /// When.
        due_at: Timestamp,
    },
    /// Content lifecycle work may be due.
    ContentLifecycle {
        /// Which epoch.
        epoch: aex_content_domain::GcEpoch,
    },
    /// An owner key edge must be destroyed.
    DestroyKeyEdge {
        /// Which edge.
        edge: OwnerKeyEdgeId,
    },
}

/// One command's whole durable effect.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionTransaction {
    /// Which command produced it.
    pub intent: TransactionIntent,
    /// What must hold.
    pub conditions: Vec<Condition>,
    /// What changes.
    pub writes: Vec<Write>,
    /// What to notify afterwards.
    pub after_commit: Vec<Hint>,
}

/// What a validated plan looks like.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PlanShape {
    /// How many distinct physical item actions it carries after guard merging.
    pub actions: usize,
    /// Its estimated byte size.
    pub bytes: usize,
}

/// Why a plan is not submittable.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PlanError {
    /// A create plan does not carry exactly the participants a create must.
    ///
    /// The membership rule is A D-1 and it is total: five items always, plus
    /// custody only when the request named secrets, plus the root pin and its
    /// owner edge only when the sealed root retains something. A create that
    /// carries fewer orphans the session from an authority a caller can already
    /// observe; a create that carries more is writing something the create was
    /// not asked to write.
    #[error("session creation authority is wrong: {detail}")]
    CreateAuthority {
        /// Which part of the membership rule failed.
        detail: &'static str,
    },
    /// A registry download omitted or mixed one of its three atomic writes.
    #[error("registry download authority is wrong: {detail}")]
    RegistryDownloadAuthority {
        /// Which membership rule failed.
        detail: &'static str,
    },
    /// A message admission omitted or widened one of its atomic participants.
    #[error("message admission authority is wrong: {detail}")]
    MessageAdmissionAuthority {
        /// Which membership rule failed.
        detail: &'static str,
    },
    /// The plan carries too many actions.
    #[error("plan carries {actions} actions, above the maximum of {max}")]
    TooManyActions {
        /// How many.
        actions: usize,
        /// The maximum.
        max: usize,
    },
    /// The plan is too large.
    #[error("plan needs {bytes} bytes, above the maximum of {max}")]
    TooManyBytes {
        /// How many.
        bytes: usize,
        /// The maximum.
        max: usize,
    },
    /// A required condition is absent.
    #[error("plan is missing a required condition")]
    MissingRequiredCondition(ConditionId),
    /// Two writes target the same item.
    #[error("plan writes the same item twice")]
    DuplicateWriteTarget(Box<ItemKey>),
    /// A hint was placed where a condition belongs.
    #[error("a hint can never be a condition")]
    HintUsedAsCondition,
}

impl SessionTransaction {
    /// Whether the plan may be submitted.
    ///
    /// Fails rather than splitting (D-17): a silent split breaks "commits
    /// completely or not at all", which is the only reason the plan exists.
    ///
    /// # Errors
    ///
    /// Returns [`PlanError`] for an over-large plan or a duplicate write target.
    pub fn validate(&self) -> Result<PlanShape, PlanError> {
        if self.intent == TransactionIntent::CreateSession {
            self.check_create_participants()?;
        }
        if self.intent == TransactionIntent::AdmitMessage {
            self.check_message_admission_participants()?;
        }
        if self.intent == TransactionIntent::RegistryDownload {
            self.check_registry_download_participants()?;
        }
        let mut action_targets = BTreeSet::new();
        action_targets.extend(self.conditions.iter().map(Condition::target));
        action_targets.extend(self.writes.iter().map(Write::target));
        let actions = action_targets.len();
        if actions > MAX_ACTIONS {
            return Err(PlanError::TooManyActions {
                actions,
                max: MAX_ACTIONS,
            });
        }
        let bytes: usize = self.writes.iter().map(Write::estimated_bytes).sum();
        if bytes > MAX_BYTES {
            return Err(PlanError::TooManyBytes {
                bytes,
                max: MAX_BYTES,
            });
        }
        let mut seen: BTreeSet<ItemKey> = BTreeSet::new();
        for write in &self.writes {
            let target = write.target();
            if !seen.insert(target.clone()) {
                return Err(PlanError::DuplicateWriteTarget(Box::new(target)));
            }
        }
        Ok(PlanShape { actions, bytes })
    }

    /// The largest number of items a create transaction may ever carry.
    ///
    /// Four always: the ready public head, exact receipt, one condition on the
    /// already-started root control/journal authority, and the commit-time
    /// active-account fence. This is a
    /// **constant**, not a bound to grow into: a create whose action count grew
    /// with request size would be a create whose tail latency and
    /// `TransactionConflictException` rate grew with request size, which the
    /// performance-first ordering forbids (A D-5).
    pub const CREATE_MAX_ACTIONS: usize = 4;

    /// Enforces A D-1's create membership rule.
    ///
    /// Written as an exhaustive match over the write vocabulary rather than a
    /// set of `contains` checks, so a new `Write` arm has to be classified here
    /// instead of silently becoming a legal create participant.
    fn check_create_participants(&self) -> Result<(), PlanError> {
        let mut head: Option<&Session> = None;
        let mut root_writes = 0_usize;
        let mut receipts = 0_usize;
        for write in &self.writes {
            match write {
                Write::PutSessionHead(session) => {
                    if head.is_some() {
                        return Err(create_error("a create writes exactly one session head"));
                    }
                    head = Some(session);
                }
                Write::PutAgentControl(_) => root_writes += 1,
                Write::PutIdempotencyReceipt(_) => receipts += 1,
                Write::PutMessage(_)
                | Write::PutSealedMessage(_)
                | Write::PutRun(_)
                | Write::AdmitRootRun { .. }
                | Write::RequestRootCancellation { .. }
                | Write::CancelAgent { .. }
                | Write::AppendJournalPage { .. }
                | Write::PutApproval(_)
                | Write::PutOperation(_)
                | Write::RedactOperationResult(_)
                | Write::PutWorkItem(_)
                | Write::PutAgentWake(_)
                | Write::PutAgentWakeDedupe(_)
                | Write::CompleteWorkItem(_)
                | Write::PutOutboxEvent(_)
                | Write::PutMessageAdmittedEvent(_)
                | Write::PutPin(_)
                | Write::DeletePin(_)
                | Write::PutRegistryPointer(_)
                | Write::PutUpload(_)
                | Write::PutGrant(_)
                | Write::PutCustody(_)
                | Write::PutSecret(_)
                | Write::PutTombstone(_)
                | Write::DeleteItem(_) => {
                    return Err(create_error(
                        "ready publication writes only the public head and exact receipt",
                    ));
                }
            }
        }
        let Some(_head) = head else {
            return Err(create_error("a create writes exactly one session head"));
        };
        if root_writes != 0 {
            return Err(create_error(
                "ready publication must not overwrite the root control whose AgentStarted tail it conditions on",
            ));
        }
        if receipts != 1 {
            return Err(create_error(
                "a create writes exactly one idempotency receipt; the receipt's conditional put \
                 is the concurrency election, so a create without one can mint two sessions for \
                 one key",
            ));
        }
        let head = head.expect("checked above");
        let has_root_revision = self.conditions.iter().any(|condition| {
            matches!(condition, Condition::AgentRevision { session, agent, .. }
                if *session == head.id && *agent == head.root_agent)
        });
        let has_root_tail = self.conditions.iter().any(|condition| {
            matches!(condition, Condition::JournalTail { session, agent, .. }
                if *session == head.id && *agent == head.root_agent)
        });
        let has_root_tail_hash = self.conditions.iter().any(|condition| {
            matches!(condition, Condition::JournalTailHash { session, agent, .. }
                if *session == head.id && *agent == head.root_agent)
        });
        let has_active_account_fence = self.conditions.iter().any(|condition| {
            matches!(condition, Condition::AccountRevisionAtLeast { workspace, organization, .. }
                if *workspace == head.workspace && *organization == head.organization)
        });
        if !has_root_revision || !has_root_tail || !has_root_tail_hash || !has_active_account_fence
        {
            return Err(create_error(
                "ready publication must condition on the elected root AgentStarted revision, journal tail and tail hash plus the active-account revision",
            ));
        }
        Ok(())
    }

    /// Enforces the fixed 11-action message-admission authority.
    ///
    /// Ten immutable/update rows carry the message, its sealed projection,
    /// internal Brain execution facts, durable wake, public event, and exact
    /// receipt. The eleventh action is the read-only active-account projection
    /// fence. Head and root predicates merge onto their corresponding writes.
    #[expect(
        clippy::too_many_lines,
        reason = "the exhaustive closed participant and condition vocabularies must remain reviewable together"
    )]
    fn check_message_admission_participants(&self) -> Result<(), PlanError> {
        let mut writes = [0_u8; 10];
        for write in &self.writes {
            let slot = match write {
                Write::PutMessage(_) => 0,
                Write::PutSealedMessage(_) => 1,
                Write::PutRun(_) => 2,
                Write::PutSessionHead(_) => 3,
                Write::AdmitRootRun { .. } => 4,
                Write::AppendJournalPage { .. } => 5,
                Write::PutAgentWake(_) => 6,
                Write::PutAgentWakeDedupe(_) => 7,
                Write::PutMessageAdmittedEvent(_) => 8,
                Write::PutIdempotencyReceipt(_) => 9,
                Write::PutAgentControl(_)
                | Write::RequestRootCancellation { .. }
                | Write::CancelAgent { .. }
                | Write::PutApproval(_)
                | Write::PutOperation(_)
                | Write::RedactOperationResult(_)
                | Write::PutWorkItem(_)
                | Write::CompleteWorkItem(_)
                | Write::PutOutboxEvent(_)
                | Write::PutPin(_)
                | Write::DeletePin(_)
                | Write::PutRegistryPointer(_)
                | Write::PutUpload(_)
                | Write::PutGrant(_)
                | Write::PutCustody(_)
                | Write::PutSecret(_)
                | Write::PutTombstone(_)
                | Write::DeleteItem(_) => {
                    return Err(message_admission_error(
                        "admission writes only its ten closed participants",
                    ));
                }
            };
            writes[slot] = writes[slot].saturating_add(1);
        }
        if writes != [1; 10] {
            return Err(message_admission_error(
                "admission writes each of its ten participants exactly once",
            ));
        }

        let required = [
            self.conditions
                .iter()
                .any(|condition| matches!(condition, Condition::SessionRevision { .. })),
            self.conditions
                .iter()
                .any(|condition| matches!(condition, Condition::DeletionState { .. })),
            self.conditions.iter().any(|condition| {
                matches!(
                    condition,
                    Condition::SessionActiveRun { expected: None, .. }
                )
            }),
            self.conditions.iter().any(|condition| {
                matches!(
                    condition,
                    Condition::WorkAdmission {
                        expected: WorkAdmission::Open,
                        ..
                    }
                )
            }),
            self.conditions
                .iter()
                .any(|condition| matches!(condition, Condition::MutationGuardFree { .. })),
            self.conditions
                .iter()
                .any(|condition| matches!(condition, Condition::CancellationEpoch { .. })),
            self.conditions
                .iter()
                .any(|condition| matches!(condition, Condition::AccountRevisionAtLeast { .. })),
            self.conditions
                .iter()
                .any(|condition| matches!(condition, Condition::ProviderCredentialReady { .. })),
            self.conditions
                .iter()
                .any(|condition| matches!(condition, Condition::AgentRevision { .. })),
            self.conditions
                .iter()
                .any(|condition| matches!(condition, Condition::JournalTail { .. })),
            self.conditions
                .iter()
                .any(|condition| matches!(condition, Condition::RootAgentIdle { .. })),
        ];
        if required.contains(&false) {
            return Err(message_admission_error(
                "admission must fence the live idle session, active account, and exact Brain root",
            ));
        }
        if self.conditions.iter().any(|condition| {
            !matches!(
                condition,
                Condition::SessionRevision { .. }
                    | Condition::DeletionState { .. }
                    | Condition::SessionActiveRun { expected: None, .. }
                    | Condition::WorkAdmission {
                        expected: WorkAdmission::Open,
                        ..
                    }
                    | Condition::MutationGuardFree { .. }
                    | Condition::CancellationEpoch { .. }
                    | Condition::AccountRevisionAtLeast { .. }
                    | Condition::ProviderCredentialReady { .. }
                    | Condition::AgentRevision { .. }
                    | Condition::JournalTail { .. }
                    | Condition::RootAgentIdle { .. }
                    | Condition::ItemAbsent(_)
            )
        }) {
            return Err(message_admission_error(
                "admission carries no condition outside its closed authority",
            ));
        }
        let wake_absence = self
            .conditions
            .iter()
            .filter(|condition| matches!(condition, Condition::ItemAbsent(key) if key.family == TableFamily::WorkAuthority))
            .count();
        if wake_absence != 2 {
            return Err(message_admission_error(
                "admission conditions on both the wake and its dedupe row being absent",
            ));
        }
        let mut targets = BTreeSet::new();
        targets.extend(self.conditions.iter().map(Condition::target));
        targets.extend(self.writes.iter().map(Write::target));
        if targets.len() != 12 {
            return Err(message_admission_error(
                "admission is exactly twelve physical actions after guard merging",
            ));
        }
        Ok(())
    }

    /// The stable identity of each condition, in plan order.
    #[must_use]
    pub fn condition_ids(&self) -> Vec<ConditionId> {
        (0..self.conditions.len())
            .map(|index| ConditionId(u16::try_from(index).unwrap_or(u16::MAX)))
            .collect()
    }

    fn registry_download_guards(
        &self,
    ) -> Result<(&RegistrySelector, (WorkspaceId, ContentDigest)), PlanError> {
        let mut pointer = None;
        let mut content = None;
        for condition in &self.conditions {
            match condition {
                Condition::RegistryEtag { selector, .. } if pointer.is_none() => {
                    pointer = Some(selector);
                }
                Condition::ContentOwned { workspace, digest } if content.is_none() => {
                    content = Some((*workspace, *digest));
                }
                _ => {
                    return Err(registry_download_error(
                        "a registry download requires exactly one file-pointer ETag guard and one content-ownership guard",
                    ));
                }
            }
        }
        pointer.zip(content).ok_or_else(|| {
            registry_download_error(
                "a registry download requires exactly one file-pointer ETag guard and one content-ownership guard",
            )
        })
    }

    fn registry_download_writes(
        &self,
    ) -> Result<(&DownloadGrant, &Pin, &IdempotencyReceipt), PlanError> {
        let mut grant = None;
        let mut pin = None;
        let mut receipt = None;
        for write in &self.writes {
            match write {
                Write::PutGrant(value) if grant.is_none() => grant = Some(value.as_ref()),
                Write::PutPin(value)
                    if pin.is_none() && matches!(value.as_ref(), Pin::Grant { .. }) =>
                {
                    pin = Some(value.as_ref());
                }
                Write::PutIdempotencyReceipt(value) if receipt.is_none() => {
                    receipt = Some(value.as_ref());
                }
                _ => {
                    return Err(registry_download_error(
                        "a registry download writes only one grant, its matching grant pin, and one receipt",
                    ));
                }
            }
        }
        grant
            .zip(pin)
            .zip(receipt)
            .map(|((grant, pin), receipt)| (grant, pin, receipt))
            .ok_or_else(|| {
                registry_download_error(
                    "a registry download requires one grant, one grant pin, and one replay receipt",
                )
            })
    }

    fn check_registry_download_participants(&self) -> Result<(), PlanError> {
        if !self.after_commit.is_empty() {
            return Err(registry_download_error(
                "a registry download has no after-commit side channel",
            ));
        }
        let (pointer, (content_workspace, content_digest)) = self.registry_download_guards()?;
        let (grant, pin, receipt) = self.registry_download_writes()?;
        let Pin::Grant {
            grant: pin_grant,
            digest,
            expires_at,
        } = pin
        else {
            return Err(registry_download_error(
                "a registry download requires one grant and one grant pin",
            ));
        };
        if *pin_grant != grant.id
            || *digest != grant.whole_sha256
            || *expires_at != grant.expires_at
        {
            return Err(registry_download_error(
                "the grant pin must match the grant id, digest, and expiry",
            ));
        }
        if grant.subject.session.is_some()
            || pointer.kind != RegistryKind::File
            || pointer.workspace != grant.subject.workspace
            || content_workspace != grant.subject.workspace
            || content_digest != grant.whole_sha256
        {
            return Err(registry_download_error(
                "the file pointer, owned content, and workspace grant authority must match",
            ));
        }
        let aex_session_domain::ReceiptOutcome::Resource { kind, id, response } = &receipt.outcome
        else {
            return Err(registry_download_error(
                "a registry download receipt must reproduce its grant",
            ));
        };
        if *kind != aex_session_domain::ResourceKind::Grant
            || id.0 != grant.id.0.to_string()
            || !matches!(response, aex_session_domain::ResponseBody::Inline(_))
            || receipt.expires_at != Some(grant.expires_at)
        {
            return Err(registry_download_error(
                "a registry download receipt must reproduce its grant id, response, and expiry",
            ));
        }
        Ok(())
    }
}

const fn registry_download_error(detail: &'static str) -> PlanError {
    PlanError::RegistryDownloadAuthority { detail }
}

const fn message_admission_error(detail: &'static str) -> PlanError {
    PlanError::MessageAdmissionAuthority { detail }
}

/// A projected result plus the one plan that would produce it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Planned<T> {
    /// The transaction the deployable submits.
    pub plan: SessionTransaction,
    /// What the caller is told, once the plan commits.
    pub projected: T,
}

#[cfg(test)]
mod tests {
    use super::{
        Condition, ItemKey, MAX_ACTIONS, PlanError, SessionTransaction, TableFamily,
        TransactionIntent, Write,
    };

    fn key(sort: &str) -> ItemKey {
        ItemKey {
            family: TableFamily::SessionAuthority,
            partition: "ses".to_owned(),
            sort: sort.to_owned(),
        }
    }

    fn plan(conditions: Vec<Condition>, writes: Vec<Write>) -> SessionTransaction {
        SessionTransaction {
            intent: TransactionIntent::CommitTerminal,
            conditions,
            writes,
            after_commit: Vec::new(),
        }
    }

    #[test]
    fn an_empty_plan_validates() {
        let shape = plan(Vec::new(), Vec::new()).validate().expect("validates");
        assert_eq!(shape.actions, 0);
        assert_eq!(shape.bytes, 0);
    }

    #[test]
    fn a_duplicate_write_target_is_rejected() {
        let outcome = plan(
            Vec::new(),
            vec![Write::DeleteItem(key("A")), Write::DeleteItem(key("A"))],
        )
        .validate();
        assert!(matches!(outcome, Err(PlanError::DuplicateWriteTarget(_))));
    }

    #[test]
    fn an_over_large_plan_fails_rather_than_splitting() {
        let writes: Vec<Write> = (0..=MAX_ACTIONS)
            .map(|index| Write::DeleteItem(key(&index.to_string())))
            .collect();
        assert!(matches!(
            plan(Vec::new(), writes).validate(),
            Err(PlanError::TooManyActions { .. })
        ));
    }

    #[test]
    fn condition_ids_are_positional_and_stable() {
        let value = plan(
            vec![
                Condition::MutationGuardFree {
                    session: aex_wire::ids::PrefixedId::from_uuid7(aex_wire::ids::Uuid7::compose(
                        1, [1; 10],
                    )),
                },
                Condition::ItemAbsent(key("A")),
            ],
            Vec::new(),
        );
        let ids = value.condition_ids();
        assert_eq!(ids.len(), 2);
        assert_eq!(ids, value.condition_ids());
    }

    /// A create plan over the domain fixture.
    fn create_plan(session: aex_session_domain::Session) -> SessionTransaction {
        let receipt = crate::testing::create_receipt(&session);
        SessionTransaction {
            intent: TransactionIntent::CreateSession,
            conditions: vec![
                Condition::ItemAbsent(ItemKey {
                    family: TableFamily::SessionAuthority,
                    partition: session.id.to_string(),
                    sort: "HEAD".to_owned(),
                }),
                Condition::AccountRevisionAtLeast {
                    workspace: session.workspace,
                    organization: session.organization,
                    at_least: aex_session_domain::AccountRevision(7),
                },
                Condition::AgentRevision {
                    session: session.id,
                    agent: session.root_agent,
                    expected: aex_session_domain::AgentRevision::INITIAL,
                },
                Condition::JournalTail {
                    session: session.id,
                    agent: session.root_agent,
                    expected: aex_session_domain::JournalSeq::INITIAL,
                },
                Condition::JournalTailHash {
                    session: session.id,
                    agent: session.root_agent,
                    expected: [7; 32],
                },
            ],
            writes: vec![
                Write::PutSessionHead(Box::new(session)),
                Write::PutIdempotencyReceipt(Box::new(receipt)),
            ],
            after_commit: Vec::new(),
        }
    }

    #[test]
    fn a_head_only_create_is_still_refused_by_the_participant_rule() {
        let session = aex_session_domain::testing::session_fixture();
        let value = SessionTransaction {
            intent: TransactionIntent::CreateSession,
            conditions: Vec::new(),
            writes: vec![Write::PutSessionHead(Box::new(session))],
            after_commit: Vec::new(),
        };
        assert!(
            matches!(value.validate(), Err(PlanError::CreateAuthority { .. })),
            "a head-only transaction orphans the session from its root agent and its replay receipt"
        );
    }

    #[test]
    fn ready_create_publication_is_four_constant_actions() {
        let shape = create_plan(aex_session_domain::testing::session_fixture())
            .validate()
            .expect("the minimal create is complete");
        assert_eq!(shape.actions, SessionTransaction::CREATE_MAX_ACTIONS);
    }

    #[test]
    fn a_create_may_not_smuggle_a_write_a_create_was_not_asked_for() {
        let mut plan = create_plan(aex_session_domain::testing::session_fixture());
        plan.writes.push(Write::DeleteItem(key("SOMETHING")));
        assert!(matches!(
            plan.validate(),
            Err(PlanError::CreateAuthority { .. })
        ));
    }

    #[test]
    fn a_create_without_its_receipt_cannot_elect_a_winner() {
        let mut plan = create_plan(aex_session_domain::testing::session_fixture());
        plan.writes
            .retain(|write| !matches!(write, Write::PutIdempotencyReceipt(_)));
        assert!(matches!(
            plan.validate(),
            Err(PlanError::CreateAuthority { .. })
        ));
    }

    #[test]
    fn guards_on_a_written_item_count_as_one_physical_action() {
        let session =
            aex_wire::ids::PrefixedId::from_uuid7(aex_wire::ids::Uuid7::compose(1, [1; 10]));
        let value = plan(
            vec![
                Condition::MutationGuardFree { session },
                Condition::SessionActiveRun {
                    session,
                    expected: None,
                },
            ],
            vec![Write::DeleteItem(ItemKey {
                family: TableFamily::SessionAuthority,
                partition: session.to_string(),
                sort: "HEAD".to_owned(),
            })],
        );
        assert_eq!(value.validate().expect("valid").actions, 1);
    }
}
