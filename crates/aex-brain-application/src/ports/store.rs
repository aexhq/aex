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
use aex_wire::ids::{OrganizationId, WorkspaceId};

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
    /// The commit counter.
    pub revision: AgentRevision,
    /// The ownership generation.
    pub fence: Fence,
    /// The last committed sequence.
    pub journal_tail: Option<JournalSeq>,
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

/// One page of an agent's journal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JournalPage {
    /// The entries, contiguous and in order.
    pub entries: Vec<JournalEntry>,
    /// The sequence to resume from, when the page did not reach the tail.
    pub next: Option<JournalSeq>,
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
    /// This is a separate durable write, not an optimization to remove. Without it a crash
    /// between `Prepared` and the socket write is indistinguishable from a crash after it,
    /// and every prepared effect becomes ambiguous. It costs one conditional update per
    /// external effect.
    fn mark_dispatch_started<'a>(
        &'a self,
        guard: &'a FenceGuard,
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
    /// [`ReleaseDisposition::Drain`] sets the expiry to zero so a surviving task claims
    /// immediately instead of waiting out the whole TTL.
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
    /// The agent is terminal; there is nothing to claim.
    #[error("agent is terminal")]
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
    /// The process is draining. The lease expiry is set to zero.
    Abandoned,
    /// The activation gave up without committing.
    Drain,
}

/// One wake as the queue delivered it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WakeDelivery {
    /// The durable wake.
    pub wake: DurableWake,
    /// The transport's receipt handle, for acking and visibility changes.
    pub receipt: String,
    /// How many times this message has been received. Used by the poison policy.
    pub receive_count: u32,
}

/// A durable wake item.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DurableWake {
    /// The wake identity.
    pub id: WakeId,
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

/// Wake **delivery**. Creation lives in [`DecisionCommit`] and nowhere else.
pub trait WakeQueue: Send + Sync + 'static {
    /// Receives up to `max` deliveries, long-polling for `wait`.
    fn receive(
        &self,
        max: usize,
        wait: core::time::Duration,
    ) -> BoxFuture<'_, Result<Vec<WakeDelivery>, StoreError>>;

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
    ) -> BoxFuture<'_, Result<Vec<DurableWake>, StoreError>>;
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
    /// The underlying service failed.
    #[error("store transport failed: {reason}")]
    Transport {
        /// A redacted reason.
        reason: String,
        /// Whether retrying inside the deadline could help.
        retryable: bool,
    },
}
