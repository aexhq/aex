//! The four store ports — implemented by `aex-brain-store-aws`.
//!
//! Two rules shape every signature here.
//!
//! - **Settlement is not on [`EffectStore`].** `Complete`, `KnownFailure` and
//!   `OutcomeUnknown` are appended inside a [`DecisionCommit`] so the outcome and the
//!   journal record it produced land in one transaction. A separate settlement call would
//!   create a window where an effect is settled and the journal does not say so.
//! - **[`WakeQueue`] has no `enqueue`.** Wakes are created only inside a `DecisionCommit`,
//!   so the queue is a delivery hint and the durable item is the fact. A queue that could
//!   originate a wake would be a second authority.

use super::BoxFuture;
use super::proof::{DispatchTicket, FenceGuard};
use aex_brain_domain::budget::BudgetNode;
use aex_brain_domain::commit::DecisionCommit;
use aex_brain_domain::effect::{DispatchEvidence, DurableEffect};
use aex_brain_domain::ids::{
    AgentKey, AgentRevision, CancelEpoch, ContentHash, EffectId, Fence, JournalSeq, OwnerToken,
    Timestamp, WakeId, WorkShard,
};
use aex_brain_domain::journal::{FinishReason, JournalEntry, ParkReason};
use aex_wire::ids::{GenerationId, OrganizationId, WorkspaceId};
use std::collections::BTreeMap;

/// The session-head facts that own every Brain row written for one activation.
///
/// This is read from `session-authority` for the claimed session. It is deliberately not
/// part of process configuration: one mux serves sessions from many tenants, and a deletion
/// epoch is valid only for the session head it came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionAuthority {
    /// The workspace that owns every row and wake.
    pub workspace: WorkspaceId,
    /// The organization charged for the work.
    pub organization: OrganizationId,
    /// The deletion generation the decision must still observe.
    pub deletion_epoch: u64,
}

/// Per-decision facts supplied to the durable transaction compiler.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecisionContext {
    /// Authority derived from this activation's session head.
    pub authority: SessionAuthority,
    /// The lease expiry the control update carries forward.
    pub lease_expires_at: Timestamp,
    /// The activation clock reading used by this decision.
    pub now: Timestamp,
}

/// The agent control item, as one conditional read returns it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentHead {
    /// Which agent.
    pub key: AgentKey,
    /// The canonical Hands generation inherited from the session authority.
    pub generation: GenerationId,
    /// The commit counter.
    pub revision: AgentRevision,
    /// The ownership generation.
    pub fence: Fence,
    /// The last committed sequence.
    pub journal_tail: Option<JournalSeq>,
    /// The content hash at `journal_tail`.
    ///
    /// Sequence alone cannot prove a restored journal is the history the control authority
    /// claimed: a fork at the same tail has the same count. The two values are therefore
    /// either both present or both absent, and activation verifies both before recovery or
    /// planning.
    pub journal_tail_hash: Option<ContentHash>,
    /// The session cancellation epoch.
    pub cancel_epoch: CancelEpoch,
    /// The terminal reason, once the agent has one.
    pub finish: Option<FinishReason>,
    /// A stable phase tag.
    pub phase: String,
    /// The budget node.
    pub budget: BudgetNode,
    /// Whether a fenced stop has been requested.
    pub stop_requested: bool,
    /// The effects open at the time of the read, summarized.
    pub open_effects: Vec<EffectId>,
    /// When the current lease expires.
    pub lease_expires_at: Timestamp,
}

/// An opaque native continuation for one journal query.
///
/// The application reads only `next`; the adapter owns `parts` and must validate the whole
/// tuple before using it. Keeping the native key prevents a short `DynamoDB` page from being
/// mistaken for EOF merely because it returned fewer than the caller's entry limit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JournalCursor {
    parts: BTreeMap<String, String>,
    start: JournalSeq,
    next: JournalSeq,
}

impl JournalCursor {
    /// Builds an adapter-owned cursor.
    #[must_use]
    pub fn new<I, K, V>(parts: I, start: JournalSeq, next: JournalSeq) -> Self
    where
        I: IntoIterator<Item = (K, V)>,
        K: Into<String>,
        V: Into<String>,
    {
        Self {
            parts: parts
                .into_iter()
                .map(|(name, value)| (name.into(), value.into()))
                .collect(),
            start,
            next,
        }
    }

    /// Returns the complete adapter-owned native key.
    #[must_use]
    pub fn parts(&self) -> &BTreeMap<String, String> {
        &self.parts
    }

    /// The first sequence in the original query whose native key produced this cursor.
    #[must_use]
    pub const fn start(&self) -> JournalSeq {
        self.start
    }

    /// The sequence the next page must begin with.
    #[must_use]
    pub const fn next(&self) -> JournalSeq {
        self.next
    }
}

/// One page of an agent's journal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JournalPage {
    /// The entries, contiguous and in order.
    pub entries: Vec<JournalEntry>,
    /// Bytes hydrated by this page, including placed bodies.
    pub hydrated_bytes: usize,
    /// The native continuation when the service did not reach EOF.
    pub next: Option<JournalCursor>,
}

/// The bounds one page read runs under.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReadBudget {
    /// The most entries one page returns.
    pub max_entries: usize,
    /// The most bytes one page hydrates, including placed bodies.
    ///
    /// Bounded on purpose: an unbounded read is how one large agent takes the whole task's
    /// memory envelope with it.
    pub max_bytes: usize,
}

/// What a committed decision produced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommitReceipt {
    /// The revision the commit wrote.
    pub revision: AgentRevision,
    /// The tail the commit wrote.
    pub tail: JournalSeq,
    /// The wakes the commit created.
    pub wakes: Vec<WakeId>,
    /// When the authority accepted it.
    pub committed_at: Timestamp,
}

/// The agent's journal: read, page and commit.
pub trait JournalStore: Send + Sync + 'static {
    /// Reads the control item.
    fn load_head<'a>(
        &'a self,
        key: &'a AgentKey,
    ) -> BoxFuture<'a, Result<Option<AgentHead>, StoreError>>;

    /// Reads one page from `from`.
    ///
    /// A page that observes a gap is [`StoreError::JournalGap`] and never returns partial
    /// entries: the agent does not fold and does not act.
    fn read_page<'a>(
        &'a self,
        key: &'a AgentKey,
        from: JournalSeq,
        budget: ReadBudget,
        after: Option<JournalCursor>,
    ) -> BoxFuture<'a, Result<JournalPage, StoreError>>;

    /// Commits one decision as one transaction.
    ///
    /// A redelivered wake producing a byte-identical decision hits `attribute_not_exists`
    /// on the journal put and returns [`ConditionFailure::IdempotentReplay`], which the
    /// caller treats as success.
    fn commit<'a>(
        &'a self,
        context: &'a DecisionContext,
        commit: &'a DecisionCommit,
    ) -> BoxFuture<'a, Result<CommitReceipt, CommitError>>;
}

/// The durable effect record's two pre-settlement transitions.
pub trait EffectStore: Send + Sync + 'static {
    /// Commits the durable pre-send write and mints the permission to dispatch.
    ///
    /// This is a separate durable transaction, not an optimization to remove. Without it a
    /// crash between `Prepared` and the socket write is indistinguishable from a crash after
    /// it, and every prepared effect becomes ambiguous. The transaction checks the current
    /// session lifecycle/cancellation/deletion facts plus agent ownership while moving the
    /// effect, so cancellation or purge cannot race ticket minting. It costs no extra read.
    fn mark_dispatch_started<'a>(
        &'a self,
        guard: &'a FenceGuard,
        authority: &'a SessionAuthority,
        effect: &'a EffectId,
        attempt: u16,
        at: Timestamp,
    ) -> BoxFuture<'a, Result<DispatchTicket, CommitError>>;

    /// Records that the first validated response byte arrived.
    fn mark_response_started<'a>(
        &'a self,
        ticket: &'a DispatchTicket,
        evidence: &'a DispatchEvidence,
    ) -> BoxFuture<'a, Result<(), CommitError>>;

    /// Reads every effect this agent left open, so a new owner can classify them.
    fn load_open<'a>(
        &'a self,
        key: &'a AgentKey,
    ) -> BoxFuture<'a, Result<Vec<DurableEffect>, StoreError>>;
}

/// A claim on an agent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Claim {
    /// Which agent.
    pub key: AgentKey,
    /// The token this attempt minted.
    pub owner: OwnerToken,
    /// The ownership generation the claim took.
    pub fence: Fence,
    /// When the lease expires.
    pub expires_at: Timestamp,
    /// The authoritative tenant and deletion generation read from this session's head.
    pub authority: SessionAuthority,
    /// The agent head, returned by the same conditional write.
    ///
    /// Carried here rather than fetched separately because the measured cost of becoming
    /// an owner was dominated by redundant agent-control reads. The session authority above
    /// requires its own strongly-consistent head read because it lives on another item.
    pub head: AgentHead,
}

/// Activation ownership.
pub trait LeaseStore: Send + Sync + 'static {
    /// Takes ownership, bumping the fence, and returns both agent and session authority.
    fn claim<'a>(
        &'a self,
        key: &'a AgentKey,
        owner: OwnerToken,
        ttl: core::time::Duration,
        now: Timestamp,
    ) -> BoxFuture<'a, Result<Claim, ClaimError>>;

    /// Extends an existing claim.
    ///
    /// A renewal must **not** move the fence: it proves nothing changed hands, and moving
    /// it would fence out the very owner doing the renewing.
    fn renew<'a>(
        &'a self,
        claim: &'a Claim,
        ttl: core::time::Duration,
        now: Timestamp,
    ) -> BoxFuture<'a, Result<Claim, ClaimError>>;

    /// Gives up ownership.
    ///
    /// Every disposition ends this ownership scope and expires the lease immediately. The
    /// exact fence and owner still guard the release, so a delayed predecessor cannot clear
    /// a successor's claim.
    fn release(
        &self,
        claim: Claim,
        disposition: ReleaseDisposition,
    ) -> BoxFuture<'_, Result<(), StoreError>>;
}

/// Why a claim failed.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ClaimError {
    /// Another owner holds a live lease.
    #[error("held by another owner until {expires_at:?}")]
    HeldByOther {
        /// When their lease expires.
        expires_at: Timestamp,
    },
    /// The agent's fence moved past the one this claim assumed.
    #[error("fenced out; current fence is {current:?}")]
    Fenced {
        /// The fence the agent actually carries.
        current: Fence,
    },
    /// The owning session is terminal and cannot mint decision authority.
    #[error("session is terminal")]
    Terminal,
    /// The store failed.
    #[error(transparent)]
    Store(#[from] StoreError),
}

/// Why a lease was given up.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReleaseDisposition {
    /// The activation committed and has nothing more to do now.
    Committed,
    /// The activation parked on a durable wait.
    Parked,
    /// The activation gave up without committing.
    Abandoned,
    /// The process is draining.
    Drain,
}

/// One wake as the queue delivered it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WakeDelivery {
    /// The durable wake.
    pub wake: DurableWake,
    /// Whether this came from the queue projection or the durable due backstop.
    pub origin: WakeOrigin,
}

/// One queue record that could not be decoded into a durable wake.
///
/// The body and receipt are never exposed as diagnostics. The adapter retains only the
/// receipt needed to apply the poison policy, the approximate receive count, a closed reason
/// class and a short digest suitable for bounded correlation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MalformedWakeDelivery {
    /// Queue receipt, when the transport supplied one.
    pub receipt: Option<String>,
    /// Approximate number of times the queue has delivered this record.
    pub receive_count: u32,
    /// Closed decode failure class.
    pub reason: MalformedWakeReason,
    /// Sixteen lowercase hexadecimal characters derived from the body, never the body.
    pub fingerprint: String,
}

/// Why a queue record could not become a wake.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MalformedWakeReason {
    /// The transport supplied no receipt, so the record cannot be released or acknowledged.
    MissingReceipt,
    /// The transport supplied no message body.
    MissingBody,
    /// The body was present but was not a valid durable-wake projection.
    InvalidProjection,
}

/// One isolated queue receive.
///
/// Malformed records are siblings of valid deliveries, not an error for the whole receive.
/// The application applies the configured max-receive policy to each malformed record while
/// valid siblings continue through admission.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct WakeBatch {
    /// Valid decoded deliveries.
    pub deliveries: Vec<WakeDelivery>,
    /// Invalid records retained only for release or poison acknowledgement.
    pub malformed: Vec<MalformedWakeDelivery>,
}

impl WakeDelivery {
    /// How many times the queue has delivered this hint.
    ///
    /// A due-scan recovery is not a queue delivery and therefore has no poison count.
    #[must_use]
    pub const fn receive_count(&self) -> Option<u32> {
        match self.origin {
            WakeOrigin::Queue { receive_count, .. } => Some(receive_count),
            WakeOrigin::DueScan => None,
        }
    }
}

/// The transport provenance of one delivery.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WakeOrigin {
    /// An at-least-once queue projection carrying a real receipt handle.
    Queue {
        /// The receipt used for visibility and acknowledgement.
        receipt: String,
        /// The approximate queue receive count.
        receive_count: u32,
    },
    /// A durable row recovered directly from one due-index shard.
    DueScan,
}

/// The authoritative state of a delivered source wake.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WakeState {
    /// The row is still pending and may admit an activation.
    Pending,
    /// The row is done, poisoned, or already reclaimed by TTL.
    Retired,
}

/// A durable wake item.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DurableWake {
    /// The wake identity.
    pub id: WakeId,
    /// The canonical source identity in `regional-work`.
    pub work_id: String,
    /// Which agent to wake.
    pub key: AgentKey,
    /// The key that collapses duplicates before admission.
    pub dedup_key: String,
    /// Why the agent is being woken.
    pub reason: ParkReason,
    /// When it became due, for the reasons that have a due time.
    pub due: Option<Timestamp>,
    /// Scheduling priority; lower is sooner.
    pub priority: u8,
    /// The tenant it is fair-shared under.
    pub tenant: String,
}

/// An opaque continuation returned by one due-index adapter.
///
/// The application never interprets the parts. It retains at most one cursor per shard and
/// returns it to the same port on the next bounded query. A map is used instead of a vendor
/// attribute type so the port stays SDK-independent while preserving the complete native
/// continuation tuple.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DueScanCursor {
    parts: BTreeMap<String, String>,
}

impl DueScanCursor {
    /// Builds a cursor from adapter-owned string parts.
    #[must_use]
    pub fn new<I, K, V>(parts: I) -> Self
    where
        I: IntoIterator<Item = (K, V)>,
        K: Into<String>,
        V: Into<String>,
    {
        Self {
            parts: parts
                .into_iter()
                .map(|(name, value)| (name.into(), value.into()))
                .collect(),
        }
    }

    /// Returns the adapter-owned parts unchanged.
    #[must_use]
    pub fn parts(&self) -> &BTreeMap<String, String> {
        &self.parts
    }
}

/// One bounded page from a due shard.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DueScanPage {
    /// Valid pending agent wakes decoded from this page.
    pub wakes: Vec<DurableWake>,
    /// Native continuation when more rows remain in this shard pass. `None` wraps the next
    /// pass to the shard head.
    pub next: Option<DueScanCursor>,
    /// Rows isolated because they were malformed. They do not discard valid siblings and
    /// the continuation advances past them.
    pub malformed: usize,
    /// A bounded, redacted sample of the isolated rows.
    ///
    /// Isolation is deliberately read-only: [`WakeQueue`] is a delivery port and must not
    /// gain authority to rewrite or delete another adapter's work row. The cursor advances
    /// past the row, the durable source remains available for operator repair, and this
    /// sample provides an opaque correlation fingerprint without exposing a tenant or row
    /// key. At most [`MAX_DUE_ROW_ISOLATIONS`] entries are returned per page; `malformed`
    /// remains the complete count when the sample is full.
    pub isolations: Vec<DueRowIsolation>,
}

/// The largest operational sample one due page may return.
///
/// This is a diagnostic-memory bound, not a scheduling limit. The query's caller-selected
/// page size remains the bound on work inspected.
pub const MAX_DUE_ROW_ISOLATIONS: usize = 8;

/// One due-index row that was isolated instead of becoming runnable work.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DueRowIsolation {
    /// The closed, non-sensitive reason class.
    pub reason: DueRowIsolationReason,
    /// A short digest of the base/index key tuple, never a raw key or tenant identifier.
    pub fingerprint: String,
}

/// Why a due-index row was isolated.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DueRowIsolationReason {
    /// A required projected value was missing, mistyped, or outside its closed vocabulary.
    MalformedProjection,
    /// `pk` or `sk` did not name the projected `workId`.
    BaseKeyMismatch,
    /// `dueShardPk` was not the shard derived from `workId`.
    ShardMismatch,
    /// `dueShardSk` did not equal the effective due instant derived from due, priority and
    /// `workId`. This also isolates a row forged into an earlier scan position.
    DuePositionMismatch,
}

/// Wake **delivery**. Creation lives in [`DecisionCommit`] and nowhere else.
pub trait WakeQueue: Send + Sync + 'static {
    /// Receives up to `max` deliveries, long-polling for `wait`.
    fn receive(
        &self,
        max: usize,
        wait: core::time::Duration,
    ) -> BoxFuture<'_, Result<WakeBatch, StoreError>>;

    /// Returns one malformed queue record to visibility.
    ///
    /// This is intentionally separate from [`WakeQueue::release`]: an undecodable body has
    /// no [`DurableWake`] and must never be padded with an invented identity.
    fn release_malformed(
        &self,
        delivery: MalformedWakeDelivery,
        after: core::time::Duration,
    ) -> BoxFuture<'_, Result<(), StoreError>>;

    /// Acknowledges one malformed record after its max-receive threshold is reached.
    fn ack_malformed(
        &self,
        delivery: MalformedWakeDelivery,
    ) -> BoxFuture<'_, Result<(), StoreError>>;

    /// Strongly verifies the source row and reports whether it remains outstanding.
    ///
    /// This is the fail-closed bridge from a delivery hint back to authority: the adapter
    /// checks the exact work, workspace, session and agent before an activation may run.
    fn state<'a>(&'a self, wake: &'a DurableWake) -> BoxFuture<'a, Result<WakeState, StoreError>>;

    /// Extends a delivery's visibility. Called only while a claim is live and progressing:
    /// extending visibility for work that is not progressing hides a stuck activation.
    fn extend_visibility<'a>(
        &'a self,
        delivery: &'a WakeDelivery,
        by: core::time::Duration,
    ) -> BoxFuture<'a, Result<(), StoreError>>;

    /// Returns a delivery to the queue after `after`.
    fn release(
        &self,
        delivery: WakeDelivery,
        after: core::time::Duration,
    ) -> BoxFuture<'_, Result<(), StoreError>>;

    /// Acknowledges a delivery. Called only **after** the decision commits: deleting a
    /// message is not a commit, and acking first would lose the wake on a crash.
    fn ack(&self, delivery: WakeDelivery) -> BoxFuture<'_, Result<(), StoreError>>;

    /// Scans one due shard for wakes that are due and unclaimed.
    ///
    /// This is the backstop that restores scheduling when the stream projection into the
    /// queue is delayed or lost, which is why it reads the durable items rather than the
    /// queue.
    fn due_scan(
        &self,
        shard: WorkShard,
        now: Timestamp,
        max: usize,
        after: Option<DueScanCursor>,
    ) -> BoxFuture<'_, Result<DueScanPage, StoreError>>;
}

/// Why a conditional write was refused.
///
/// Every arm names one precondition, because "the condition failed" tells the caller
/// nothing about whether to reload, replan, page or quarantine.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ConditionFailure {
    /// The revision moved.
    #[error("revision moved")]
    StaleRevision,
    /// Another owner took the agent. The losing owner publishes nothing.
    #[error("fenced out by a newer owner")]
    StaleFence,
    /// The lease expired.
    #[error("lease lost")]
    LeaseLost,
    /// The session was cancelled.
    #[error("cancellation epoch advanced")]
    CancelEpochAdvanced,
    /// The journal tail is not what the commit assumed.
    #[error("journal tail moved")]
    UnexpectedTail,
    /// An effect was not in the state the write required.
    #[error("effect {effect} is not in the expected state")]
    EffectStateMismatch {
        /// The effect.
        effect: EffectId,
    },
    /// A budget dimension had no headroom.
    #[error("budget exhausted")]
    BudgetExhausted,
    /// A child was not in the state the write required.
    #[error("child state moved")]
    ChildStateMismatch,
    /// The delivered source wake was no longer the pending row this decision observed.
    #[error("source wake state moved")]
    WakeStateMoved,
    /// The identical decision already committed. The caller treats this as success.
    #[error("already committed")]
    IdempotentReplay(Box<CommitReceipt>),
}

/// Why a commit failed.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CommitError {
    /// A precondition was not met.
    #[error(transparent)]
    Condition(#[from] ConditionFailure),
    /// The decision would not fit one transaction. Rejected before the call, so the caller
    /// pages instead of paying a round trip for an undiagnosable size rejection.
    #[error(transparent)]
    Envelope(#[from] aex_brain_domain::commit::EnvelopeViolation),
    /// The store asked the caller to slow down.
    #[error("throttled; retry after {retry_after:?}")]
    Throttled {
        /// How long to wait.
        retry_after: core::time::Duration,
    },
    /// The store failed.
    #[error(transparent)]
    Store(#[from] StoreError),
}

/// Why a store operation failed.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum StoreError {
    /// A queue projection asserted a different tenant from the session head.
    #[error("wake tenant does not match session authority")]
    WakeTenantMismatch,
    /// The journal is not contiguous. The agent does not fold and does not act.
    #[error("journal gap at {missing}")]
    JournalGap {
        /// The first missing sequence.
        missing: JournalSeq,
    },
    /// The same sequence was stored under two different hashes. The agent quarantines.
    #[error("journal fork at {seq}: stored {stored}, read {read}")]
    JournalForked {
        /// Where.
        seq: JournalSeq,
        /// What was already folded.
        stored: ContentHash,
        /// What the read returned.
        read: ContentHash,
    },
    /// A stored item could not be decoded into a known shape.
    #[error("item at {location} could not be decoded: {reason}")]
    Undecodable {
        /// Where the bad item lives, as a diagnostic pointer.
        location: String,
        /// A redacted reason.
        reason: String,
    },
    /// A referenced body could not be placed or read.
    #[error("content {reference} is unavailable: {reason}")]
    ContentUnavailable {
        /// The content key.
        reference: String,
        /// A redacted reason.
        reason: String,
    },
    /// The page budget was exhausted before the read could complete.
    #[error("read budget exhausted after {entries} entries and {bytes} bytes")]
    ReadBudgetExhausted {
        /// How many entries were read.
        entries: usize,
        /// How many bytes were read.
        bytes: usize,
    },
    /// The whole activation restore ceiling was exhausted.
    #[error(
        "restore budget exhausted after {entries} entries and {bytes} bytes (limits: {max_entries} entries, {max_bytes} bytes)"
    )]
    RestoreBudgetExhausted {
        /// How many entries would be retained.
        entries: usize,
        /// How many bytes would be retained.
        bytes: usize,
        /// The activation-wide entry ceiling.
        max_entries: usize,
        /// The activation-wide byte ceiling.
        max_bytes: usize,
    },
    /// The completely folded journal did not equal the tail claimed with the lease.
    #[error(
        "folded journal tail {folded_seq:?}/{folded_hash:?} does not match claimed tail {claimed_seq:?}/{claimed_hash:?}"
    )]
    JournalTailMismatch {
        /// The sequence claimed by the agent head.
        claimed_seq: Option<JournalSeq>,
        /// The hash claimed by the agent head.
        claimed_hash: Option<ContentHash>,
        /// The sequence reached by the complete fold.
        folded_seq: Option<JournalSeq>,
        /// The hash reached by the complete fold.
        folded_hash: Option<ContentHash>,
    },
    /// The underlying service failed.
    #[error("store transport failed: {reason}")]
    Transport {
        /// A redacted reason.
        reason: String,
        /// Whether retrying inside the deadline could help.
        retryable: bool,
    },
}
