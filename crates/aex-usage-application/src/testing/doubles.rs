//! In-memory doubles for the four usage ports.
//!
//! Each double implements one port exactly, including its failure modes. They
//! are deliberately faithful about the two conditions that decide correctness —
//! the admission identity fence and the projection coverage fence — because a
//! double that always succeeds would make every convergence property vacuous.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use aex_usage_domain::fact::{FactDraft, UsageFact};
use aex_usage_domain::frontier::{AcceptedSequence, Frontier, PoisonReason};
use aex_usage_domain::identity::FactId;
use aex_usage_domain::keys::ItemValue;
use aex_usage_domain::meter::Category;
use aex_usage_domain::projection::Generation;
use aex_usage_domain::wire_pending::{Timestamp, WorkspaceId};
use async_trait::async_trait;

use super::{at, region};
use crate::outbox::OutboxMessage;
use crate::ports::{
    Admission, Applied, AuthorityStore, Clock, CoverageAdvance, FactLocation, OutboxEntry,
    PortError, ProjectionStore, RatingQueue, ReceiptOutcome, SettlementReceipt,
};
use crate::projection::{
    ProjectionCondition, ProjectionRow, ProjectionTransaction, ProjectionWrite, Sign,
};

/// A recorded quarantine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Parked {
    /// The workspace whose frontier was parked.
    pub workspace: WorkspaceId,
    /// Where it parked.
    pub sequence: AcceptedSequence,
    /// Why.
    pub reason: PoisonReason,
    /// The bounded detail: identifiers only, never a quantity or evidence.
    pub detail: String,
}

/// The mutable half of the authority double.
#[derive(Debug)]
struct AuthorityState {
    category: Category,
    facts: Mutex<BTreeMap<(String, u64), UsageFact>>,
    claims: Mutex<BTreeMap<String, FactLocation>>,
    frontiers: Mutex<BTreeMap<String, Frontier>>,
    outbox: Mutex<BTreeMap<(String, u64), OutboxEntry>>,
    receipts: Mutex<BTreeMap<(String, u64), SettlementReceipt>>,
    parked: Mutex<Vec<Parked>>,
    unavailable: AtomicBool,
}

/// An in-memory authority table for one category.
#[derive(Debug, Clone)]
pub struct FakeAuthority(Arc<AuthorityState>);

impl FakeAuthority {
    /// An empty authority bound to one category.
    #[must_use]
    pub fn new(category: Category) -> Self {
        Self(Arc::new(AuthorityState {
            category,
            facts: Mutex::new(BTreeMap::new()),
            claims: Mutex::new(BTreeMap::new()),
            frontiers: Mutex::new(BTreeMap::new()),
            outbox: Mutex::new(BTreeMap::new()),
            receipts: Mutex::new(BTreeMap::new()),
            parked: Mutex::new(Vec::new()),
            unavailable: AtomicBool::new(false),
        }))
    }

    /// Seeds an already-admitted fact, as a stream record implies.
    pub fn seed(&self, fact: &UsageFact) {
        let key = (fact.workspace.to_string(), fact.accepted_sequence.get());
        self.0
            .facts
            .lock()
            .expect("facts")
            .insert(key, fact.clone());
        self.0.claims.lock().expect("claims").insert(
            fact.fact_id.to_string(),
            FactLocation {
                workspace: fact.workspace.clone(),
                sequence: fact.accepted_sequence,
            },
        );
        {
            let mut frontiers = self.0.frontiers.lock().expect("frontiers");
            let frontier = frontiers
                .entry(fact.workspace.to_string())
                .or_insert_with(|| {
                    Frontier::empty(fact.region.clone(), fact.workspace.clone(), fact.category())
                });
            if fact.accepted_sequence.get() == frontier.accepted.get() + 1 {
                *frontier = frontier
                    .admit(fact.accepted_sequence)
                    .expect("a seeded fact is contiguous");
            }
        }
        self.0.outbox.lock().expect("outbox").insert(
            (fact.workspace.to_string(), fact.accepted_sequence.get()),
            OutboxEntry {
                workspace: fact.workspace.clone(),
                organization: fact.organization.clone(),
                region: fact.region.clone(),
                category: fact.category(),
                fact_id: fact.fact_id.clone(),
                accepted_sequence: fact.accepted_sequence,
                enqueued_at: fact.accepted_at,
                attempts: 0,
            },
        );
    }

    /// Every quarantine recorded so far.
    #[must_use]
    pub fn parked(&self) -> Vec<Parked> {
        self.0.parked.lock().expect("parked").clone()
    }

    /// How many undelivered outbox rows remain.
    #[must_use]
    pub fn outbox_depth(&self) -> usize {
        self.0.outbox.lock().expect("outbox").len()
    }

    /// The frontier of one workspace, or an empty one.
    #[must_use]
    pub fn frontier_of(&self, workspace: &WorkspaceId) -> Frontier {
        self.0
            .frontiers
            .lock()
            .expect("frontiers")
            .get(&workspace.to_string())
            .cloned()
            .unwrap_or_else(|| Frontier::empty(region(), workspace.clone(), self.0.category))
    }

    /// Makes every call fail retryably, as a throttled table would.
    pub fn set_unavailable(&self, unavailable: bool) {
        self.0.unavailable.store(unavailable, Ordering::Release);
    }

    fn check(&self) -> Result<(), PortError> {
        if self.0.unavailable.load(Ordering::Acquire) {
            return Err(PortError::Unavailable {
                what: "usage authority",
                reason: "fixture set unavailable".to_owned(),
            });
        }
        Ok(())
    }
}

#[async_trait]
impl AuthorityStore for FakeAuthority {
    fn category(&self) -> Category {
        self.0.category
    }

    async fn admit(&self, draft: &FactDraft, at: Timestamp) -> Result<Admission, PortError> {
        self.check()?;
        let fact_id = draft.fact_id();
        let offered = draft.intent_hash().map_err(|error| PortError::Corrupt {
            what: "fact draft",
            reason: error.to_string(),
        })?;
        let existing = self
            .0
            .claims
            .lock()
            .expect("claims")
            .get(fact_id.as_str())
            .cloned();
        if let Some(location) = existing {
            let stored = self
                .0
                .facts
                .lock()
                .expect("facts")
                .get(&(location.workspace.to_string(), location.sequence.get()))
                .cloned()
                .ok_or(PortError::NotFound {
                    what: "usage_fact",
                    id: fact_id.to_string(),
                })?;
            return Ok(if stored.idempotency.intent_hash == offered {
                Admission::Replayed(Box::new(stored))
            } else {
                Admission::IdentityConflict {
                    fact_id,
                    stored: stored.idempotency.intent_hash,
                    offered,
                }
            });
        }
        let next = self
            .0
            .frontiers
            .lock()
            .expect("frontiers")
            .get(&draft.workspace.to_string())
            .map_or(1, |frontier| frontier.accepted.get() + 1);
        let sequence = AcceptedSequence::new(next).map_err(|error| PortError::Corrupt {
            what: "accepted sequence",
            reason: error.to_string(),
        })?;
        let fact = draft
            .clone()
            .admit(sequence, at)
            .map_err(|error| PortError::Corrupt {
                what: "fact draft",
                reason: error.to_string(),
            })?;
        self.seed(&fact);
        Ok(Admission::Admitted(Box::new(fact)))
    }

    async fn frontier(&self, workspace: &WorkspaceId) -> Result<Frontier, PortError> {
        self.check()?;
        Ok(self.frontier_of(workspace))
    }

    async fn advance_frontier(&self, from: &Frontier, to: &Frontier) -> Result<(), PortError> {
        self.check()?;
        let mut frontiers = self.0.frontiers.lock().expect("frontiers");
        let key = to.workspace.to_string();
        if frontiers.get(&key).is_some_and(|current| current != from) {
            return Err(PortError::Conflict { what: "frontier" });
        }
        frontiers.insert(key, to.clone());
        Ok(())
    }

    async fn fact_at(
        &self,
        workspace: &WorkspaceId,
        sequence: AcceptedSequence,
    ) -> Result<Option<UsageFact>, PortError> {
        self.check()?;
        Ok(self
            .0
            .facts
            .lock()
            .expect("facts")
            .get(&(workspace.to_string(), sequence.get()))
            .cloned())
    }

    async fn locate(&self, fact: &FactId) -> Result<Option<FactLocation>, PortError> {
        self.check()?;
        Ok(self
            .0
            .claims
            .lock()
            .expect("claims")
            .get(fact.as_str())
            .cloned())
    }

    async fn quarantine(
        &self,
        workspace: &WorkspaceId,
        sequence: AcceptedSequence,
        reason: PoisonReason,
        detail: &str,
    ) -> Result<(), PortError> {
        self.check()?;
        self.0.parked.lock().expect("parked").push(Parked {
            workspace: workspace.clone(),
            sequence,
            reason,
            detail: detail.to_owned(),
        });
        let mut frontiers = self.0.frontiers.lock().expect("frontiers");
        // A real `UpdateItem` creates the row when it is absent, so a workspace
        // whose very first record is poison still parks rather than silently
        // carrying on with no frontier at all.
        let frontier = frontiers
            .entry(workspace.to_string())
            .or_insert_with(|| Frontier::empty(region(), workspace.clone(), self.0.category));
        *frontier = frontier.quarantine(sequence, reason);
        Ok(())
    }

    async fn record_receipt(
        &self,
        receipt: &SettlementReceipt,
        location: &FactLocation,
    ) -> Result<ReceiptOutcome, PortError> {
        self.check()?;
        let key = (location.workspace.to_string(), location.sequence.get());
        let mut receipts = self.0.receipts.lock().expect("receipts");
        if receipts.contains_key(&key) {
            return Ok(ReceiptOutcome::AlreadyRecorded);
        }
        receipts.insert(key, receipt.clone());
        Ok(ReceiptOutcome::Recorded)
    }

    async fn has_receipt(
        &self,
        workspace: &WorkspaceId,
        sequence: AcceptedSequence,
    ) -> Result<bool, PortError> {
        self.check()?;
        Ok(self
            .0
            .receipts
            .lock()
            .expect("receipts")
            .contains_key(&(workspace.to_string(), sequence.get())))
    }

    async fn discard_outbox(
        &self,
        workspace: &WorkspaceId,
        sequence: AcceptedSequence,
    ) -> Result<(), PortError> {
        self.check()?;
        self.0
            .outbox
            .lock()
            .expect("outbox")
            .remove(&(workspace.to_string(), sequence.get()));
        Ok(())
    }

    async fn due_outbox(
        &self,
        shard: u8,
        older_than: Timestamp,
        limit: usize,
    ) -> Result<Vec<OutboxEntry>, PortError> {
        self.check()?;
        let outbox = self.0.outbox.lock().expect("outbox");
        Ok(outbox
            .values()
            .filter(|entry| shard_of(&entry.workspace) == shard)
            .filter(|entry| entry.enqueued_at.unix_millis() <= older_than.unix_millis())
            .take(limit)
            .cloned()
            .collect())
    }

    async fn note_outbox_attempt(
        &self,
        workspace: &WorkspaceId,
        sequence: AcceptedSequence,
        attempts: u32,
        at: Timestamp,
    ) -> Result<(), PortError> {
        self.check()?;
        if let Some(entry) = self
            .0
            .outbox
            .lock()
            .expect("outbox")
            .get_mut(&(workspace.to_string(), sequence.get()))
        {
            entry.attempts = attempts;
            entry.enqueued_at = at;
        }
        Ok(())
    }
}

/// The fixture shard assignment.
///
/// The adapters use `xxh3_64(workspace) % 16`; the fixture only needs a stable
/// spread, so it uses a byte sum. Nothing depends on the two agreeing — the
/// shard decides which sweep page a row appears on, never where it is stored.
#[must_use]
pub fn shard_of(workspace: &WorkspaceId) -> u8 {
    let sum: u32 = workspace
        .as_str()
        .as_bytes()
        .iter()
        .map(|byte| u32::from(*byte))
        .sum();
    u8::try_from(sum % 16).unwrap_or(0)
}

/// One stored projection row.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProjectionRowState {
    /// The accumulated counters.
    pub counters: BTreeMap<String, i128>,
    /// The last-writer values.
    pub values: BTreeMap<String, ItemValue>,
}

/// The mutable half of the projection double.
#[derive(Debug)]
struct ProjectionState {
    rows: Mutex<BTreeMap<(String, String), ProjectionRowState>>,
    generation: Mutex<Generation>,
    applied: AtomicU64,
    unavailable: AtomicBool,
}

/// An in-memory `usage-query-projection`.
#[derive(Debug, Clone)]
pub struct FakeProjection(Arc<ProjectionState>);

impl Default for FakeProjection {
    fn default() -> Self {
        Self::new()
    }
}

impl FakeProjection {
    /// An empty projection at the first generation.
    #[must_use]
    pub fn new() -> Self {
        Self(Arc::new(ProjectionState {
            rows: Mutex::new(BTreeMap::new()),
            generation: Mutex::new(Generation::FIRST),
            applied: AtomicU64::new(0),
            unavailable: AtomicBool::new(false),
        }))
    }

    /// Points customer reads at another generation.
    pub fn set_generation(&self, generation: Generation) {
        *self.0.generation.lock().expect("generation") = generation;
    }

    /// Makes every call fail retryably.
    pub fn set_unavailable(&self, unavailable: bool) {
        self.0.unavailable.store(unavailable, Ordering::Release);
    }

    /// How many transactions committed.
    #[must_use]
    pub fn applied_total(&self) -> u64 {
        self.0.applied.load(Ordering::Relaxed)
    }

    /// One row's state.
    #[must_use]
    pub fn row(&self, partition: &str, sort: &str) -> Option<ProjectionRowState> {
        self.0
            .rows
            .lock()
            .expect("rows")
            .get(&(partition.to_owned(), sort.to_owned()))
            .cloned()
    }

    /// Every stored row, so a replay can be compared byte for byte.
    #[must_use]
    pub fn snapshot(&self) -> BTreeMap<(String, String), ProjectionRowState> {
        self.0.rows.lock().expect("rows").clone()
    }

    /// The sum of every hourly rollup's quantity.
    #[must_use]
    pub fn hourly_total(&self) -> i128 {
        self.0
            .rows
            .lock()
            .expect("rows")
            .iter()
            .filter(|(key, _)| key.1.starts_with("H#"))
            .filter_map(|(_, row)| row.counters.get("quantity").copied())
            .sum()
    }

    /// The sum of every daily rollup's quantity.
    #[must_use]
    pub fn daily_total(&self) -> i128 {
        self.0
            .rows
            .lock()
            .expect("rows")
            .iter()
            .filter(|(key, _)| key.1.starts_with("D#"))
            .filter_map(|(_, row)| row.counters.get("quantity").copied())
            .sum()
    }

    /// How many detail rows are still active.
    #[must_use]
    pub fn active_details(&self) -> usize {
        self.0
            .rows
            .lock()
            .expect("rows")
            .iter()
            .filter(|(key, _)| key.1.starts_with("X#"))
            .filter(|(_, row)| row.values.get("active") == Some(&ItemValue::Bool(true)))
            .count()
    }

    /// How many detail rows exist at all, active or withdrawn.
    #[must_use]
    pub fn detail_rows(&self) -> usize {
        self.0
            .rows
            .lock()
            .expect("rows")
            .keys()
            .filter(|key| key.1.starts_with("X#"))
            .count()
    }

    fn satisfied(
        rows: &BTreeMap<(String, String), ProjectionRowState>,
        write: &ProjectionWrite,
    ) -> bool {
        let key = (
            write.key.pk.clone(),
            write.key.sk.clone().unwrap_or_default(),
        );
        let existing = rows.get(&key);
        match &write.condition {
            ProjectionCondition::WriteOnce => existing.is_none(),
            ProjectionCondition::Exists => existing.is_some(),
            ProjectionCondition::CategoryMatches { category } => existing.is_none_or(|row| {
                row.values
                    .get("publicCategory")
                    .is_none_or(|value| value == &ItemValue::text(category.id()))
            }),
            ProjectionCondition::ProjectedSequenceIs { previous } => existing.is_none_or(|row| {
                row.values.get("projectedSequence") == Some(&ItemValue::number(previous.get()))
                    || (previous.get() == 0 && !row.values.contains_key("projectedSequence"))
            }),
        }
    }

    fn unavailable(&self) -> Result<(), PortError> {
        if self.0.unavailable.load(Ordering::Acquire) {
            return Err(PortError::Unavailable {
                what: "usage projection",
                reason: "fixture set unavailable".to_owned(),
            });
        }
        Ok(())
    }
}

#[async_trait]
impl ProjectionStore for FakeProjection {
    async fn current_generation(&self) -> Result<Generation, PortError> {
        self.unavailable()?;
        Ok(*self.0.generation.lock().expect("generation"))
    }

    async fn apply(&self, transaction: &ProjectionTransaction) -> Result<Applied, PortError> {
        self.unavailable()?;
        let mut rows = self.0.rows.lock().expect("rows");
        // All or nothing, exactly as `TransactWriteItems` is. A refused
        // condition anywhere means nothing is written, so a redelivered fact
        // cannot half-apply.
        let refused = transaction
            .writes
            .iter()
            .any(|write| !Self::satisfied(&rows, write));
        if refused {
            return Ok(Applied::AlreadyCovered);
        }
        for write in &transaction.writes {
            let key = (
                write.key.pk.clone(),
                write.key.sk.clone().unwrap_or_default(),
            );
            let row = rows.entry(key).or_default();
            for (attribute, delta) in &write.add {
                let magnitude = i128::try_from(delta.magnitude).unwrap_or(i128::MAX);
                let signed = match delta.sign {
                    Sign::Add => magnitude,
                    Sign::Subtract => -magnitude,
                };
                *row.counters.entry(attribute.clone()).or_default() += signed;
            }
            for (attribute, value) in &write.set {
                row.values.insert(attribute.clone(), value.clone());
            }
            debug_assert!(
                write.row != ProjectionRow::Coverage || row.values.contains_key("state"),
                "a coverage row always declares whether it is advancing"
            );
        }
        self.0.applied.fetch_add(1, Ordering::Relaxed);
        Ok(Applied::Committed)
    }

    async fn advance_coverage(&self, advance: &CoverageAdvance) -> Result<(), PortError> {
        self.unavailable()?;
        let key = (
            format!(
                "{}#{}#{}",
                advance.generation.prefix(),
                advance.workspace,
                advance.public_category.id()
            ),
            "COVERAGE".to_owned(),
        );
        let mut rows = self.0.rows.lock().expect("rows");
        let row = rows.entry(key).or_default();
        row.values.insert(
            "settledSequence".to_owned(),
            ItemValue::number(advance.settled.get()),
        );
        if let Some(through) = advance.settled_through {
            row.values
                .insert("settledThrough".to_owned(), ItemValue::instant(through));
        }
        Ok(())
    }
}

/// The mutable half of the queue double.
#[derive(Debug, Default)]
struct QueueState {
    delivered: Mutex<Vec<OutboxMessage>>,
    outage: AtomicBool,
}

/// An in-memory central rating queue.
#[derive(Debug, Clone, Default)]
pub struct FakeQueue(Arc<QueueState>);

impl FakeQueue {
    /// An empty queue.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Simulates a central outage: every send fails retryably.
    pub fn set_outage(&self, outage: bool) {
        self.0.outage.store(outage, Ordering::Release);
    }

    /// Every message delivered so far, in delivery order.
    #[must_use]
    pub fn delivered(&self) -> Vec<OutboxMessage> {
        self.0.delivered.lock().expect("delivered").clone()
    }

    /// The distinct dedupe identities delivered, which is what central collapses
    /// redeliveries onto.
    #[must_use]
    pub fn distinct_identities(&self) -> Vec<String> {
        let mut identities: Vec<String> = self
            .delivered()
            .into_iter()
            .map(|message| message.message_deduplication_id)
            .collect();
        identities.sort();
        identities.dedup();
        identities
    }
}

#[async_trait]
impl RatingQueue for FakeQueue {
    async fn publish(&self, message: &OutboxMessage) -> Result<(), PortError> {
        if self.0.outage.load(Ordering::Acquire) {
            return Err(PortError::Unavailable {
                what: "usage rating queue",
                reason: "central is unreachable".to_owned(),
            });
        }
        self.0
            .delivered
            .lock()
            .expect("delivered")
            .push(message.clone());
        Ok(())
    }
}

/// A clock frozen at one instant, movable only on purpose.
#[derive(Debug, Clone)]
pub struct FrozenClock(Arc<AtomicU64>);

impl FrozenClock {
    /// Freezes the clock at `millis` since the Unix epoch.
    #[must_use]
    pub fn new(millis: u64) -> Self {
        Self(Arc::new(AtomicU64::new(millis)))
    }

    /// Moves the clock forward.
    pub fn advance(&self, millis: u64) {
        self.0.fetch_add(millis, Ordering::AcqRel);
    }
}

impl Clock for FrozenClock {
    fn now(&self) -> Timestamp {
        at(i64::try_from(self.0.load(Ordering::Acquire)).unwrap_or(i64::MAX))
    }
}
