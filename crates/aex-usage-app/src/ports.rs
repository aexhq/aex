//! The ports the usage use cases drive.
//!
//! Every port is the narrowest surface its use case needs. Two consequences are
//! deliberate: an in-memory double is small enough that the fault matrix can be
//! asserted exhaustively without an engine, and no use case can reach a table
//! operation its own design does not name — a worker cannot delete a fact
//! because no port offers it.
//!
//! [`PortError`] separates *retryable* from *terminal* at the type level. That
//! distinction is the whole poison story: a terminal failure must quarantine and
//! park a frontier, and a retryable one must not. Collapsing them into one
//! "error" is how the previous implementation turned a permanent decode failure
//! into an infinite retry loop.

use std::fmt::Debug;

use aex_usage_domain::fact::{FactDraft, UsageFact};
use aex_usage_domain::frontier::{AcceptedSequence, Frontier, PoisonReason};
use aex_usage_domain::identity::FactId;
use aex_usage_domain::intent::IntentHash;
use aex_usage_domain::meter::{Category, PublicCategory};
use aex_usage_domain::projection::Generation;
use aex_usage_domain::wire_pending::{
    OrganizationId, PricingVersion, RegionId, Timestamp, WorkspaceId,
};
use async_trait::async_trait;

use crate::outbox::OutboxMessage;
use crate::projection::ProjectionTransaction;

/// Why a port call did not succeed.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PortError {
    /// A conditional write lost its race and the caller must re-read.
    ///
    /// Retrying the same write blindly would either double-apply or overwrite
    /// whatever won, so this is never retried in place.
    #[error("`{what}` lost its condition; the caller must re-read and re-decide")]
    Conflict {
        /// Which write.
        what: &'static str,
    },
    /// The commit outcome is unknown.
    ///
    /// Typed rather than folded into a retry, because a blind retry of an
    /// unknown commit is how a duplicate settlement is produced (`U-29`).
    #[error("`{what}` returned an unknown commit outcome: {reason}")]
    CommitAmbiguous {
        /// Which write.
        what: &'static str,
        /// What the remote reported.
        reason: String,
    },
    /// The remote was unavailable. Retryable.
    #[error("`{what}` is unavailable: {reason}")]
    Unavailable {
        /// Which dependency.
        what: &'static str,
        /// What it reported.
        reason: String,
    },
    /// A stored row could not be decoded. Never retryable.
    #[error("`{what}` is undecodable: {reason}")]
    Corrupt {
        /// Which row.
        what: &'static str,
        /// Why it was refused.
        reason: String,
    },
    /// A referenced row is absent.
    #[error("`{what}` `{id}` does not exist")]
    NotFound {
        /// Which kind of row.
        what: &'static str,
        /// Its identifier.
        id: String,
    },
}

impl PortError {
    /// Whether retrying the same call could ever succeed.
    ///
    /// A conflict and an ambiguous commit are excluded on purpose: both need the
    /// caller to re-read and re-decide, and both become duplicates if retried
    /// blindly.
    #[must_use]
    pub const fn retryable(&self) -> bool {
        matches!(self, Self::Unavailable { .. })
    }

    /// Whether this failure means the record can never be processed.
    #[must_use]
    pub const fn terminal(&self) -> bool {
        matches!(self, Self::Corrupt { .. } | Self::NotFound { .. })
    }
}

/// What admitting one draft produced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Admission {
    /// The fact was newly admitted at the returned sequence.
    Admitted(Box<UsageFact>),
    /// An identical measurement was already admitted; this is the stored row.
    ///
    /// The deterministic identity plus a matching intent hash is what makes a
    /// producer retry converge without any extra state.
    Replayed(Box<UsageFact>),
    /// The same identity was offered with a different measurement.
    ///
    /// Nothing is written. This is a producer defect, not a race: two different
    /// quantities cannot both be true of one physical measurement.
    IdentityConflict {
        /// The contested identity.
        fact_id: FactId,
        /// What the authority already holds.
        stored: IntentHash,
        /// What was offered.
        offered: IntentHash,
    },
}

/// Where a fact lives inside its authority.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FactLocation {
    /// The partition.
    pub workspace: WorkspaceId,
    /// The position in that partition's sequence.
    pub sequence: AcceptedSequence,
}

/// A committed central settlement receipt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SettlementReceipt {
    /// Central's own receipt identity.
    pub receipt_id: String,
    /// The region the fact was measured in.
    pub region: RegionId,
    /// The authority the fact belongs to.
    pub category: Category,
    /// The fact this settles.
    pub fact_id: FactId,
    /// What central rated it at, in micro-USD. Never a floating-point value.
    pub rated_microusd: i128,
    /// The journal transaction the settlement posted under.
    pub transaction_id: String,
    /// The rate book central used, which must equal the fact's pinned version.
    pub pricing_version: PricingVersion,
    /// When central committed.
    pub settled_at: Timestamp,
    /// The workspace, when central carries it. Saves one index lookup.
    pub workspace: Option<WorkspaceId>,
    /// The sequence, when central carries it. Saves one index lookup.
    pub accepted_sequence: Option<AcceptedSequence>,
}

/// What recording a receipt produced.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReceiptOutcome {
    /// The receipt was newly stored.
    Recorded,
    /// An identical receipt was already stored.
    AlreadyRecorded,
}

/// One undelivered outbox row, as the sweep sees it through the due index.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutboxEntry {
    /// The partition the fact lives in.
    pub workspace: WorkspaceId,
    /// The paying account.
    pub organization: OrganizationId,
    /// The region.
    pub region: RegionId,
    /// The authority.
    pub category: Category,
    /// The fact awaiting delivery.
    pub fact_id: FactId,
    /// Its position in the sequence.
    pub accepted_sequence: AcceptedSequence,
    /// When it was first enqueued.
    pub enqueued_at: Timestamp,
    /// How many delivery attempts have been made.
    pub attempts: u32,
}

/// What applying a projection transaction produced.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Applied {
    /// The transaction committed.
    Committed,
    /// The coverage fence rejected it because this sequence is already folded.
    ///
    /// A no-op, not a failure: a redelivered stream record is expected.
    AlreadyCovered,
}

/// The settled half of a coverage vector, copied after a receipt lands.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CoverageAdvance {
    /// The generation being written.
    pub generation: Generation,
    /// The workspace.
    pub workspace: WorkspaceId,
    /// The public category the coverage row is keyed by.
    pub public_category: PublicCategory,
    /// The new settled position.
    pub settled: AcceptedSequence,
    /// How far settlement is complete in service time.
    pub settled_through: Option<Timestamp>,
    /// When the copy was made.
    pub updated_at: Timestamp,
}

/// The authority table one category owns.
#[async_trait]
pub trait AuthorityStore: Debug + Send + Sync {
    /// Which authority this store addresses. Never more than one.
    fn category(&self) -> Category;

    /// Admits one draft in the four-item single-partition transaction.
    ///
    /// # Errors
    ///
    /// Any [`PortError`]; a lost frontier compare-and-set is
    /// [`PortError::Conflict`] and the caller re-reads rather than retrying.
    async fn admit(&self, draft: &FactDraft, at: Timestamp) -> Result<Admission, PortError>;

    /// Reads one workspace's frontier.
    ///
    /// # Errors
    ///
    /// Any [`PortError`].
    async fn frontier(&self, workspace: &WorkspaceId) -> Result<Frontier, PortError>;

    /// Advances a frontier under a compare-and-set on its previous position.
    ///
    /// # Errors
    ///
    /// [`PortError::Conflict`] when the stored frontier moved underneath.
    async fn advance_frontier(&self, from: &Frontier, to: &Frontier) -> Result<(), PortError>;

    /// Reads one fact by position.
    ///
    /// # Errors
    ///
    /// Any [`PortError`].
    async fn fact_at(
        &self,
        workspace: &WorkspaceId,
        sequence: AcceptedSequence,
    ) -> Result<Option<UsageFact>, PortError>;

    /// Resolves a fact identity to its partition through the sparse index.
    ///
    /// # Errors
    ///
    /// Any [`PortError`].
    async fn locate(&self, fact: &FactId) -> Result<Option<FactLocation>, PortError>;

    /// Records a poisoned record and parks the workspace's frontier.
    ///
    /// # Errors
    ///
    /// Any [`PortError`].
    async fn quarantine(
        &self,
        workspace: &WorkspaceId,
        sequence: AcceptedSequence,
        reason: PoisonReason,
        detail: &str,
    ) -> Result<(), PortError>;

    /// Stores a settlement receipt, write-once.
    ///
    /// # Errors
    ///
    /// Any [`PortError`].
    async fn record_receipt(
        &self,
        receipt: &SettlementReceipt,
        location: &FactLocation,
    ) -> Result<ReceiptOutcome, PortError>;

    /// Whether a receipt is already parked at this position.
    ///
    /// # Errors
    ///
    /// Any [`PortError`].
    async fn has_receipt(
        &self,
        workspace: &WorkspaceId,
        sequence: AcceptedSequence,
    ) -> Result<bool, PortError>;

    /// Removes a delivered outbox marker.
    ///
    /// # Errors
    ///
    /// Any [`PortError`].
    async fn discard_outbox(
        &self,
        workspace: &WorkspaceId,
        sequence: AcceptedSequence,
    ) -> Result<(), PortError>;

    /// A bounded ordered page of undelivered outbox rows for one shard.
    ///
    /// # Errors
    ///
    /// Any [`PortError`].
    async fn due_outbox(
        &self,
        shard: u8,
        older_than: Timestamp,
        limit: usize,
    ) -> Result<Vec<OutboxEntry>, PortError>;

    /// Records one more delivery attempt against an outbox row.
    ///
    /// # Errors
    ///
    /// Any [`PortError`].
    async fn note_outbox_attempt(
        &self,
        workspace: &WorkspaceId,
        sequence: AcceptedSequence,
        attempts: u32,
        at: Timestamp,
    ) -> Result<(), PortError>;
}

/// The shared, rebuildable query projection.
#[async_trait]
pub trait ProjectionStore: Debug + Send + Sync {
    /// The generation customer reads are currently served from.
    ///
    /// # Errors
    ///
    /// Any [`PortError`].
    async fn current_generation(&self) -> Result<Generation, PortError>;

    /// Applies one folded transaction atomically.
    ///
    /// # Errors
    ///
    /// Any [`PortError`]. A rejected coverage fence is
    /// [`Applied::AlreadyCovered`], not an error: a redelivered stream record is
    /// an expected condition, not a fault.
    async fn apply(&self, transaction: &ProjectionTransaction) -> Result<Applied, PortError>;

    /// Copies the settled half of a frontier into the coverage row.
    ///
    /// # Errors
    ///
    /// Any [`PortError`].
    async fn advance_coverage(&self, advance: &CoverageAdvance) -> Result<(), PortError>;
}

/// The central settlement FIFO queue.
#[async_trait]
pub trait RatingQueue: Debug + Send + Sync {
    /// Delivers one rating request.
    ///
    /// # Errors
    ///
    /// [`PortError::Unavailable`] during a central outage, which must accumulate
    /// backlog rather than stall admission (`U-18`).
    async fn publish(&self, message: &OutboxMessage) -> Result<(), PortError>;
}

/// The authority's own clock.
///
/// Producer clocks stamp service time; this one stamps admission. Ordering and
/// idempotency never depend on a producer clock.
pub trait Clock: Debug + Send + Sync {
    /// The current instant.
    fn now(&self) -> Timestamp;
}

/// Where a fact draft goes once a probe has produced it.
#[async_trait]
pub trait RecordFactPort: Debug + Send + Sync {
    /// Records a batch of drafts.
    ///
    /// # Errors
    ///
    /// Any [`PortError`]. A partially recorded batch reports the error; the
    /// drain treats anything unrecorded as unflushed and refuses a clean exit.
    async fn record(&self, drafts: &[FactDraft]) -> Result<Vec<Admission>, PortError>;
}
