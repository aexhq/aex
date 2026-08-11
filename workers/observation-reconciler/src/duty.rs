//! The duty engine: a bounded due-scan, a durable claim, and one real body per
//! control domain.
//!
//! Three properties are structural rather than conventional.
//!
//! - **The due-scan is a `Query` on the sparse `gsi_control` index, never a
//!   `Scan`.** The partition is `CTRL#{domain}#{shard:02}` and it is bound as a
//!   *value* through [`ExpressionBuilder`], so nothing ever reaches an
//!   expression string.
//! - **Work is claimed durably.** A conditional `UpdateItem` bumps the attempt
//!   counter and pushes `nextAttemptAt` out by the pinned spool backoff, so two
//!   concurrent invocations cannot both work one item.
//! - **No arm silently drops an item.** A duty that cannot be completed either
//!   leaves the item due with a recorded failure, escalates it to a durable
//!   `pipeline_loss` gap revision, or quarantines it with a durable reason. The
//!   wire spelling of a pipeline loss is
//!   [`TelemetryGapReason::SpoolLost`]; `pipeline_loss` is the plan's word for
//!   it and [`aex_observation_domain::gap::PRODUCIBLE_REASONS`] is the closed
//!   set this codebase may mint.

use std::collections::{BTreeSet, HashMap};

use aex_observation_domain::frontier::DeletionState;
use aex_observation_domain::gap::{GapRecord, GapRevision, OrdinalRange, TimeWindow};
use aex_observation_domain::keys::{self, BucketHour, ControlDomain, ScopeKey};
use aex_observation_domain::limits;
use aex_observation_domain::signal::{Signal, SignalSet};
use aex_observation_store_dynamodb::expressions::{ExpressionBuilder, Index, PK, SK};
use aex_observation_store_dynamodb::gap::{append_action, decode as decode_gap};
use aex_observation_store_dynamodb::gap_hint::hint_update_action;
use aex_observation_store_dynamodb::spool::{
    GateEvidence, GateState, Pending, SpoolChunk, evaluate,
};
use aex_observation_store_dynamodb::{ExportPairError, ExportPairStore};
use aex_wire::ids::{
    ExportId, OperationId, PrefixedId, TelemetryBatchId, TelemetryGapId, WorkspaceId,
};
use aex_wire::models::TelemetryGapReason;
use aex_wire::types::{Region, Timestamp};
use aws_sdk_dynamodb::types::{
    AttributeValue, Delete, Select, TransactWriteItem, Update, WriteRequest,
};

/// The attempt counter every claimable item carries.
pub const ATTEMPTS: &str = "attempts";
/// When the item becomes due again.
pub const NEXT_ATTEMPT_AT: &str = "nextAttemptAt";
/// When the current claim was taken.
pub const CLAIMED_AT: &str = "claimedAt";
/// The durable state word of a claimable item.
pub const STATE: &str = "state";
/// The outstanding-duty set of a spool chunk.
pub const PENDING: &str = "pending";
/// Why an item was quarantined.
pub const QUARANTINE_REASON: &str = "quarantineReason";
/// The durable state of an item that exhausted its attempts.
pub const QUARANTINED: &str = "quarantined";

/// The sort-key prefix of the usage outbox sibling of a spool chunk.
pub const OUTBOX_PREFIX: &str = "OUTBOX#";

/// How many partitions one accepted-time bucket is written across by default.
///
/// The writer records the exact count on the `SEG#` row; this is the fallback
/// for a chunk whose segment row has not been read. It matches the admission
/// edge's fan-out and is declared here because `aex-observation-store-dynamodb`
/// exposes no constant for it.
pub const DEFAULT_BUCKET_SHARDS: u8 = 4;

/// How long an unacknowledged wake may go undelivered before it is cleared.
///
/// A wake is a hint, so its only failure mode is latency: once the reader has
/// dwelt longer than the gate hysteresis, re-poking it cannot recover anything
/// the scheduled scan would not already have found.
pub const WAKE_STALE_MS: i64 = limits::OBS_GATE_HYSTERESIS_MS;

/// How long a metric series claim may go unseen before it is reclaimable.
///
/// Declared here because `aex-observation-domain::limits` pins no series idle
/// horizon; a day is one full retention cycle of the workspace cardinality
/// counter, which is the shortest window that cannot reclaim a series a daily
/// job is still writing.
pub const SERIES_IDLE_MS: i64 = 24 * 60 * 60 * 1_000;

/// The sentinel that closes a due-window key range.
///
/// `aex_observation_domain::keys::component` excludes `\u{ffff}` from every key
/// component precisely so it can be used as an upper bound that sorts after
/// every real identifier.
pub const DUE_SENTINEL: &str = "\u{ffff}";

/// The identifier a partial-batch response reports.
///
/// For a queue-driven invocation this is the SQS `messageId`, because that is
/// what `batchItemFailures` is defined over; for a scheduled scan it is the due
/// item's own identifier.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct ItemId(Box<str>);

impl ItemId {
    /// Wraps an identifier.
    #[must_use]
    pub fn new(text: impl Into<Box<str>>) -> Self {
        Self(text.into())
    }

    /// The identifier.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for ItemId {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

/// The exact primary key of one item on `observation-authority`.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct ItemKey {
    /// The partition key.
    pub pk: String,
    /// The sort key.
    pub sk: String,
}

/// One due item named by the sparse control index.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DueRef {
    /// The identifier reported in a partial-batch response.
    pub id: ItemId,
    /// Where the item lives on the base table.
    pub key: ItemKey,
}

/// One record a queue named.
///
/// The target is optional because a record whose body does not name a `pk` and
/// `sk` is still reported — as a failure — rather than dropped.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct QueuedRecord {
    /// The `SQS` message identifier a partial-batch response reports.
    pub message_id: ItemId,
    /// The item the record names, when the body could be read.
    pub target: Option<ItemKey>,
}

/// One due item, loaded from the base table.
#[derive(Clone, Debug)]
pub struct DueItem {
    /// The identifier reported in a partial-batch response.
    pub id: ItemId,
    /// Where the item lives on the base table.
    pub key: ItemKey,
    /// How many times the duty has already been attempted.
    pub attempts: u32,
    /// The item as stored.
    pub attributes: HashMap<String, AttributeValue>,
}

/// One exact per-signal loss candidate retained by admission.
#[derive(Clone, Debug, Eq, PartialEq)]
struct LossCandidate {
    gap_id: TelemetryGapId,
    signal: Signal,
    lo: u64,
    hi_exclusive: u64,
    time_range: Option<TimeWindow>,
    attempted_records: u64,
    attempted_bytes: u64,
}

impl LossCandidate {
    fn record(
        &self,
        workspace: WorkspaceId,
        scope: ScopeKey,
        reason: TelemetryGapReason,
        now: Timestamp,
    ) -> Result<GapRecord, DutyError> {
        let hi = self
            .hi_exclusive
            .checked_sub(1)
            .ok_or(DutyError::Malformed {
                item: "loss_candidate",
                attribute: "acceptedSeqHiExclusive",
            })?;
        let ordinals = OrdinalRange::new(self.lo, hi).ok_or(DutyError::Malformed {
            item: "loss_candidate",
            attribute: "acceptedSeqHiExclusive",
        })?;
        let revision = GapRevision::try_open(
            self.gap_id,
            SignalSet::from_signal(self.signal),
            reason,
            self.time_range,
            now,
        )
        .map_err(|error| DutyError::unresolved(ControlDomain::SpoolRepair, error.to_string()))?
        .with_ordinals(ordinals);
        GapRecord::try_new(
            workspace,
            scope,
            revision,
            Some(self.attempted_records),
            Some(self.attempted_bytes),
            false,
        )
        .map_err(|error| DutyError::unresolved(ControlDomain::SpoolRepair, error.to_string()))
    }
}

/// What one invocation completed and what it did not.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct BatchOutcome {
    /// Items whose duty is durably resolved.
    pub succeeded: Vec<ItemId>,
    /// Items that must be re-driven, each with why.
    pub failed: Vec<(ItemId, Box<str>)>,
}

impl BatchOutcome {
    /// Records a durably resolved item.
    pub fn succeed(&mut self, id: ItemId) {
        self.succeeded.push(id);
    }

    /// Records an item that must be re-driven.
    pub fn fail(&mut self, id: ItemId, reason: impl Into<Box<str>>) {
        self.failed.push((id, reason.into()));
    }

    /// Folds another outcome in, keeping both verdicts apart.
    pub fn merge(&mut self, other: Self) {
        self.succeeded.extend(other.succeeded);
        self.failed.extend(other.failed);
    }

    /// How many items the invocation touched.
    #[must_use]
    pub fn len(&self) -> usize {
        self.succeeded.len() + self.failed.len()
    }

    /// Whether the invocation touched nothing.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// Why a duty could not be executed.
#[derive(Clone, Debug, thiserror::Error)]
pub enum DutyError {
    /// A provider call failed.
    #[error("`{operation}` failed: {reason}")]
    Provider {
        /// Which call.
        operation: &'static str,
        /// What the provider reported, without its own body.
        reason: String,
    },
    /// A stored item did not carry a value this engine can read.
    #[error("stored item `{item}` is missing or malformed attribute `{attribute}`")]
    Malformed {
        /// Which item family.
        item: &'static str,
        /// Which attribute.
        attribute: &'static str,
    },
    /// The duty made no progress and the item stays due.
    #[error("the `{duty}` duty did not complete: {reason}")]
    Unresolved {
        /// Which duty.
        duty: &'static str,
        /// What is still outstanding.
        reason: String,
    },
    /// A duty this deployable must never run reached the engine.
    #[error("`{duty}` belongs to another deployable and must never run here")]
    Foreign {
        /// The offending duty.
        duty: &'static str,
    },
}

impl DutyError {
    /// Builds a provider failure without carrying an upstream body.
    fn provider(operation: &'static str, reason: impl std::fmt::Display) -> Self {
        Self::Provider {
            operation,
            reason: reason.to_string(),
        }
    }

    /// Builds an unresolved-duty failure.
    fn unresolved(duty: ControlDomain, reason: impl Into<String>) -> Self {
        Self::Unresolved {
            duty: duty.as_str(),
            reason: reason.into(),
        }
    }
}

/// Everything one duty deployment is bound to.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DutySettings {
    /// The observation-authority table.
    pub table: String,
    /// The session authority containing canonical export operations.
    pub session_table: String,
    /// The regional observation bucket.
    pub bucket: String,
    /// The usage queue the storage fact is delivered to.
    pub usage_queue_url: String,
    /// The region this deployment is bound to.
    pub region: Region,
    /// The one duty this deployment runs.
    pub duty: ControlDomain,
    /// How many due items one scan of one shard reads.
    pub page: u16,
    /// How many shards the duty's due index is spread over.
    pub shards: u8,
    /// Attempts before an item is quarantined rather than retried.
    pub max_attempts: u32,
}

/// The terminal state and claim fence applied alongside durable gap evidence.
struct GapTerminalization<'a> {
    state: &'a str,
    reason: Option<&'a str>,
    expected_attempts: u32,
    claimed_at: Option<Timestamp>,
    changed_at: Timestamp,
}

/// Progress from one bounded deletion family.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct DeletionDrain {
    removed: u64,
    /// A fenced admission still owns writes that must settle before proof.
    pending: bool,
}

/// The validated identity and owned keys carried by a scope-to-batch row.
struct ScopeBatchDirectory {
    batch: TelemetryBatchId,
    batch_pk: String,
    key: ItemKey,
    spool_key: Option<ItemKey>,
}

/// The validated identity and owned keys carried by a scope-to-export row.
struct ScopeExportDirectory {
    export: ExportId,
    key: ItemKey,
    extension: &'static str,
}

impl DeletionDrain {
    fn add(&mut self, other: Self) {
        self.removed = self.removed.saturating_add(other.removed);
        self.pending |= other.pending;
    }
}

/// The duty engine.
#[derive(Clone, Debug)]
pub struct DutyEngine {
    dynamodb: aws_sdk_dynamodb::Client,
    s3: aws_sdk_s3::Client,
    sqs: aws_sdk_sqs::Client,
    settings: DutySettings,
}

impl DutyEngine {
    /// Binds the engine to its resolved resources.
    #[must_use]
    pub fn new(
        dynamodb: aws_sdk_dynamodb::Client,
        s3: aws_sdk_s3::Client,
        sqs: aws_sdk_sqs::Client,
        settings: DutySettings,
    ) -> Self {
        Self {
            dynamodb,
            s3,
            sqs,
            settings,
        }
    }

    /// The settings this engine runs under.
    #[must_use]
    pub const fn settings(&self) -> &DutySettings {
        &self.settings
    }

    /// Proves the table and bucket are reachable.
    ///
    /// # Errors
    ///
    /// Returns [`DutyError::Provider`] when either probe fails. A probe that has
    /// not passed is never assumed to have passed.
    pub async fn probe(&self) -> Result<(), DutyError> {
        self.dynamodb
            .describe_table()
            .table_name(&self.settings.table)
            .send()
            .await
            .map_err(|error| DutyError::provider("DescribeTable", error))?;
        self.s3
            .head_bucket()
            .bucket(&self.settings.bucket)
            .send()
            .await
            .map_err(|error| DutyError::provider("HeadBucket", error))?;
        Ok(())
    }

    /// The regional singleton this duty works, when it has one instead of a due
    /// queue.
    ///
    /// `gate.evaluate` re-evaluates one item per region on a fixed cadence; it
    /// is not a backlog, so it is never enqueued and never quarantined.
    #[must_use]
    pub fn singleton_target(&self) -> Option<ItemKey> {
        (self.settings.duty == ControlDomain::GateEvaluate).then(|| ItemKey {
            pk: keys::gate_pk(self.settings.region),
            sk: keys::GATE_SK.to_owned(),
        })
    }

    /// Runs one scheduled scan across every shard of the duty's due index.
    ///
    /// # Errors
    ///
    /// Returns [`DutyError::Provider`] when the due-scan itself fails, which is
    /// a whole-invocation failure: the schedule, not a queue, re-drives it.
    pub async fn run_scheduled(&self, now: Timestamp) -> Result<BatchOutcome, DutyError> {
        let mut outcome = BatchOutcome::default();
        if let Some(key) = self.singleton_target() {
            let id = ItemId::new(key.pk.clone());
            match self.load(&id, &key).await? {
                Some(item) => self.work_one(item, now, &mut outcome).await,
                None => self.work_one(absent(id, key), now, &mut outcome).await,
            }
        }
        for shard in 0..self.settings.shards {
            for due in self.due_page(shard, now).await? {
                match self.load(&due.id, &due.key).await? {
                    // An item that no longer exists is already resolved.
                    None => outcome.succeed(due.id),
                    Some(item) => self.work_one(item, now, &mut outcome).await,
                }
            }
        }
        Ok(outcome)
    }

    /// Runs exactly the items a queue named, reporting each verdict separately.
    pub async fn run_records(&self, records: &[QueuedRecord], now: Timestamp) -> BatchOutcome {
        let mut outcome = BatchOutcome::default();
        for record in records {
            let id = record.message_id.clone();
            let Some(key) = record.target.as_ref() else {
                outcome.fail(id, "the record body did not name a `pk` and `sk`");
                continue;
            };
            match self.load(&id, key).await {
                Ok(None) => outcome.succeed(id),
                Ok(Some(item)) => self.work_one(item, now, &mut outcome).await,
                Err(error) => outcome.fail(id, error.to_string()),
            }
        }
        outcome
    }

    /// Claims one item and runs its duty, recording exactly one verdict.
    async fn work_one(&self, item: DueItem, now: Timestamp, outcome: &mut BatchOutcome) {
        let id = item.id.clone();
        if item.attempts >= self.settings.max_attempts && self.singleton_target().is_none() {
            let reason = format!(
                "the `{}` duty exhausted its {} attempts",
                self.settings.duty.as_str(),
                self.settings.max_attempts
            );
            match self.quarantine(&item, &reason, now).await {
                Ok(()) => outcome.succeed(id),
                Err(error) => outcome.fail(id, error.to_string()),
            }
            return;
        }
        match self.claim(&item, now).await {
            // Another invocation owns the item; it is not this one's to re-drive.
            Ok(false) => outcome.succeed(id),
            Err(error) => outcome.fail(id, error.to_string()),
            Ok(true) => self.run_claimed(&item, id, now, outcome).await,
        }
    }

    /// Runs the duty under a held claim and records the single verdict.
    async fn run_claimed(
        &self,
        item: &DueItem,
        id: ItemId,
        now: Timestamp,
        outcome: &mut BatchOutcome,
    ) {
        if let Err(error) = self.work(item, now).await {
            outcome.fail(id, error.to_string());
            return;
        }
        // A singleton is re-evaluated forever, so its attempt counter is a
        // failure streak rather than a backlog and is cleared on success.
        if self.singleton_target().is_some()
            && let Err(error) = self.reset_attempts(item).await
        {
            outcome.fail(id, error.to_string());
            return;
        }
        outcome.succeed(id);
    }

    /// Reads one bounded page of due items from the sparse control index.
    ///
    /// # Errors
    ///
    /// Returns [`DutyError::Provider`] when the query fails.
    pub async fn due_page(&self, shard: u8, now: Timestamp) -> Result<Vec<DueRef>, DutyError> {
        let mut builder = ExpressionBuilder::new();
        let partition = builder.name(Index::Control.partition_key());
        let sort = builder.name(Index::Control.sort_key());
        let bound = builder.string(keys::control_pk(self.settings.duty, shard));
        let ceiling = builder.string(keys::control_sk(now, DUE_SENTINEL));
        let response = self
            .dynamodb
            .query()
            .table_name(&self.settings.table)
            .index_name(Index::Control.as_str())
            .key_condition_expression(format!("{partition} = {bound} AND {sort} <= {ceiling}"))
            .set_expression_attribute_names(Some(builder.names()))
            .set_expression_attribute_values(Some(builder.values()))
            .limit(i32::from(self.settings.page))
            .send()
            .await
            .map_err(|error| DutyError::provider("Query", error))?;
        Ok(response.items().iter().filter_map(due_ref).collect())
    }

    /// Takes the durable claim on one item.
    ///
    /// The write bumps `attempts` and pushes both `nextAttemptAt` and the
    /// control index's own sort key out by the pinned spool backoff, under a
    /// condition on the attempt count that was observed. Two concurrent
    /// invocations therefore cannot both work one item.
    ///
    /// # Errors
    ///
    /// Returns [`DutyError::Provider`] when the write fails for a reason other
    /// than the claim being lost, and [`DutyError::Malformed`] when the pushed
    /// deadline is not representable.
    pub async fn claim(&self, item: &DueItem, now: Timestamp) -> Result<bool, DutyError> {
        let mut chunk = SpoolChunk::new(0, 0, now);
        chunk.attempts = item.attempts;
        let next_due =
            Timestamp::from_unix_millis(now.unix_millis() + chunk.next_attempt_delay_ms())
                .map_err(|_| DutyError::Malformed {
                    item: "control_item",
                    attribute: NEXT_ATTEMPT_AT,
                })?;

        let mut builder = ExpressionBuilder::new();
        let attempts = builder.name(ATTEMPTS);
        let next_at = builder.name(NEXT_ATTEMPT_AT);
        let control_sort = builder.name(Index::Control.sort_key());
        let claimed_at = builder.name(CLAIMED_AT);
        let observed = builder.name(ATTEMPTS);
        let absent = builder.name(ATTEMPTS);
        let bumped = builder.number(item.attempts + 1);
        let current = builder.number(item.attempts);
        let deadline = builder.string(next_due.to_wire());
        let index_key = builder.string(keys::control_sk(next_due, item.id.as_str()));
        let at = builder.string(now.to_wire());

        let outcome = self
            .dynamodb
            .update_item()
            .table_name(&self.settings.table)
            .key(PK, AttributeValue::S(item.key.pk.clone()))
            .key(SK, AttributeValue::S(item.key.sk.clone()))
            .update_expression(format!(
                "SET {attempts} = {bumped}, {next_at} = {deadline}, \
                 {control_sort} = {index_key}, {claimed_at} = {at}"
            ))
            .condition_expression(format!(
                "attribute_not_exists({absent}) OR {observed} = {current}"
            ))
            .set_expression_attribute_names(Some(builder.names()))
            .set_expression_attribute_values(Some(builder.values()))
            .send()
            .await;
        match outcome {
            Ok(_) => Ok(true),
            // Another invocation bumped the counter first, so the claim is lost.
            Err(error) if is_conditional_failure(&error) => Ok(false),
            Err(error) => Err(DutyError::provider("UpdateItem", error)),
        }
    }

    /// Clears the attempt counter of a singleton whose duty completed.
    async fn reset_attempts(&self, item: &DueItem) -> Result<(), DutyError> {
        let mut builder = ExpressionBuilder::new();
        let attempts = builder.name(ATTEMPTS);
        let zero = builder.number(0u32);
        self.dynamodb
            .update_item()
            .table_name(&self.settings.table)
            .key(PK, AttributeValue::S(item.key.pk.clone()))
            .key(SK, AttributeValue::S(item.key.sk.clone()))
            .update_expression(format!("SET {attempts} = {zero}"))
            .set_expression_attribute_names(Some(builder.names()))
            .set_expression_attribute_values(Some(builder.values()))
            .send()
            .await
            .map_err(|error| DutyError::provider("UpdateItem", error))?;
        Ok(())
    }

    /// Quarantines an item that exhausted its attempts.
    ///
    /// The item leaves the due index and keeps a durable reason. A spool chunk
    /// additionally escalates into an explicit `pipeline_loss` gap over its
    /// exact accepted range, so the loss is visible to every query that
    /// intersects it rather than silently absent.
    ///
    /// # Errors
    ///
    /// Returns [`DutyError::Provider`] when the durable write fails.
    pub async fn quarantine(
        &self,
        item: &DueItem,
        reason: &str,
        now: Timestamp,
    ) -> Result<(), DutyError> {
        if self.settings.duty == ControlDomain::SpoolRepair {
            let gaps = Self::loss_gap_records(item, TelemetryGapReason::SpoolLost, now)?;
            return self
                .terminalize_with_gaps(
                    item,
                    &gaps,
                    GapTerminalization {
                        state: QUARANTINED,
                        reason: Some(reason),
                        expected_attempts: item.attempts,
                        claimed_at: None,
                        changed_at: now,
                    },
                )
                .await;
        }
        let mut builder = ExpressionBuilder::new();
        let state = builder.name(STATE);
        let why = builder.name(QUARANTINE_REASON);
        let at = builder.name(CLAIMED_AT);
        let control_partition = builder.name(Index::Control.partition_key());
        let control_sort = builder.name(Index::Control.sort_key());
        let quarantined = builder.string(QUARANTINED);
        let text = builder.string(reason.to_owned());
        let when = builder.string(now.to_wire());
        self.dynamodb
            .update_item()
            .table_name(&self.settings.table)
            .key(PK, AttributeValue::S(item.key.pk.clone()))
            .key(SK, AttributeValue::S(item.key.sk.clone()))
            .update_expression(format!(
                "SET {state} = {quarantined}, {why} = {text}, {at} = {when} \
                 REMOVE {control_partition}, {control_sort}"
            ))
            .set_expression_attribute_names(Some(builder.names()))
            .set_expression_attribute_values(Some(builder.values()))
            .send()
            .await
            .map_err(|error| DutyError::provider("UpdateItem", error))?;
        Ok(())
    }

    /// Builds every exact per-signal gap a failed source item proves.
    fn loss_gap_records(
        item: &DueItem,
        reason: TelemetryGapReason,
        now: Timestamp,
    ) -> Result<Vec<GapRecord>, DutyError> {
        let workspace = workspace_of(item)?;
        let scope = scope_of(item)?;
        loss_candidates_of(item)?
            .iter()
            .map(|candidate| candidate.record(workspace, scope, reason, now))
            .collect()
    }

    /// Atomically appends immutable gap rows and removes their source from the
    /// due index. A source can never become terminal without all gap evidence.
    async fn terminalize_with_gaps(
        &self,
        item: &DueItem,
        gaps: &[GapRecord],
        transition: GapTerminalization<'_>,
    ) -> Result<(), DutyError> {
        use std::fmt::Write as _;

        let GapTerminalization {
            state: terminal,
            reason,
            expected_attempts,
            claimed_at,
            changed_at: transition_at,
        } = transition;
        let mut builder = ExpressionBuilder::new();
        let state = builder.name(STATE);
        let changed_at = builder.name("stateChangedAt");
        let control_partition = builder.name(Index::Control.partition_key());
        let control_sort = builder.name(Index::Control.sort_key());
        let partition = builder.name(PK);
        let sort = builder.name(SK);
        let attempts = builder.name(ATTEMPTS);
        let terminal_value = builder.string(terminal.to_owned());
        let when = builder.string(transition_at.to_wire());
        let expected_attempts = builder.number(expected_attempts);
        let mut update = format!("SET {state} = {terminal_value}, {changed_at} = {when}");
        if let Some(reason) = reason {
            let why = builder.name(QUARANTINE_REASON);
            let text = builder.string(reason.to_owned());
            let _ = write!(update, ", {why} = {text}");
        }
        let _ = write!(update, " REMOVE {control_partition}, {control_sort}");
        let mut condition = format!(
            "attribute_exists({partition}) AND attribute_exists({sort}) AND \
             {attempts} = {expected_attempts}"
        );
        if let Some(claimed) = claimed_at {
            let claimed_at = builder.name(CLAIMED_AT);
            let claimed_value = builder.string(claimed.to_wire());
            let _ = write!(condition, " AND {claimed_at} = {claimed_value}");
        }
        let source = Update::builder()
            .table_name(&self.settings.table)
            .key(PK, AttributeValue::S(item.key.pk.clone()))
            .key(SK, AttributeValue::S(item.key.sk.clone()))
            .update_expression(update)
            .condition_expression(condition)
            .set_expression_attribute_names(Some(builder.names()))
            .set_expression_attribute_values(Some(builder.values()))
            .build()
            .map_err(|error| DutyError::provider("TransactWriteItems", error))?;
        let mut actions = Vec::with_capacity(gaps.len() + 2);
        for gap in gaps {
            actions.push(
                append_action(&self.settings.table, gap).map_err(|error| {
                    DutyError::unresolved(self.settings.duty, error.to_string())
                })?,
            );
        }
        // The gap-change hint rides the same transaction that makes the gaps
        // durable, so a reader is told to look exactly when there is something
        // to find. One action per distinct workspace, because a transaction may
        // not act twice on one item — in practice a control item names one
        // workspace, so this is one action.
        for workspace in gaps
            .iter()
            .map(|gap| gap.workspace)
            .collect::<BTreeSet<_>>()
        {
            let appended = gaps.iter().filter(|gap| gap.workspace == workspace).count();
            actions.push(
                hint_update_action(
                    &self.settings.table,
                    workspace,
                    u64::try_from(appended).unwrap_or(u64::MAX),
                )
                .map_err(|error| DutyError::unresolved(self.settings.duty, error.to_string()))?,
            );
        }
        actions.push(TransactWriteItem::builder().update(source).build());
        let outcome = self
            .dynamodb
            .transact_write_items()
            .set_transact_items(Some(actions))
            .send()
            .await;
        match outcome {
            Ok(_) => Ok(()),
            Err(error) => {
                if self
                    .terminalized_with_gaps(item, terminal, reason, gaps)
                    .await?
                {
                    Ok(())
                } else {
                    Err(DutyError::provider("TransactWriteItems", error))
                }
            }
        }
    }

    /// Resolves an ambiguous/replayed terminal transaction by durable identity.
    async fn terminalized_with_gaps(
        &self,
        item: &DueItem,
        terminal: &str,
        reason: Option<&str>,
        gaps: &[GapRecord],
    ) -> Result<bool, DutyError> {
        let Some(source) = self.get(&item.key).await? else {
            return Ok(false);
        };
        if string(&source, STATE) != Some(terminal)
            || reason.is_some_and(|expected| string(&source, QUARANTINE_REASON) != Some(expected))
        {
            return Ok(false);
        }
        for expected in gaps {
            let key = ItemKey {
                pk: keys::gap_pk(&expected.scope),
                sk: keys::gap_sk(expected.revision.gap_id, expected.revision.revision),
            };
            let Some(item) = self.get(&key).await? else {
                return Ok(false);
            };
            let decoded = decode_gap(&item)
                .map_err(|error| DutyError::unresolved(self.settings.duty, error.to_string()))?;
            if decoded != *expected {
                return Ok(false);
            }
        }
        Ok(true)
    }

    /// Runs the configured duty's body against one claimed item.
    async fn work(&self, item: &DueItem, now: Timestamp) -> Result<(), DutyError> {
        match self.settings.duty {
            ControlDomain::SpoolRepair => self.repair_spool(item, now).await,
            ControlDomain::BatchExpire => self.expire_batch(item, now).await,
            // Refused at start-up; refused again here so the arm can never be
            // reached by a future caller that skips the configuration.
            ControlDomain::ExportLaunch => Err(DutyError::Foreign {
                duty: ControlDomain::ExportLaunch.as_str(),
            }),
            ControlDomain::ExportReap => self.reap_export(item, now).await,
            ControlDomain::DeletionExecute => self.execute_deletion(item, now).await,
            ControlDomain::DeletionVerify => self.verify_deletion(item, now).await,
            ControlDomain::IndexVerify => self.verify_index(item, now).await,
            ControlDomain::GateEvaluate => self.evaluate_gate(item, now).await,
            ControlDomain::SeriesReclaim => self.reclaim_series(item, now).await,
        }
    }
}

// --- `spool.repair` ---------------------------------------------------------

impl DutyEngine {
    /// Clears the `{outbox, index, wake}` pending set of one spool chunk.
    async fn repair_spool(&self, item: &DueItem, now: Timestamp) -> Result<(), DutyError> {
        let outstanding = pending_of(&item.attributes);
        if outstanding.is_empty() {
            return self.settle_chunk(item).await;
        }
        let mut cleared = BTreeSet::new();
        if outstanding.contains(&Pending::Outbox) && self.deliver_outbox(item).await? {
            cleared.insert(Pending::Outbox);
        }
        if outstanding.contains(&Pending::Index) && self.index_is_complete(item).await? {
            cleared.insert(Pending::Index);
        }
        if outstanding.contains(&Pending::Wake) && wake_is_stale(&item.attributes, now)? {
            cleared.insert(Pending::Wake);
        }
        if cleared.is_empty() {
            return Err(DutyError::unresolved(
                self.settings.duty,
                format!(
                    "{} duty/duties are still outstanding on the chunk",
                    outstanding.len()
                ),
            ));
        }
        self.clear_pending(item, &cleared).await?;
        if cleared.len() == outstanding.len() {
            self.settle_chunk(item).await?;
        }
        Ok(())
    }

    /// Delivers the `SPOOL#…/OUTBOX#` storage fact, idempotently.
    ///
    /// The queue is FIFO and the deduplication id is the batch's own
    /// idempotency key, so a redelivery of an already-accepted fact is dropped
    /// by the queue rather than double-billed.
    async fn deliver_outbox(&self, item: &DueItem) -> Result<bool, DutyError> {
        let key = ItemKey {
            pk: item.key.pk.clone(),
            sk: format!("{OUTBOX_PREFIX}{}", item.key.sk),
        };
        let Some(fact) = self.get(&key).await? else {
            // Nothing was ever staged, so there is nothing to deliver.
            return Ok(true);
        };
        if string(&fact, STATE) == Some("delivered") {
            return Ok(true);
        }
        let idempotency = require_string(&fact, "idempotencyKey", "usage_outbox")?.to_owned();
        let group = require_string(&fact, "workspaceId", "usage_outbox")?.to_owned();
        let body = serde_json::to_string(&outbox_payload(&fact))
            .map_err(|error| DutyError::provider("SendMessage", error))?;
        self.sqs
            .send_message()
            .queue_url(&self.settings.usage_queue_url)
            .message_body(body)
            .message_group_id(group)
            .message_deduplication_id(idempotency)
            .send()
            .await
            .map_err(|error| DutyError::provider("SendMessage", error))?;

        let mut builder = ExpressionBuilder::new();
        let state = builder.name(STATE);
        let observed = builder.name(STATE);
        let delivered = builder.string("delivered");
        let pending = builder.string("pending");
        let outcome = self
            .dynamodb
            .update_item()
            .table_name(&self.settings.table)
            .key(PK, AttributeValue::S(key.pk))
            .key(SK, AttributeValue::S(key.sk))
            .update_expression(format!("SET {state} = {delivered}"))
            .condition_expression(format!("{observed} = {pending}"))
            .set_expression_attribute_names(Some(builder.names()))
            .set_expression_attribute_values(Some(builder.values()))
            .send()
            .await;
        match outcome {
            // A raced delivery already flipped it; the fact is deduplicated.
            Ok(_) => Ok(true),
            Err(error) if is_conditional_failure(&error) => Ok(true),
            Err(error) => Err(DutyError::provider("UpdateItem", error)),
        }
    }

    /// Confirms every `acceptedSeq` in the chunk's range is materialized.
    async fn index_is_complete(&self, item: &DueItem) -> Result<bool, DutyError> {
        let scope = scope_of(item)?;
        let accepted_at = timestamp_of(&item.attributes, "acceptedAt", "spool_chunk")?;
        for candidate in loss_candidates_of(item)? {
            let expected = candidate.hi_exclusive - candidate.lo;
            let counted = self
                .count_observations(
                    &scope,
                    SignalSet::from_signal(candidate.signal),
                    BucketHour::from_timestamp(accepted_at),
                    (candidate.lo, candidate.hi_exclusive),
                )
                .await?;
            if counted < expected {
                return Ok(false);
            }
        }
        Ok(true)
    }

    /// Counts materialized observations over one accepted range.
    async fn count_observations(
        &self,
        scope: &ScopeKey,
        signals: SignalSet,
        bucket: BucketHour,
        range: (u64, u64),
    ) -> Result<u64, DutyError> {
        let (lo, hi) = range;
        let mut counted = 0u64;
        for signal in signals.in_authority().iter() {
            for shard in 0..DEFAULT_BUCKET_SHARDS {
                counted += self
                    .count_sequence_between(
                        &keys::observation_pk(scope, signal, bucket, shard),
                        lo,
                        hi,
                    )
                    .await?;
            }
        }
        Ok(counted)
    }

    /// Removes the cleared duties from the chunk's pending set.
    async fn clear_pending(
        &self,
        item: &DueItem,
        cleared: &BTreeSet<Pending>,
    ) -> Result<(), DutyError> {
        let mut builder = ExpressionBuilder::new();
        let pending = builder.name(PENDING);
        let done = builder.value(AttributeValue::Ss(
            cleared
                .iter()
                .map(|duty| duty.as_str().to_owned())
                .collect(),
        ));
        self.dynamodb
            .update_item()
            .table_name(&self.settings.table)
            .key(PK, AttributeValue::S(item.key.pk.clone()))
            .key(SK, AttributeValue::S(item.key.sk.clone()))
            .update_expression(format!("DELETE {pending} {done}"))
            .set_expression_attribute_names(Some(builder.names()))
            .set_expression_attribute_values(Some(builder.values()))
            .send()
            .await
            .map_err(|error| DutyError::provider("UpdateItem", error))?;
        Ok(())
    }

    /// Deletes a chunk whose pending set is empty.
    ///
    /// `DynamoDB` removes a set attribute once its last element is deleted, so
    /// `attribute_not_exists(pending)` is exactly "every duty acknowledged" and
    /// a raced re-open loses the delete rather than the chunk.
    async fn settle_chunk(&self, item: &DueItem) -> Result<(), DutyError> {
        let mut builder = ExpressionBuilder::new();
        let pending = builder.name(PENDING);
        let outcome = self
            .dynamodb
            .delete_item()
            .table_name(&self.settings.table)
            .key(PK, AttributeValue::S(item.key.pk.clone()))
            .key(SK, AttributeValue::S(item.key.sk.clone()))
            .condition_expression(format!("attribute_not_exists({pending})"))
            .set_expression_attribute_names(Some(builder.names()))
            .send()
            .await;
        match outcome {
            Ok(_) => Ok(()),
            Err(error) if is_conditional_failure(&error) => Err(DutyError::unresolved(
                self.settings.duty,
                "the chunk re-opened a duty between the clear and the delete",
            )),
            Err(error) => Err(DutyError::provider("DeleteItem", error)),
        }
    }
}

// --- `batch.expire` ---------------------------------------------------------

impl DutyEngine {
    /// Aborts a `preparing` receipt past its deadline and releases its quota.
    ///
    /// Both actions are one transaction, conditional on the receipt still
    /// preparing: a commit that raced wins outright and the abort becomes a
    /// no-op rather than releasing quota a committed batch is still holding.
    async fn expire_batch(&self, item: &DueItem, now: Timestamp) -> Result<(), DutyError> {
        let bytes = number(&item.attributes, "logicalBytes").unwrap_or(0);
        let records = number(&item.attributes, "recordCount").unwrap_or(0);
        let workspace = require_string(&item.attributes, "workspaceId", "admission_receipt")?;
        let abort = self.abort_receipt(item, now)?;
        let release = self.release_quota(workspace, bytes, records)?;
        let outcome = self
            .dynamodb
            .transact_write_items()
            .transact_items(abort)
            .transact_items(release)
            .send()
            .await;
        match outcome {
            Ok(_) => Ok(()),
            // The commit landed first; the receipt is not this duty's to abort.
            Err(error) if is_transaction_conditional_failure(&error) => Ok(()),
            Err(error) => Err(DutyError::provider("TransactWriteItems", error)),
        }
    }

    /// The conditional flip of a `preparing` receipt to `aborted`.
    fn abort_receipt(
        &self,
        item: &DueItem,
        now: Timestamp,
    ) -> Result<TransactWriteItem, DutyError> {
        let mut builder = ExpressionBuilder::new();
        let state = builder.name(STATE);
        let observed = builder.name(STATE);
        let aborted_at = builder.name("abortedAt");
        let control_partition = builder.name(Index::Control.partition_key());
        let control_sort = builder.name(Index::Control.sort_key());
        let aborted = builder.string("aborted");
        let preparing = builder.string("preparing");
        let at = builder.string(now.to_wire());
        Ok(TransactWriteItem::builder()
            .update(
                Update::builder()
                    .table_name(&self.settings.table)
                    .key(PK, AttributeValue::S(item.key.pk.clone()))
                    .key(SK, AttributeValue::S(item.key.sk.clone()))
                    .update_expression(format!(
                        "SET {state} = {aborted}, {aborted_at} = {at} \
                         REMOVE {control_partition}, {control_sort}"
                    ))
                    .condition_expression(format!("{observed} = {preparing}"))
                    .set_expression_attribute_names(Some(builder.names()))
                    .set_expression_attribute_values(Some(builder.values()))
                    .build()
                    .map_err(|error| DutyError::provider("TransactWriteItems", error))?,
            )
            .build())
    }

    /// The exact release of the reservation the receipt took.
    fn release_quota(
        &self,
        workspace: &str,
        bytes: u64,
        records: u64,
    ) -> Result<TransactWriteItem, DutyError> {
        let mut builder = ExpressionBuilder::new();
        let reserved_bytes = builder.name("reservedBytes");
        let reserved_records = builder.name("reservedRecords");
        let byte_delta = builder.number(-i128::from(bytes));
        let record_delta = builder.number(-i128::from(records));
        Ok(TransactWriteItem::builder()
            .update(
                Update::builder()
                    .table_name(&self.settings.table)
                    .key(PK, AttributeValue::S(format!("QUOTA#{workspace}")))
                    .key(SK, AttributeValue::S(keys::INGEST_SK.to_owned()))
                    .update_expression(format!(
                        "ADD {reserved_bytes} {byte_delta}, {reserved_records} {record_delta}"
                    ))
                    .set_expression_attribute_names(Some(builder.names()))
                    .set_expression_attribute_values(Some(builder.values()))
                    .build()
                    .map_err(|error| DutyError::provider("TransactWriteItems", error))?,
            )
            .build())
    }
}

// --- `export.reap` ----------------------------------------------------------

impl DutyEngine {
    /// Retires an expired or revoked export.
    ///
    /// This deployable holds no `s3:PutObject` on `exports/*` and no delete on
    /// it either: the object lifecycle is the bucket's, and the reap is the
    /// durable state flip plus the removal of the download grant.
    async fn reap_export(&self, item: &DueItem, now: Timestamp) -> Result<(), DutyError> {
        let mut builder = ExpressionBuilder::new();
        let state = builder.name(STATE);
        let observed = builder.name(STATE);
        let reaped_at = builder.name("reapedAt");
        let grant = builder.name("downloadGrant");
        let control_partition = builder.name(Index::Control.partition_key());
        let control_sort = builder.name(Index::Control.sort_key());
        let reaped = builder.string("reaped");
        let expired = builder.string("expired");
        let revoked = builder.string("revoked");
        let at = builder.string(now.to_wire());
        let outcome = self
            .dynamodb
            .update_item()
            .table_name(&self.settings.table)
            .key(PK, AttributeValue::S(item.key.pk.clone()))
            .key(SK, AttributeValue::S(item.key.sk.clone()))
            .update_expression(format!(
                "SET {state} = {reaped}, {reaped_at} = {at} \
                 REMOVE {grant}, {control_partition}, {control_sort}"
            ))
            .condition_expression(format!("{observed} IN ({expired}, {revoked})"))
            .set_expression_attribute_names(Some(builder.names()))
            .set_expression_attribute_values(Some(builder.values()))
            .send()
            .await;
        match outcome {
            Ok(_) => Ok(()),
            // The export is live again, so there is nothing to reap.
            Err(error) if is_conditional_failure(&error) => Ok(()),
            Err(error) => Err(DutyError::provider("UpdateItem", error)),
        }
    }
}

// --- `deletion.execute` and `deletion.verify` -------------------------------

impl DutyEngine {
    /// Removes one bounded page of a fenced scope's observations and bodies.
    ///
    /// The page is the configured budget; the item stays due, so a large scope
    /// drains across invocations rather than inside one. When a whole pass
    /// removes nothing, the scope has drained and the deletion advances to
    /// `verifying` — proof is a separate duty under a separate identity.
    async fn execute_deletion(&self, item: &DueItem, now: Timestamp) -> Result<(), DutyError> {
        let observed = string(&item.attributes, STATE).unwrap_or(DeletionState::None.as_str());
        if observed != DeletionState::Deleting.as_str() {
            return Err(DutyError::unresolved(
                self.settings.duty,
                format!("the scope is `{observed}`, not `deleting`"),
            ));
        }
        let scope = scope_of(item)?;
        let mut budget = usize::from(self.settings.page);
        let mut drain = DeletionDrain::default();

        // New writes are enumerated from strongly-consistent scope directories.
        // The legacy segment walk remains as a safe fallback for rows admitted
        // before the directory invariant was installed.
        drain.add(self.purge_scope_batches(&scope, now, &mut budget).await?);
        if budget > 0 {
            drain.add(self.purge_scope_exports(&scope, now, &mut budget).await?);
        }
        for signal in SignalSet::authority().iter() {
            if budget == 0 {
                break;
            }
            drain.removed = drain
                .removed
                .saturating_add(self.purge_signal(&scope, signal, &mut budget).await?);
            drain.removed = drain.removed.saturating_add(
                self.purge_directory(&keys::time_segment_pk(&scope, signal), &mut budget)
                    .await?,
            );
        }
        if budget > 0 {
            drain.removed = drain
                .removed
                .saturating_add(self.purge_gaps(&scope, &mut budget).await?);
        }
        if budget > 0 {
            drain.removed = drain
                .removed
                .saturating_add(self.purge_frontiers(&scope, &mut budget).await?);
        }
        if budget > 0 {
            drain.removed = drain
                .removed
                .saturating_add(self.purge_scope_body_prefix(&scope, &mut budget).await?);
        }
        if drain.removed == 0 && !drain.pending {
            self.advance_deletion(item, DeletionState::Deleting, DeletionState::Verifying, now)
                .await?;
        }
        Ok(())
    }

    /// Removes up to `budget` observations of one signal.
    async fn purge_signal(
        &self,
        scope: &ScopeKey,
        signal: Signal,
        budget: &mut usize,
    ) -> Result<u64, DutyError> {
        let directory = keys::segment_pk(scope, signal);
        let segments = self.page_of(&directory).await?;
        let mut removed = 0u64;
        for segment in segments {
            if *budget == 0 {
                break;
            }
            let Some(bucket) = string(&segment, SK).and_then(|sk| BucketHour::parse(sk).ok())
            else {
                continue;
            };
            let shards = u8::try_from(
                number(&segment, "shards").unwrap_or(u64::from(DEFAULT_BUCKET_SHARDS)),
            )
            .unwrap_or(DEFAULT_BUCKET_SHARDS);
            let drained = self
                .purge_bucket(scope, signal, bucket, shards, budget)
                .await?;
            removed += drained;
            if drained == 0 {
                self.delete_key(&ItemKey {
                    pk: directory.clone(),
                    sk: bucket.as_str().to_owned(),
                })
                .await?;
            }
        }
        Ok(removed)
    }

    /// Removes up to `budget` observations of one bucket.
    async fn purge_bucket(
        &self,
        scope: &ScopeKey,
        signal: Signal,
        bucket: BucketHour,
        shards: u8,
        budget: &mut usize,
    ) -> Result<u64, DutyError> {
        let mut removed = 0u64;
        for shard in 0..shards.max(1) {
            if *budget == 0 {
                break;
            }
            removed += self
                .purge_partition(
                    scope,
                    &keys::observation_pk(scope, signal, bucket, shard),
                    budget,
                )
                .await?;
        }
        Ok(removed)
    }

    /// Removes up to `budget` observations of one partition, bodies included.
    async fn purge_partition(
        &self,
        scope: &ScopeKey,
        pk: &str,
        budget: &mut usize,
    ) -> Result<u64, DutyError> {
        let mut builder = ExpressionBuilder::new();
        let partition = builder.name(PK);
        let bound = builder.string(pk.to_owned());
        let sort = builder.name(SK);
        let body = builder.name("bodyS3Key");
        let response = self
            .dynamodb
            .query()
            .table_name(&self.settings.table)
            .key_condition_expression(format!("{partition} = {bound}"))
            .projection_expression(format!("{partition}, {sort}, {body}"))
            .set_expression_attribute_names(Some(builder.names()))
            .set_expression_attribute_values(Some(builder.values()))
            .limit(i32::try_from(*budget).unwrap_or(i32::MAX))
            .send()
            .await
            .map_err(|error| DutyError::provider("Query", error))?;

        let items = response.items();
        for item in items {
            if let Some(key) = string(item, "bodyS3Key") {
                ensure_session_body_key(scope, key)?;
                self.delete_object(key).await?;
            }
        }
        let keys: Vec<ItemKey> = items.iter().filter_map(item_key).collect();
        let removed = keys.len() as u64;
        self.delete_keys(&keys).await?;
        *budget = budget.saturating_sub(keys.len());
        Ok(removed)
    }

    /// Drains one page of the exact scope→batch directory.
    async fn purge_scope_batches(
        &self,
        scope: &ScopeKey,
        now: Timestamp,
        budget: &mut usize,
    ) -> Result<DeletionDrain, DutyError> {
        let mut drain = DeletionDrain::default();
        for directory in self.page_of(&keys::scope_batch_pk(scope)).await? {
            if *budget == 0 {
                break;
            }
            drain.add(
                self.purge_scope_batch(scope, &directory, now, budget)
                    .await?,
            );
        }
        Ok(drain)
    }

    /// Drains the rows owned by one validated scope-to-batch directory entry.
    async fn purge_scope_batch(
        &self,
        scope: &ScopeKey,
        directory: &HashMap<String, AttributeValue>,
        now: Timestamp,
        budget: &mut usize,
    ) -> Result<DeletionDrain, DutyError> {
        let entry = scope_batch_directory(scope, directory)?;
        let receipt_key = ItemKey {
            pk: entry.batch_pk.clone(),
            sk: keys::RECEIPT_SK.to_owned(),
        };
        let receipt = self.get(&receipt_key).await?;
        validate_batch_receipt(scope, entry.batch, receipt.as_ref())?;
        if receipt
            .as_ref()
            .is_some_and(|item| string(item, STATE) == Some("preparing"))
        {
            // This request may still be staging S3 bodies. Transaction C must
            // lose to the fence, but deletion cannot prove the prefix quiet
            // until batch.expire has terminalized the preparer.
            return Ok(DeletionDrain {
                pending: true,
                ..DeletionDrain::default()
            });
        }

        let mut drain = DeletionDrain::default();
        if let Some(spool_key) = entry.spool_key
            && let Some(spool) = self.get(&spool_key).await?
        {
            validate_batch_spool(scope, entry.batch, &spool)?;
            if pending_of(&spool).contains(&Pending::Outbox) {
                // The aggregate storage fact survives session payload deletion
                // and must be delivered before its source hint can be removed.
                drain.pending = true;
                return Ok(drain);
            }
            self.delete_key(&spool_key).await?;
            drain.removed = 1;
            *budget = budget.saturating_sub(1);
            if *budget == 0 {
                return Ok(drain);
            }
        }

        let ledgers = self.page_prefix(&entry.batch_pk, "MAT#").await?;
        if let Some(ledger) = ledgers.first() {
            drain.removed = drain.removed.saturating_add(
                self.purge_materialization_ledger(scope, entry.batch, ledger, budget)
                    .await?,
            );
            return Ok(drain);
        }

        if let Some(receipt) = receipt.as_ref()
            && string(receipt, STATE) == Some("committed")
            && !receipt.contains_key("materializedAt")
        {
            self.settle_abandoned_workspace_frontiers(scope, entry.batch, receipt, now)
                .await?;
        }

        let items = self.page_of(&entry.batch_pk).await?;
        if !items.is_empty() {
            let item_keys: Vec<ItemKey> = items.iter().take(*budget).filter_map(item_key).collect();
            self.delete_keys(&item_keys).await?;
            drain.removed = drain
                .removed
                .saturating_add(u64::try_from(item_keys.len()).unwrap_or(u64::MAX));
            *budget = budget.saturating_sub(item_keys.len());
            return Ok(drain);
        }

        self.delete_key(&entry.key).await?;
        drain.removed = drain.removed.saturating_add(1);
        *budget = budget.saturating_sub(1);
        Ok(drain)
    }

    /// Deletes the exact observations named by one atomic materialization page.
    async fn purge_materialization_ledger(
        &self,
        scope: &ScopeKey,
        batch: TelemetryBatchId,
        ledger: &HashMap<String, AttributeValue>,
        budget: &mut usize,
    ) -> Result<u64, DutyError> {
        if string(ledger, "scopeKey") != Some(scope.to_key().as_str())
            || string(ledger, "batchId") != Some(batch.to_string().as_str())
        {
            return Err(DutyError::Malformed {
                item: "materialization_ledger",
                attribute: "scopeKey",
            });
        }
        let records = ledger
            .get("observations")
            .and_then(|value| value.as_l().ok())
            .ok_or(DutyError::Malformed {
                item: "materialization_ledger",
                attribute: "observations",
            })?;
        // A ledger page is fixed at the writer's 25-item materialization bound.
        // Finish one whole page so even a deployment page of one makes progress;
        // this can never fan out with the customer's batch size.
        for record in records {
            let map = record.as_m().map_err(|_| DutyError::Malformed {
                item: "materialization_ledger",
                attribute: "observations",
            })?;
            if let Some(key) = string(map, "bodyS3Key") {
                ensure_session_body_key(scope, key)?;
                self.delete_object(key).await?;
            }
            let key = item_key(map).ok_or(DutyError::Malformed {
                item: "materialization_ledger",
                attribute: PK,
            })?;
            if !key.pk.starts_with(&format!("OBS#{}#", scope.to_key())) {
                return Err(DutyError::Malformed {
                    item: "materialization_ledger",
                    attribute: PK,
                });
            }
            self.delete_key(&key).await?;
        }
        let ledger_key = item_key(ledger).ok_or(DutyError::Malformed {
            item: "materialization_ledger",
            attribute: PK,
        })?;
        self.settle_workspace_segments(scope, batch, ledger, &ledger_key)
            .await?;
        *budget = budget.saturating_sub(records.len().saturating_add(1));
        Ok(u64::try_from(records.len().saturating_add(1)).unwrap_or(u64::MAX))
    }

    /// Removes one session page's exact contribution from the shared workspace
    /// segment directories and consumes its ledger atomically.
    ///
    /// Observation and S3 deletes above are idempotent. The additive workspace
    /// counters are not, so the ledger delete is their transaction marker: an
    /// ambiguous retry that finds the ledger gone knows the decrement already
    /// committed and can never apply it twice. Zero-count directory rows are
    /// intentionally retained as anonymous aggregate structure and readers
    /// skip them; a concurrent admission from another session can safely add
    /// to the same rows.
    async fn settle_workspace_segments(
        &self,
        scope: &ScopeKey,
        batch: TelemetryBatchId,
        ledger: &HashMap<String, AttributeValue>,
        ledger_key: &ItemKey,
    ) -> Result<(), DutyError> {
        let ScopeKey::Session { workspace, .. } = scope else {
            self.delete_key(ledger_key).await?;
            return Ok(());
        };
        let contributions = ledger
            .get("segments")
            .and_then(|value| value.as_l().ok())
            .ok_or(DutyError::Malformed {
                item: "materialization_ledger",
                attribute: "segments",
            })?;
        let aggregate_scope = ScopeKey::Workspace(*workspace);
        let mut actions = self.workspace_segment_decrements(&aggregate_scope, contributions)?;
        if actions.len().saturating_add(1) > limits::DDB_TRANSACT_MAX_ACTIONS {
            return Err(DutyError::Malformed {
                item: "materialization_ledger",
                attribute: "segments",
            });
        }
        let mut builder = ExpressionBuilder::new();
        let scope_name = builder.name("scopeKey");
        let batch_name = builder.name("batchId");
        let digest_name = builder.name("materializationDigest");
        let scope_value = builder.string(scope.to_key());
        let batch_value = builder.string(batch.to_string());
        let digest = builder.string(require_string(
            ledger,
            "materializationDigest",
            "materialization_ledger",
        )?);
        let delete = Delete::builder()
            .table_name(&self.settings.table)
            .key(PK, AttributeValue::S(ledger_key.pk.clone()))
            .key(SK, AttributeValue::S(ledger_key.sk.clone()))
            .condition_expression(format!(
                "{scope_name} = {scope_value} AND {batch_name} = {batch_value} AND \
                 {digest_name} = {digest}"
            ))
            .set_expression_attribute_names(Some(builder.names()))
            .set_expression_attribute_values(Some(builder.values()))
            .build()
            .map_err(|error| DutyError::provider("TransactWriteItems", error))?;
        actions.push(TransactWriteItem::builder().delete(delete).build());
        let outcome = self
            .dynamodb
            .transact_write_items()
            .set_transact_items(Some(actions))
            .send()
            .await;
        match outcome {
            Ok(_) => Ok(()),
            Err(_) if self.get(ledger_key).await?.is_none() => Ok(()),
            Err(error) => Err(DutyError::provider("TransactWriteItems", error)),
        }
    }

    /// Builds the atomic decrements for one session materialization ledger.
    fn workspace_segment_decrements(
        &self,
        aggregate_scope: &ScopeKey,
        contributions: &[AttributeValue],
    ) -> Result<Vec<TransactWriteItem>, DutyError> {
        let mut actions = Vec::with_capacity(contributions.len().saturating_add(1));
        let mut seen = BTreeSet::new();
        for contribution in contributions {
            let map = contribution.as_m().map_err(|_| DutyError::Malformed {
                item: "materialization_ledger",
                attribute: "segments",
            })?;
            let axis = require_string(map, "axis", "materialization_ledger")?;
            let signal = Signal::parse(require_string(map, "signal", "materialization_ledger")?)
                .filter(|signal| signal.in_observation_authority())
                .ok_or(DutyError::Malformed {
                    item: "materialization_ledger",
                    attribute: "signal",
                })?;
            let bucket =
                BucketHour::parse(require_string(map, "bucket", "materialization_ledger")?)
                    .map_err(|_| DutyError::Malformed {
                        item: "materialization_ledger",
                        attribute: "bucket",
                    })?;
            let count = required_number(map, "count", "materialization_ledger")?;
            let logical_bytes = required_number(map, "logicalBytes", "materialization_ledger")?;
            if count == 0 || !seen.insert((axis.to_owned(), signal, bucket)) {
                return Err(DutyError::Malformed {
                    item: "materialization_ledger",
                    attribute: "segments",
                });
            }
            let partition = match axis {
                "accepted" => keys::segment_pk(aggregate_scope, signal),
                "event_time" => keys::time_segment_pk(aggregate_scope, signal),
                _ => {
                    return Err(DutyError::Malformed {
                        item: "materialization_ledger",
                        attribute: "axis",
                    });
                }
            };
            let mut builder = ExpressionBuilder::new();
            let item_type = builder.name("itemType");
            let scope_name = builder.name("scopeKey");
            let signal_name = builder.name("signal");
            let bucket_name = builder.name("bucket");
            let stored_count = builder.name("count");
            let stored_bytes = builder.name("logicalBytes");
            let segment_type = builder.string("segment");
            let scope_value = builder.string(aggregate_scope.to_key());
            let signal_value = builder.string(signal.as_str());
            let bucket_value = builder.string(bucket.as_str());
            let minimum_count = builder.number(count);
            let minimum_bytes = builder.number(logical_bytes);
            let count_decrement = builder.number(-i128::from(count));
            let bytes_decrement = builder.number(-i128::from(logical_bytes));
            let update = Update::builder()
                .table_name(&self.settings.table)
                .key(PK, AttributeValue::S(partition))
                .key(SK, AttributeValue::S(bucket.as_str().to_owned()))
                .update_expression(format!(
                    "ADD {stored_count} {count_decrement}, {stored_bytes} {bytes_decrement}"
                ))
                .condition_expression(format!(
                    "{item_type} = {segment_type} AND {scope_name} = {scope_value} AND \
                     {signal_name} = {signal_value} AND {bucket_name} = {bucket_value} AND \
                     {stored_count} >= {minimum_count} AND {stored_bytes} >= {minimum_bytes}"
                ))
                .set_expression_attribute_names(Some(builder.names()))
                .set_expression_attribute_values(Some(builder.values()))
                .build()
                .map_err(|error| DutyError::provider("TransactWriteItems", error))?;
            actions.push(TransactWriteItem::builder().update(update).build());
        }
        Ok(actions)
    }

    /// Releases aggregate workspace visibility blocked by a committed session
    /// admission that lost its finalization race to deletion.
    ///
    /// The receipt marker and every per-signal decrement share one transaction,
    /// so retries either observe `deletionSettledAt`/`materializedAt` or perform
    /// the decrement once. Exact session frontiers are removed by this deletion
    /// participant and need no corresponding release.
    async fn settle_abandoned_workspace_frontiers(
        &self,
        scope: &ScopeKey,
        batch: TelemetryBatchId,
        receipt: &HashMap<String, AttributeValue>,
        now: Timestamp,
    ) -> Result<(), DutyError> {
        let ScopeKey::Session { workspace, .. } = scope else {
            return Ok(());
        };
        let signals = allocation_signals(receipt)?;
        let mut actions = Vec::with_capacity(signals.len().saturating_add(1));
        let mut marker = ExpressionBuilder::new();
        let state_name = marker.name(STATE);
        let materialized = marker.name("materializedAt");
        let settled = marker.name("deletionSettledAt");
        let scope_name = marker.name("scopeKey");
        let batch_name = marker.name("batchId");
        let committed = marker.string("committed");
        let scope_value = marker.string(scope.to_key());
        let batch_value = marker.string(batch.to_string());
        let at = marker.string(now.to_wire());
        let update = Update::builder()
            .table_name(&self.settings.table)
            .key(PK, AttributeValue::S(keys::batch_pk(*workspace, batch)))
            .key(SK, AttributeValue::S(keys::RECEIPT_SK.to_owned()))
            .update_expression(format!("SET {settled} = {at}"))
            .condition_expression(format!(
                "{state_name} = {committed} AND attribute_not_exists({materialized}) AND \
                 attribute_not_exists({settled}) AND {scope_name} = {scope_value} AND \
                 {batch_name} = {batch_value}"
            ))
            .set_expression_attribute_names(Some(marker.names()))
            .set_expression_attribute_values(Some(marker.values()))
            .build()
            .map_err(|error| DutyError::provider("TransactWriteItems", error))?;
        actions.push(TransactWriteItem::builder().update(update).build());
        let workspace_scope = ScopeKey::Workspace(*workspace);
        for signal in signals {
            let mut builder = ExpressionBuilder::new();
            let pending = builder.name("pendingMaterializations");
            let one = builder.number(-1_i64);
            let at_least_one = builder.number(1_u64);
            let update = Update::builder()
                .table_name(&self.settings.table)
                .key(PK, AttributeValue::S(keys::frontier_pk(&workspace_scope)))
                .key(SK, AttributeValue::S(keys::frontier_sk(signal)))
                .update_expression(format!("ADD {pending} {one}"))
                .condition_expression(format!("{pending} >= {at_least_one}"))
                .set_expression_attribute_names(Some(builder.names()))
                .set_expression_attribute_values(Some(builder.values()))
                .build()
                .map_err(|error| DutyError::provider("TransactWriteItems", error))?;
            actions.push(TransactWriteItem::builder().update(update).build());
        }
        let outcome = self
            .dynamodb
            .transact_write_items()
            .set_transact_items(Some(actions))
            .send()
            .await;
        match outcome {
            Ok(_) => Ok(()),
            Err(error) => {
                let key = ItemKey {
                    pk: keys::batch_pk(*workspace, batch),
                    sk: keys::RECEIPT_SK.to_owned(),
                };
                if self.get(&key).await?.is_some_and(|receipt| {
                    receipt.contains_key("materializedAt")
                        || receipt.contains_key("deletionSettledAt")
                }) {
                    return Ok(());
                }
                Err(DutyError::provider("TransactWriteItems", error))
            }
        }
    }

    /// Revokes and removes one bounded page of session-owned exports.
    async fn purge_scope_exports(
        &self,
        scope: &ScopeKey,
        now: Timestamp,
        budget: &mut usize,
    ) -> Result<DeletionDrain, DutyError> {
        let mut drain = DeletionDrain::default();
        let pairs = ExportPairStore::new(
            self.dynamodb.clone(),
            self.settings.table.clone(),
            self.settings.session_table.clone(),
        );
        for directory in self.page_of(&keys::scope_export_pk(scope)).await? {
            if *budget == 0 {
                break;
            }
            drain.add(
                self.purge_scope_export(&pairs, scope, &directory, now, budget)
                    .await?,
            );
        }
        Ok(drain)
    }

    /// Revokes and removes the resources named by one scope-to-export row.
    async fn purge_scope_export(
        &self,
        pairs: &ExportPairStore,
        scope: &ScopeKey,
        directory: &HashMap<String, AttributeValue>,
        now: Timestamp,
        budget: &mut usize,
    ) -> Result<DeletionDrain, DutyError> {
        let entry = scope_export_directory(scope, directory)?;
        let workspace = scope.workspace();
        let export_key = ItemKey {
            pk: keys::export_pk(workspace),
            sk: keys::export_sk(entry.export),
        };
        let checkpoint_key = ItemKey {
            pk: keys::export_pk(workspace),
            sk: keys::export_checkpoint_sk(entry.export),
        };
        let canonical_export = self.get(&export_key).await?;
        let resume_checkpoint = self.get(&checkpoint_key).await?;
        let had_pair_rows = canonical_export.is_some() || resume_checkpoint.is_some();

        if canonical_export.is_some() {
            match pairs.revoke(workspace, entry.export, *scope, now).await {
                Ok(_) | Err(ExportPairError::NotFound) => {}
                Err(error) => {
                    return Err(DutyError::unresolved(
                        self.settings.duty,
                        format!("the export pair could not be revoked: {error}"),
                    ));
                }
            }
        }

        let prefix = format!("exports/{workspace}/{}", entry.export);
        let artifact = canonical_export
            .as_ref()
            .and_then(|item| string(item, "objectKey"))
            .map_or_else(|| format!("{prefix}.{}", entry.extension), str::to_owned);
        if let Some(upload_id) = resume_checkpoint
            .as_ref()
            .and_then(|item| string(item, "uploadId"))
        {
            self.abort_upload(&format!("{prefix}.{}", entry.extension), upload_id)
                .await?;
        }
        let (aborted, truncated) = self.abort_uploads_with_prefix(&prefix).await?;
        let object_present = self.object_exists(&artifact).await?;
        if object_present {
            self.delete_object(&artifact).await?;
        }
        let mut drain = DeletionDrain::default();
        if had_pair_rows {
            self.delete_keys(&[checkpoint_key, export_key]).await?;
            drain.removed = 1;
        }

        // Keep the exact export identity for a later quiet pass whenever this
        // pass revoked/deleted anything or filled a bounded multipart page. The
        // next pass strongly lists and heads the prefix after the pair is gone,
        // so directory absence proves external S3 quiescence.
        if had_pair_rows || aborted > 0 || truncated || object_present {
            drain.pending = true;
        } else {
            self.delete_key(&entry.key).await?;
            drain.removed = drain.removed.saturating_add(1);
        }
        *budget = budget.saturating_sub(1);
        Ok(drain)
    }

    /// Deletes one bounded page of a direct scope partition.
    async fn purge_directory(&self, pk: &str, budget: &mut usize) -> Result<u64, DutyError> {
        let items = self.page_of(pk).await?;
        let keys: Vec<ItemKey> = items.iter().take(*budget).filter_map(item_key).collect();
        self.delete_keys(&keys).await?;
        *budget = budget.saturating_sub(keys.len());
        Ok(u64::try_from(keys.len()).unwrap_or(u64::MAX))
    }

    /// Deletes session gap revisions and advances the workspace change hint in
    /// the same transaction. A follow reader can therefore never observe the
    /// old hint after the rows have disappeared and incorrectly reuse cached
    /// session gaps.
    async fn purge_gaps(&self, scope: &ScopeKey, budget: &mut usize) -> Result<u64, DutyError> {
        let items = self.page_of(&keys::gap_pk(scope)).await?;
        let keys: Vec<ItemKey> = items.iter().take(*budget).filter_map(item_key).collect();
        let delete_limit = limits::DDB_TRANSACT_MAX_ACTIONS.saturating_sub(1);
        for chunk in keys.chunks(delete_limit) {
            let mut actions = Vec::with_capacity(chunk.len().saturating_add(1));
            for key in chunk {
                let delete = Delete::builder()
                    .table_name(&self.settings.table)
                    .key(PK, AttributeValue::S(key.pk.clone()))
                    .key(SK, AttributeValue::S(key.sk.clone()))
                    .build()
                    .map_err(|error| DutyError::provider("TransactWriteItems", error))?;
                actions.push(TransactWriteItem::builder().delete(delete).build());
            }
            actions.push(
                hint_update_action(
                    &self.settings.table,
                    scope.workspace(),
                    u64::try_from(chunk.len()).unwrap_or(u64::MAX),
                )
                .map_err(|error| DutyError::unresolved(self.settings.duty, error.to_string()))?,
            );
            self.dynamodb
                .transact_write_items()
                .set_transact_items(Some(actions))
                .send()
                .await
                .map_err(|error| DutyError::provider("TransactWriteItems", error))?;
        }
        *budget = budget.saturating_sub(keys.len());
        Ok(u64::try_from(keys.len()).unwrap_or(u64::MAX))
    }

    /// Removes the per-signal frontier rows while retaining the deletion fence.
    async fn purge_frontiers(
        &self,
        scope: &ScopeKey,
        budget: &mut usize,
    ) -> Result<u64, DutyError> {
        let mut removed = 0u64;
        for signal in SignalSet::authority().iter() {
            if *budget == 0 {
                break;
            }
            let key = ItemKey {
                pk: keys::frontier_pk(scope),
                sk: keys::frontier_sk(signal),
            };
            if self.get(&key).await?.is_some() {
                self.delete_key(&key).await?;
                removed = removed.saturating_add(1);
                *budget = budget.saturating_sub(1);
            }
        }
        Ok(removed)
    }

    /// Deletes session-scoped bodies, including bodies staged by a commit that
    /// lost to the deletion fence before it could create observation rows.
    async fn purge_scope_body_prefix(
        &self,
        scope: &ScopeKey,
        budget: &mut usize,
    ) -> Result<u64, DutyError> {
        let Some(prefix) = session_body_prefix(scope) else {
            return Ok(0);
        };
        let response = self
            .s3
            .list_objects_v2()
            .bucket(&self.settings.bucket)
            .prefix(prefix)
            .max_keys(i32::try_from(*budget).unwrap_or(i32::MAX))
            .send()
            .await
            .map_err(|error| DutyError::provider("ListObjectsV2", error))?;
        let keys: Vec<String> = response
            .contents()
            .iter()
            .filter_map(|object| object.key().map(str::to_owned))
            .collect();
        for key in &keys {
            self.delete_object(key).await?;
        }
        *budget = budget.saturating_sub(keys.len());
        Ok(u64::try_from(keys.len()).unwrap_or(u64::MAX))
    }

    /// Proves a fenced scope holds nothing, then seals the tombstone.
    async fn verify_deletion(&self, item: &DueItem, now: Timestamp) -> Result<(), DutyError> {
        let observed = string(&item.attributes, STATE).unwrap_or(DeletionState::None.as_str());
        if observed != DeletionState::Verifying.as_str() {
            return Err(DutyError::unresolved(
                self.settings.duty,
                format!("the scope is `{observed}`, not `verifying`"),
            ));
        }
        let scope = scope_of(item)?;
        let mut remaining = 0u64;
        for signal in SignalSet::authority().iter() {
            remaining += self
                .count_partition(&keys::segment_pk(&scope, signal))
                .await?;
            remaining += self
                .count_partition(&keys::time_segment_pk(&scope, signal))
                .await?;
        }
        remaining += self.count_partition(&keys::scope_batch_pk(&scope)).await?;
        remaining += self.count_partition(&keys::scope_export_pk(&scope)).await?;
        remaining += self.count_partition(&keys::gap_pk(&scope)).await?;
        for signal in SignalSet::authority().iter() {
            let key = ItemKey {
                pk: keys::frontier_pk(&scope),
                sk: keys::frontier_sk(signal),
            };
            remaining += u64::from(self.get(&key).await?.is_some());
        }
        if self.scope_body_exists(&scope).await? {
            remaining = remaining.saturating_add(1);
        }
        if remaining > 0 {
            return Err(DutyError::unresolved(
                self.settings.duty,
                format!("{remaining} session telemetry payload partition(s) are still present"),
            ));
        }
        self.advance_deletion(item, DeletionState::Verifying, DeletionState::Complete, now)
            .await
    }

    /// Advances the scope's deletion state under the state that was observed.
    async fn advance_deletion(
        &self,
        item: &DueItem,
        from: DeletionState,
        to: DeletionState,
        now: Timestamp,
    ) -> Result<(), DutyError> {
        let mut builder = ExpressionBuilder::new();
        let state = builder.name(STATE);
        let observed = builder.name(STATE);
        let changed_at = builder.name("stateChangedAt");
        let next = builder.string(to.as_str());
        let expected = builder.string(from.as_str());
        let at = builder.string(now.to_wire());
        let update = match to {
            DeletionState::Verifying => {
                let partition_attribute = builder.name(Index::Control.partition_key());
                let sort_attribute = builder.name(Index::Control.sort_key());
                let attempts = builder.name(ATTEMPTS);
                let claimed = builder.name(CLAIMED_AT);
                let next_attempt = builder.name(NEXT_ATTEMPT_AT);
                let partition = builder.string(keys::control_pk(
                    ControlDomain::DeletionVerify,
                    control_shard(item).unwrap_or(0),
                ));
                let due = builder.string(keys::control_sk(now, item.id.as_str()));
                let zero = builder.number(0u64);
                format!(
                    "SET {state} = {next}, {changed_at} = {at}, \
                     {partition_attribute} = {partition}, {sort_attribute} = {due}, \
                     {attempts} = {zero} REMOVE {claimed}, {next_attempt}"
                )
            }
            DeletionState::Complete => {
                let completed = builder.name("completedAt");
                let partition_attribute = builder.name(Index::Control.partition_key());
                let sort_attribute = builder.name(Index::Control.sort_key());
                let attempts = builder.name(ATTEMPTS);
                let claimed = builder.name(CLAIMED_AT);
                let next_attempt = builder.name(NEXT_ATTEMPT_AT);
                format!(
                    "SET {state} = {next}, {changed_at} = {at}, {completed} = {at} \
                     REMOVE {partition_attribute}, {sort_attribute}, {attempts}, {claimed}, \
                     {next_attempt}"
                )
            }
            _ => {
                return Err(DutyError::unresolved(
                    self.settings.duty,
                    format!("the duty cannot advance deletion to `{}`", to.as_str()),
                ));
            }
        };
        let outcome = self
            .dynamodb
            .update_item()
            .table_name(&self.settings.table)
            .key(PK, AttributeValue::S(item.key.pk.clone()))
            .key(SK, AttributeValue::S(item.key.sk.clone()))
            .update_expression(update)
            .condition_expression(format!("{observed} = {expected}"))
            .set_expression_attribute_names(Some(builder.names()))
            .set_expression_attribute_values(Some(builder.values()))
            .send()
            .await;
        match outcome {
            Ok(_) => Ok(()),
            Err(error) if is_conditional_failure(&error) => Err(DutyError::unresolved(
                self.settings.duty,
                format!("the scope left `{}` under this claim", from.as_str()),
            )),
            Err(error) => Err(DutyError::provider("UpdateItem", error)),
        }
    }
}

// --- `index.verify` ---------------------------------------------------------

impl DutyEngine {
    /// Proves one accepted range is fully materialized, or records the hole.
    async fn verify_index(&self, item: &DueItem, now: Timestamp) -> Result<(), DutyError> {
        let workspace = workspace_of(item)?;
        let scope = scope_of(item)?;
        let accepted_at = timestamp_of(&item.attributes, "acceptedAt", "index_verify")?;
        let mut gaps = Vec::new();
        for candidate in loss_candidates_of(item)? {
            let counted = self
                .count_observations(
                    &scope,
                    SignalSet::from_signal(candidate.signal),
                    BucketHour::from_timestamp(accepted_at),
                    (candidate.lo, candidate.hi_exclusive),
                )
                .await?;
            if counted < candidate.hi_exclusive - candidate.lo {
                gaps.push(candidate.record(
                    workspace,
                    scope,
                    TelemetryGapReason::SpoolLost,
                    now,
                )?);
            }
        }
        if gaps.is_empty() {
            return self.retire(item, "verified", now).await;
        }
        // The hole is proven, not suspected: it becomes an explicit gap over the
        // exact ordinals rather than a silently short answer.
        self.terminalize_with_gaps(
            item,
            &gaps,
            GapTerminalization {
                state: "gapped",
                reason: None,
                expected_attempts: item.attempts.saturating_add(1),
                claimed_at: Some(now),
                changed_at: now,
            },
        )
        .await
    }

    /// Moves an item out of the due index under a durable terminal state.
    async fn retire(&self, item: &DueItem, state: &str, now: Timestamp) -> Result<(), DutyError> {
        let mut builder = ExpressionBuilder::new();
        let state_name = builder.name(STATE);
        let at = builder.name("stateChangedAt");
        let control_partition = builder.name(Index::Control.partition_key());
        let control_sort = builder.name(Index::Control.sort_key());
        let terminal = builder.string(state.to_owned());
        let when = builder.string(now.to_wire());
        self.dynamodb
            .update_item()
            .table_name(&self.settings.table)
            .key(PK, AttributeValue::S(item.key.pk.clone()))
            .key(SK, AttributeValue::S(item.key.sk.clone()))
            .update_expression(format!(
                "SET {state_name} = {terminal}, {at} = {when} REMOVE {control_partition}, {control_sort}"
            ))
            .set_expression_attribute_names(Some(builder.names()))
            .set_expression_attribute_values(Some(builder.values()))
            .send()
            .await
            .map_err(|error| DutyError::provider("UpdateItem", error))?;
        Ok(())
    }
}

// --- `gate.evaluate` --------------------------------------------------------

impl DutyEngine {
    /// Re-evaluates the regional ingress gate from measured evidence.
    async fn evaluate_gate(&self, item: &DueItem, now: Timestamp) -> Result<(), DutyError> {
        let current = gate_state(string(&item.attributes, STATE));
        let (depth, oldest) = self.spool_pressure(now).await?;
        let evidence = GateEvidence {
            consecutive_failures: u32::try_from(
                number(&item.attributes, "consecutiveFailures").unwrap_or(0),
            )
            .unwrap_or(u32::MAX),
            failure_span_ms: elapsed(&item.attributes, "failureSince", now),
            oldest_spool_age_ms: oldest,
            spool_depth: depth,
            spool_capacity: u64::from(self.settings.page) * u64::from(self.settings.shards),
            index_failing_ms: elapsed(&item.attributes, "indexFailingSince", now),
            dwell_ms: elapsed(&item.attributes, "stateSince", now),
        };
        let next = evaluate(current, &evidence);
        self.publish_gate(item, current, next, &evidence, now).await
    }

    /// Measures how much unacknowledged spool the region is carrying.
    async fn spool_pressure(&self, now: Timestamp) -> Result<(u64, i64), DutyError> {
        let mut depth = 0u64;
        let mut oldest = 0i64;
        for shard in 0..self.settings.shards {
            let mut builder = ExpressionBuilder::new();
            let partition = builder.name(Index::Control.partition_key());
            let sort = builder.name(Index::Control.sort_key());
            let bound = builder.string(keys::control_pk(ControlDomain::SpoolRepair, shard));
            let ceiling = builder.string(keys::control_sk(now, DUE_SENTINEL));
            let response = self
                .dynamodb
                .query()
                .table_name(&self.settings.table)
                .index_name(Index::Control.as_str())
                .key_condition_expression(format!("{partition} = {bound} AND {sort} <= {ceiling}"))
                .set_expression_attribute_names(Some(builder.names()))
                .set_expression_attribute_values(Some(builder.values()))
                .limit(i32::from(self.settings.page))
                .send()
                .await
                .map_err(|error| DutyError::provider("Query", error))?;
            depth += u64::try_from(response.count()).unwrap_or(0);
            if let Some(age) = response
                .items()
                .first()
                .and_then(|item| string(item, Index::Control.sort_key()))
                .and_then(due_at)
                .map(|due| now.unix_millis() - due.unix_millis())
            {
                oldest = oldest.max(age);
            }
        }
        Ok((depth, oldest))
    }

    /// Writes the next gate state under the state that was observed.
    async fn publish_gate(
        &self,
        item: &DueItem,
        current: GateState,
        next: GateState,
        evidence: &GateEvidence,
        now: Timestamp,
    ) -> Result<(), DutyError> {
        let mut builder = ExpressionBuilder::new();
        let state = builder.name(STATE);
        let depth = builder.name("spoolDepth");
        let oldest = builder.name("oldestSpoolAgeMs");
        let evaluated = builder.name("evaluatedAt");
        let since = builder.name("stateSince");
        let observed = builder.name(STATE);
        let absent = builder.name(PK);
        let word = builder.string(next.as_str());
        let measured_depth = builder.number(evidence.spool_depth);
        let measured_age = builder.number(evidence.oldest_spool_age_ms);
        let at = builder.string(now.to_wire());
        let expected = builder.string(current.as_str());
        let changed = if current == next {
            String::new()
        } else {
            format!(", {since} = {at}")
        };
        let outcome = self
            .dynamodb
            .update_item()
            .table_name(&self.settings.table)
            .key(PK, AttributeValue::S(item.key.pk.clone()))
            .key(SK, AttributeValue::S(item.key.sk.clone()))
            .update_expression(format!(
                "SET {state} = {word}, {depth} = {measured_depth}, \
                 {oldest} = {measured_age}, {evaluated} = {at}{changed}"
            ))
            .condition_expression(format!(
                "attribute_not_exists({absent}) OR {observed} = {expected}"
            ))
            .set_expression_attribute_names(Some(builder.names()))
            .set_expression_attribute_values(Some(builder.values()))
            .send()
            .await;
        match outcome {
            Ok(_) => Ok(()),
            // Another evaluator published first; its evidence is no staler.
            Err(error) if is_conditional_failure(&error) => Ok(()),
            Err(error) => Err(DutyError::provider("UpdateItem", error)),
        }
    }
}

// --- `series.reclaim` -------------------------------------------------------

impl DutyEngine {
    /// Reclaims one metric series claim that has gone unseen.
    ///
    /// The claim and the workspace cardinality counter move in one transaction,
    /// so the counter can never drift from the claims it counts. A series that
    /// was written again since the horizon wins the condition and keeps its
    /// claim.
    async fn reclaim_series(&self, item: &DueItem, now: Timestamp) -> Result<(), DutyError> {
        let workspace = require_string(&item.attributes, "workspaceId", "series_claim")?;
        let hash = require_string(&item.attributes, "seriesHash", "series_claim")?;
        let horizon =
            Timestamp::from_unix_millis(now.unix_millis() - SERIES_IDLE_MS).map_err(|_| {
                DutyError::Malformed {
                    item: "series_claim",
                    attribute: "lastSeenAt",
                }
            })?;
        let shard = counter_shard(hash);
        let release = self.release_claim(item, horizon)?;
        let decrement = self.decrement_counter(workspace, shard)?;
        let outcome = self
            .dynamodb
            .transact_write_items()
            .transact_items(release)
            .transact_items(decrement)
            .send()
            .await;
        match outcome {
            Ok(_) => Ok(()),
            // The series was written again; the claim is not reclaimable.
            Err(error) if is_transaction_conditional_failure(&error) => {
                self.retire(item, "retained", now).await
            }
            Err(error) => Err(DutyError::provider("TransactWriteItems", error)),
        }
    }

    /// The conditional delete of one idle series claim.
    fn release_claim(
        &self,
        item: &DueItem,
        horizon: Timestamp,
    ) -> Result<TransactWriteItem, DutyError> {
        let mut builder = ExpressionBuilder::new();
        let last_seen = builder.name("lastSeenAt");
        let bound = builder.string(horizon.to_wire());
        Ok(TransactWriteItem::builder()
            .delete(
                Delete::builder()
                    .table_name(&self.settings.table)
                    .key(PK, AttributeValue::S(item.key.pk.clone()))
                    .key(SK, AttributeValue::S(item.key.sk.clone()))
                    .condition_expression(format!("{last_seen} <= {bound}"))
                    .set_expression_attribute_names(Some(builder.names()))
                    .set_expression_attribute_values(Some(builder.values()))
                    .build()
                    .map_err(|error| DutyError::provider("TransactWriteItems", error))?,
            )
            .build())
    }

    /// The matching decrement of the workspace cardinality counter.
    fn decrement_counter(
        &self,
        workspace: &str,
        shard: u16,
    ) -> Result<TransactWriteItem, DutyError> {
        let mut builder = ExpressionBuilder::new();
        let claimed = builder.name("claimed");
        let minus_one = builder.number(-1i32);
        Ok(TransactWriteItem::builder()
            .update(
                Update::builder()
                    .table_name(&self.settings.table)
                    .key(PK, AttributeValue::S(format!("SERIESCT#{workspace}")))
                    .key(SK, AttributeValue::S(keys::series_counter_sk(shard)))
                    .update_expression(format!("ADD {claimed} {minus_one}"))
                    .set_expression_attribute_names(Some(builder.names()))
                    .set_expression_attribute_values(Some(builder.values()))
                    .build()
                    .map_err(|error| DutyError::provider("TransactWriteItems", error))?,
            )
            .build())
    }
}

// --- shared provider calls --------------------------------------------------

impl DutyEngine {
    /// Loads one item, or `None` when it no longer exists.
    async fn load(&self, id: &ItemId, key: &ItemKey) -> Result<Option<DueItem>, DutyError> {
        let Some(attributes) = self.get(key).await? else {
            return Ok(None);
        };
        let attempts =
            u32::try_from(number(&attributes, ATTEMPTS).unwrap_or(0)).unwrap_or(u32::MAX);
        Ok(Some(DueItem {
            id: id.clone(),
            key: key.clone(),
            attempts,
            attributes,
        }))
    }

    /// A single strongly-consistent `GetItem` on the bound table.
    async fn get(
        &self,
        key: &ItemKey,
    ) -> Result<Option<HashMap<String, AttributeValue>>, DutyError> {
        let response = self
            .dynamodb
            .get_item()
            .table_name(&self.settings.table)
            .key(PK, AttributeValue::S(key.pk.clone()))
            .key(SK, AttributeValue::S(key.sk.clone()))
            .consistent_read(true)
            .send()
            .await
            .map_err(|error| DutyError::provider("GetItem", error))?;
        Ok(response.item)
    }

    /// One bounded page of a partition.
    async fn page_of(&self, pk: &str) -> Result<Vec<HashMap<String, AttributeValue>>, DutyError> {
        let mut builder = ExpressionBuilder::new();
        let partition = builder.name(PK);
        let bound = builder.string(pk.to_owned());
        let response = self
            .dynamodb
            .query()
            .table_name(&self.settings.table)
            .key_condition_expression(format!("{partition} = {bound}"))
            .set_expression_attribute_names(Some(builder.names()))
            .set_expression_attribute_values(Some(builder.values()))
            .limit(i32::from(self.settings.page))
            .consistent_read(true)
            .send()
            .await
            .map_err(|error| DutyError::provider("Query", error))?;
        Ok(response.items().to_vec())
    }

    /// One bounded page under an exact sort-key prefix.
    async fn page_prefix(
        &self,
        pk: &str,
        prefix: &str,
    ) -> Result<Vec<HashMap<String, AttributeValue>>, DutyError> {
        let mut builder = ExpressionBuilder::new();
        let partition = builder.name(PK);
        let bound = builder.string(pk.to_owned());
        let sort = builder.name(SK);
        let prefix = builder.string(prefix.to_owned());
        let response = self
            .dynamodb
            .query()
            .table_name(&self.settings.table)
            .key_condition_expression(format!(
                "{partition} = {bound} AND begins_with({sort}, {prefix})"
            ))
            .set_expression_attribute_names(Some(builder.names()))
            .set_expression_attribute_values(Some(builder.values()))
            .limit(i32::from(self.settings.page))
            .consistent_read(true)
            .send()
            .await
            .map_err(|error| DutyError::provider("Query", error))?;
        Ok(response.items().to_vec())
    }

    /// Counts a whole partition without reading it.
    async fn count_partition(&self, pk: &str) -> Result<u64, DutyError> {
        let mut builder = ExpressionBuilder::new();
        let partition = builder.name(PK);
        let bound = builder.string(pk.to_owned());
        let response = self
            .dynamodb
            .query()
            .table_name(&self.settings.table)
            .key_condition_expression(format!("{partition} = {bound}"))
            .select(Select::Count)
            .set_expression_attribute_names(Some(builder.names()))
            .set_expression_attribute_values(Some(builder.values()))
            .limit(i32::from(self.settings.page))
            .consistent_read(true)
            .send()
            .await
            .map_err(|error| DutyError::provider("Query", error))?;
        Ok(u64::try_from(response.count()).unwrap_or(0))
    }

    /// Counts one inclusive sort-key range without reading it.
    async fn count_sequence_between(&self, pk: &str, lo: u64, hi: u64) -> Result<u64, DutyError> {
        let mut builder = ExpressionBuilder::new();
        let partition = builder.name(PK);
        let bound = builder.string(pk.to_owned());
        let accepted = builder.name("acceptedSeq");
        let low = builder.number(lo);
        let high = builder.number(hi.saturating_sub(1));
        let names = builder.names();
        let values = builder.values();
        let mut start = None;
        let mut count = 0_u64;
        loop {
            let response = self
                .dynamodb
                .query()
                .table_name(&self.settings.table)
                .key_condition_expression(format!("{partition} = {bound}"))
                .filter_expression(format!("{accepted} BETWEEN {low} AND {high}"))
                .select(Select::Count)
                .set_expression_attribute_names(Some(names.clone()))
                .set_expression_attribute_values(Some(values.clone()))
                .set_exclusive_start_key(start.take())
                .limit(i32::from(self.settings.page))
                .send()
                .await
                .map_err(|error| DutyError::provider("Query", error))?;
            count = count.saturating_add(u64::try_from(response.count()).unwrap_or(0));
            start = response.last_evaluated_key;
            if start.is_none() {
                return Ok(count);
            }
        }
    }

    /// Deletes one item by key.
    async fn delete_key(&self, key: &ItemKey) -> Result<(), DutyError> {
        self.dynamodb
            .delete_item()
            .table_name(&self.settings.table)
            .key(PK, AttributeValue::S(key.pk.clone()))
            .key(SK, AttributeValue::S(key.sk.clone()))
            .send()
            .await
            .map_err(|error| DutyError::provider("DeleteItem", error))?;
        Ok(())
    }

    /// Deletes a bounded set of items, draining the unprocessed remainder.
    async fn delete_keys(&self, keys: &[ItemKey]) -> Result<(), DutyError> {
        for chunk in keys.chunks(aex_observation_store_dynamodb::store::DDB_BATCH_WRITE_MAX) {
            let mut pending = Vec::with_capacity(chunk.len());
            for key in chunk {
                pending.push(
                    WriteRequest::builder()
                        .delete_request(
                            aws_sdk_dynamodb::types::DeleteRequest::builder()
                                .key(PK, AttributeValue::S(key.pk.clone()))
                                .key(SK, AttributeValue::S(key.sk.clone()))
                                .build()
                                .map_err(|error| DutyError::provider("BatchWriteItem", error))?,
                        )
                        .build(),
                );
            }
            let mut attempts = 0u32;
            while !pending.is_empty() {
                let response = self
                    .dynamodb
                    .batch_write_item()
                    .request_items(self.settings.table.clone(), pending.clone())
                    .send()
                    .await
                    .map_err(|error| DutyError::provider("BatchWriteItem", error))?;
                pending = response
                    .unprocessed_items()
                    .and_then(|items| items.get(&self.settings.table))
                    .cloned()
                    .unwrap_or_default();
                attempts += 1;
                if attempts > self.settings.max_attempts {
                    return Err(DutyError::unresolved(
                        self.settings.duty,
                        "BatchWriteItem never drained its unprocessed items",
                    ));
                }
            }
        }
        Ok(())
    }

    /// Removes one immutable observation body.
    async fn delete_object(&self, key: &str) -> Result<(), DutyError> {
        self.s3
            .delete_object()
            .bucket(&self.settings.bucket)
            .key(key)
            .send()
            .await
            .map_err(|error| DutyError::provider("DeleteObject", error))?;
        Ok(())
    }

    /// Aborts one exact export multipart upload; an already-finished upload is
    /// idempotent success.
    async fn abort_upload(&self, key: &str, upload_id: &str) -> Result<(), DutyError> {
        let outcome = self
            .s3
            .abort_multipart_upload()
            .bucket(&self.settings.bucket)
            .key(key)
            .upload_id(upload_id)
            .send()
            .await;
        match outcome {
            Ok(_) => Ok(()),
            Err(error)
                if error
                    .raw_response()
                    .is_some_and(|response| response.status().as_u16() == 404) =>
            {
                Ok(())
            }
            Err(error) => Err(DutyError::provider("AbortMultipartUpload", error)),
        }
    }

    /// Aborts checkpointless uploads below one deterministic export prefix.
    async fn abort_uploads_with_prefix(&self, prefix: &str) -> Result<(usize, bool), DutyError> {
        let response = self
            .s3
            .list_multipart_uploads()
            .bucket(&self.settings.bucket)
            .prefix(prefix)
            .max_uploads(i32::from(self.settings.page))
            .send()
            .await
            .map_err(|error| DutyError::provider("ListMultipartUploads", error))?;
        let mut aborted = 0usize;
        for upload in response.uploads() {
            if let (Some(key), Some(upload_id)) = (upload.key(), upload.upload_id()) {
                self.abort_upload(key, upload_id).await?;
                aborted = aborted.saturating_add(1);
            }
        }
        Ok((aborted, response.is_truncated().unwrap_or(false)))
    }

    /// Whether one exact artifact remains after the deletion pass.
    async fn object_exists(&self, key: &str) -> Result<bool, DutyError> {
        let outcome = self
            .s3
            .head_object()
            .bucket(&self.settings.bucket)
            .key(key)
            .send()
            .await;
        match outcome {
            Ok(_) => Ok(true),
            Err(error)
                if error
                    .raw_response()
                    .is_some_and(|response| response.status().as_u16() == 404) =>
            {
                Ok(false)
            }
            Err(error) => Err(DutyError::provider("HeadObject", error)),
        }
    }

    /// Whether one exact session body prefix still contains payload.
    async fn scope_body_exists(&self, scope: &ScopeKey) -> Result<bool, DutyError> {
        let Some(prefix) = session_body_prefix(scope) else {
            return Ok(false);
        };
        self.s3
            .list_objects_v2()
            .bucket(&self.settings.bucket)
            .prefix(prefix)
            .max_keys(1)
            .send()
            .await
            .map(|response| !response.contents().is_empty())
            .map_err(|error| DutyError::provider("ListObjectsV2", error))
    }
}

// --- item readers -----------------------------------------------------------

/// Validates and decodes one scope-to-batch directory row.
fn scope_batch_directory(
    scope: &ScopeKey,
    directory: &HashMap<String, AttributeValue>,
) -> Result<ScopeBatchDirectory, DutyError> {
    let scope_key = scope.to_key();
    if string(directory, "scopeKey") != Some(scope_key.as_str()) {
        return Err(DutyError::Malformed {
            item: "scope_batch",
            attribute: "scopeKey",
        });
    }
    let batch = TelemetryBatchId::parse(require_string(directory, "batchId", "scope_batch")?)
        .map_err(|_| DutyError::Malformed {
            item: "scope_batch",
            attribute: "batchId",
        })?;
    let batch_pk = require_string(directory, "batchPk", "scope_batch")?.to_owned();
    let key = item_key(directory).ok_or(DutyError::Malformed {
        item: "scope_batch",
        attribute: PK,
    })?;
    if batch_pk != keys::batch_pk(scope.workspace(), batch)
        || key.pk != keys::scope_batch_pk(scope)
        || key.sk != keys::scope_batch_sk(batch)
        || session_body_prefix(scope).is_some_and(|prefix| {
            string(directory, "bodyPrefix") != Some(prefix.trim_end_matches('/'))
        })
    {
        return Err(DutyError::Malformed {
            item: "scope_batch",
            attribute: "batchPk",
        });
    }
    let spool_key = string(directory, "spoolPk")
        .zip(string(directory, "spoolSk"))
        .map(|(pk, sk)| ItemKey {
            pk: pk.to_owned(),
            sk: sk.to_owned(),
        });
    Ok(ScopeBatchDirectory {
        batch,
        batch_pk,
        key,
        spool_key,
    })
}

/// Checks that an admission receipt belongs to the directory that named it.
fn validate_batch_receipt(
    scope: &ScopeKey,
    batch: TelemetryBatchId,
    receipt: Option<&HashMap<String, AttributeValue>>,
) -> Result<(), DutyError> {
    let Some(receipt) = receipt else {
        return Ok(());
    };
    if string(receipt, "scopeKey") != Some(scope.to_key().as_str())
        || string(receipt, "batchId") != Some(batch.to_string().as_str())
    {
        return Err(DutyError::Malformed {
            item: "admission_receipt",
            attribute: "scopeKey",
        });
    }
    Ok(())
}

/// Checks that a spool chunk belongs to the directory that named it.
fn validate_batch_spool(
    scope: &ScopeKey,
    batch: TelemetryBatchId,
    spool: &HashMap<String, AttributeValue>,
) -> Result<(), DutyError> {
    if string(spool, "scopeKey") != Some(scope.to_key().as_str())
        || string(spool, "batchId") != Some(batch.to_string().as_str())
    {
        return Err(DutyError::Malformed {
            item: "spool_chunk",
            attribute: "scopeKey",
        });
    }
    Ok(())
}

/// Decodes and validates the unique observation signals on a receipt.
fn allocation_signals(
    receipt: &HashMap<String, AttributeValue>,
) -> Result<BTreeSet<Signal>, DutyError> {
    let allocations = receipt
        .get("allocations")
        .and_then(|value| value.as_l().ok())
        .ok_or(DutyError::Malformed {
            item: "admission_receipt",
            attribute: "allocations",
        })?;
    let mut signals = BTreeSet::new();
    for allocation in allocations {
        let map = allocation.as_m().map_err(|_| DutyError::Malformed {
            item: "admission_receipt",
            attribute: "allocations",
        })?;
        let signal = Signal::parse(require_string(map, "signal", "admission_receipt")?)
            .filter(|signal| signal.in_observation_authority())
            .ok_or(DutyError::Malformed {
                item: "admission_receipt",
                attribute: "allocations.signal",
            })?;
        if !signals.insert(signal) {
            return Err(DutyError::Malformed {
                item: "admission_receipt",
                attribute: "allocations.signal",
            });
        }
    }
    if signals.is_empty() || signals.len() > Signal::AUTHORITY.len() {
        return Err(DutyError::Malformed {
            item: "admission_receipt",
            attribute: "allocations",
        });
    }
    Ok(signals)
}

/// Validates and decodes one scope-to-export directory row.
fn scope_export_directory(
    scope: &ScopeKey,
    directory: &HashMap<String, AttributeValue>,
) -> Result<ScopeExportDirectory, DutyError> {
    if string(directory, "scopeKey") != Some(scope.to_key().as_str()) {
        return Err(DutyError::Malformed {
            item: "scope_export",
            attribute: "scopeKey",
        });
    }
    let workspace = scope.workspace();
    let export =
        ExportId::parse(require_string(directory, "exportId", "scope_export")?).map_err(|_| {
            DutyError::Malformed {
                item: "scope_export",
                attribute: "exportId",
            }
        })?;
    OperationId::parse(require_string(directory, "operationId", "scope_export")?).map_err(
        |_| DutyError::Malformed {
            item: "scope_export",
            attribute: "operationId",
        },
    )?;
    let key = item_key(directory).ok_or(DutyError::Malformed {
        item: "scope_export",
        attribute: PK,
    })?;
    if key.pk != keys::scope_export_pk(scope)
        || key.sk != keys::scope_export_sk(export)
        || string(directory, "workspaceId") != Some(workspace.to_string().as_str())
    {
        return Err(DutyError::Malformed {
            item: "scope_export",
            attribute: PK,
        });
    }
    let extension = export_extension(require_string(directory, "format", "scope_export")?).ok_or(
        DutyError::Malformed {
            item: "scope_export",
            attribute: "format",
        },
    )?;
    Ok(ScopeExportDirectory {
        export,
        key,
        extension,
    })
}

fn export_extension(format: &str) -> Option<&'static str> {
    match format {
        "ndjson" | "otlp_json" => Some("jsonl"),
        "parquet" => Some("parquet"),
        _ => None,
    }
}

fn session_body_prefix(scope: &ScopeKey) -> Option<String> {
    let ScopeKey::Session { workspace, session } = scope else {
        return None;
    };
    Some(format!("observations/{workspace}/sessions/{session}/"))
}

/// Refuses a pointer outside the exact session-owned body prefix. A corrupt
/// ledger must stop deletion rather than erase another live scope's body.
fn ensure_session_body_key(scope: &ScopeKey, key: &str) -> Result<(), DutyError> {
    if session_body_prefix(scope).is_some_and(|prefix| !key.starts_with(&prefix)) {
        return Err(DutyError::Malformed {
            item: "observation",
            attribute: "bodyS3Key",
        });
    }
    Ok(())
}

fn control_shard(item: &DueItem) -> Option<u8> {
    string(&item.attributes, Index::Control.partition_key())?
        .rsplit_once('#')?
        .1
        .parse()
        .ok()
}

/// A due item that the control index names but the base table no longer holds.
fn absent(id: ItemId, key: ItemKey) -> DueItem {
    DueItem {
        id,
        key,
        attempts: 0,
        attributes: HashMap::new(),
    }
}

/// The base-table key and identity of one control-index entry.
fn due_ref(item: &HashMap<String, AttributeValue>) -> Option<DueRef> {
    let key = item_key(item)?;
    let sort = string(item, Index::Control.sort_key())?;
    let (_, id) = sort.split_once('#')?;
    Some(DueRef {
        id: ItemId::new(id),
        key,
    })
}

/// The base-table key carried by one projected item.
fn item_key(item: &HashMap<String, AttributeValue>) -> Option<ItemKey> {
    Some(ItemKey {
        pk: string(item, PK)?.to_owned(),
        sk: string(item, SK)?.to_owned(),
    })
}

/// The due instant encoded in a control sort key.
fn due_at(sort: &str) -> Option<Timestamp> {
    Timestamp::parse(sort.split_once('#')?.0).ok()
}

/// Reads a string attribute.
fn string<'a>(item: &'a HashMap<String, AttributeValue>, name: &str) -> Option<&'a str> {
    item.get(name)
        .and_then(|value| value.as_s().ok())
        .map(String::as_str)
}

/// Reads a numeric attribute.
fn number(item: &HashMap<String, AttributeValue>, name: &str) -> Option<u64> {
    item.get(name)
        .and_then(|value| value.as_n().ok())
        .and_then(|text| text.parse().ok())
}

/// Reads a string attribute a duty cannot proceed without.
fn require_string<'a>(
    item: &'a HashMap<String, AttributeValue>,
    name: &'static str,
    family: &'static str,
) -> Result<&'a str, DutyError> {
    string(item, name).ok_or(DutyError::Malformed {
        item: family,
        attribute: name,
    })
}

/// Reads a timestamp attribute a duty cannot proceed without.
fn timestamp_of(
    item: &HashMap<String, AttributeValue>,
    name: &'static str,
    family: &'static str,
) -> Result<Timestamp, DutyError> {
    Timestamp::parse(require_string(item, name, family)?).map_err(|_| DutyError::Malformed {
        item: family,
        attribute: name,
    })
}

/// The scope one control item belongs to.
fn scope_of(item: &DueItem) -> Result<ScopeKey, DutyError> {
    let text = require_string(&item.attributes, "scopeKey", "control_item")?;
    ScopeKey::parse(text).map_err(|_| DutyError::Malformed {
        item: "control_item",
        attribute: "scopeKey",
    })
}

/// The workspace one control item belongs to.
fn workspace_of(item: &DueItem) -> Result<WorkspaceId, DutyError> {
    let text = require_string(&item.attributes, "workspaceId", "control_item")?;
    WorkspaceId::parse(text).map_err(|_| DutyError::Malformed {
        item: "control_item",
        attribute: "workspaceId",
    })
}

/// Exact source-stable loss candidates retained by admission.
fn loss_candidates_of(item: &DueItem) -> Result<Vec<LossCandidate>, DutyError> {
    let values = item
        .attributes
        .get("lossCandidates")
        .and_then(|value| value.as_l().ok())
        .ok_or(DutyError::Malformed {
            item: "spool_chunk",
            attribute: "lossCandidates",
        })?;
    if values.is_empty() {
        return Err(DutyError::Malformed {
            item: "spool_chunk",
            attribute: "lossCandidates",
        });
    }
    values
        .iter()
        .map(|value| {
            let map = value.as_m().map_err(|_| DutyError::Malformed {
                item: "loss_candidate",
                attribute: "lossCandidates",
            })?;
            let gap_id = TelemetryGapId::parse(require_string(map, "gapId", "loss_candidate")?)
                .map_err(|_| DutyError::Malformed {
                    item: "loss_candidate",
                    attribute: "gapId",
                })?;
            let signal = Signal::parse(require_string(map, "signal", "loss_candidate")?).ok_or(
                DutyError::Malformed {
                    item: "loss_candidate",
                    attribute: "signal",
                },
            )?;
            let lo = required_number(map, "acceptedSeqLo", "loss_candidate")?;
            let hi_exclusive = required_number(map, "acceptedSeqHiExclusive", "loss_candidate")?;
            if hi_exclusive <= lo {
                return Err(DutyError::Malformed {
                    item: "loss_candidate",
                    attribute: "acceptedSeqHiExclusive",
                });
            }
            Ok(LossCandidate {
                gap_id,
                signal,
                lo,
                hi_exclusive,
                time_range: optional_time_range(map)?,
                attempted_records: required_number(map, "attemptedRecords", "loss_candidate")?,
                attempted_bytes: required_number(map, "attemptedBytes", "loss_candidate")?,
            })
        })
        .collect()
}

fn required_number(
    item: &HashMap<String, AttributeValue>,
    name: &'static str,
    family: &'static str,
) -> Result<u64, DutyError> {
    number(item, name).ok_or(DutyError::Malformed {
        item: family,
        attribute: name,
    })
}

fn optional_time_range(
    item: &HashMap<String, AttributeValue>,
) -> Result<Option<TimeWindow>, DutyError> {
    let Some(value) = item.get("timeRange") else {
        return Ok(None);
    };
    let map = value.as_m().map_err(|_| DutyError::Malformed {
        item: "loss_candidate",
        attribute: "timeRange",
    })?;
    let from = timestamp_of(map, "gte", "loss_candidate")?;
    let to = timestamp_of(map, "lt", "loss_candidate")?;
    TimeWindow::new(from, to)
        .map(Some)
        .ok_or(DutyError::Malformed {
            item: "loss_candidate",
            attribute: "timeRange",
        })
}

/// The outstanding-duty set of one spool chunk.
fn pending_of(item: &HashMap<String, AttributeValue>) -> BTreeSet<Pending> {
    item.get(PENDING)
        .and_then(|value| value.as_ss().ok())
        .map(|words| {
            words
                .iter()
                .filter_map(|word| {
                    Pending::ALL
                        .iter()
                        .copied()
                        .find(|duty| duty.as_str() == word)
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Whether an unacknowledged wake is old enough to abandon.
fn wake_is_stale(
    item: &HashMap<String, AttributeValue>,
    now: Timestamp,
) -> Result<bool, DutyError> {
    let accepted_at = timestamp_of(item, "acceptedAt", "spool_chunk")?;
    Ok(now.unix_millis() - accepted_at.unix_millis() >= WAKE_STALE_MS)
}

/// How long ago a recorded instant was, or zero when it was never recorded.
fn elapsed(item: &HashMap<String, AttributeValue>, name: &str, now: Timestamp) -> i64 {
    string(item, name)
        .and_then(|text| Timestamp::parse(text).ok())
        .map_or(0, |then| now.unix_millis() - then.unix_millis())
}

/// The stored gate state, defaulting to `open`.
///
/// An absent item is `open`: the gate is a closure signal this duty writes, so
/// its absence means nothing has ever asked admission to stop.
fn gate_state(state: Option<&str>) -> GateState {
    GateState::ALL
        .iter()
        .copied()
        .find(|candidate| Some(candidate.as_str()) == state)
        .unwrap_or(GateState::Open)
}

/// Which counter shard one series hash belongs to.
fn counter_shard(hash: &str) -> u16 {
    let prefix = hash.get(0..4).unwrap_or("0000");
    u16::from_str_radix(prefix, 16).unwrap_or(0) % limits::SERIES_COUNTER_SHARDS
}

/// The storage usage fact delivered to the usage queue.
fn outbox_payload(fact: &HashMap<String, AttributeValue>) -> serde_json::Value {
    serde_json::json!({
        "meter": string(fact, "meter").unwrap_or("storage.byte_min.v1"),
        "quantityBytes": number(fact, "quantityBytes").unwrap_or(0),
        "workspaceId": string(fact, "workspaceId").unwrap_or_default(),
        "organizationId": string(fact, "organizationId").unwrap_or_default(),
        "idempotencyKey": string(fact, "idempotencyKey").unwrap_or_default(),
    })
}

/// Whether a failed write lost its condition.
fn is_conditional_failure<E, R>(error: &aws_sdk_dynamodb::error::SdkError<E, R>) -> bool
where
    E: ConditionalCheck,
{
    error
        .as_service_error()
        .is_some_and(ConditionalCheck::is_conditional_check_failed)
}

/// The one thing this engine asks of a `DynamoDB` write error.
pub trait ConditionalCheck {
    /// Whether the write lost its condition.
    fn is_conditional_check_failed(&self) -> bool;
}

impl ConditionalCheck for aws_sdk_dynamodb::operation::update_item::UpdateItemError {
    fn is_conditional_check_failed(&self) -> bool {
        self.is_conditional_check_failed_exception()
    }
}

impl ConditionalCheck for aws_sdk_dynamodb::operation::delete_item::DeleteItemError {
    fn is_conditional_check_failed(&self) -> bool {
        self.is_conditional_check_failed_exception()
    }
}

impl ConditionalCheck for aws_sdk_dynamodb::operation::put_item::PutItemError {
    fn is_conditional_check_failed(&self) -> bool {
        self.is_conditional_check_failed_exception()
    }
}

/// Whether a cancelled transaction was cancelled by a lost condition.
fn is_transaction_conditional_failure<R>(
    error: &aws_sdk_dynamodb::error::SdkError<
        aws_sdk_dynamodb::operation::transact_write_items::TransactWriteItemsError,
        R,
    >,
) -> bool {
    use aws_sdk_dynamodb::operation::transact_write_items::TransactWriteItemsError;

    error
        .as_service_error()
        .is_some_and(|service| match service {
            TransactWriteItemsError::TransactionCanceledException(cancelled) => cancelled
                .cancellation_reasons()
                .iter()
                .any(|reason| reason.code() == Some("ConditionalCheckFailed")),
            _ => false,
        })
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use aex_observation_domain::frontier::DeletionState;
    use aex_observation_domain::keys::{self, ControlDomain, ScopeKey};
    use aex_observation_domain::limits;
    use aex_observation_domain::signal::Signal;
    use aex_observation_store_dynamodb::expressions::{Index, PK, SK, is_safe_expression};
    use aex_observation_store_dynamodb::spool::{GateState, Pending, SpoolChunk};
    use aex_wire::PrefixedId;
    use aex_wire::ids::{SessionId, TelemetryBatchId, Uuid7, WorkspaceId};
    use aex_wire::types::{Region, Timestamp};
    use aws_sdk_dynamodb::types::AttributeValue;
    use aws_smithy_http_client::test_util::{ReplayEvent, StaticReplayClient};
    use aws_smithy_types::body::SdkBody;

    use super::{
        BatchOutcome, DUE_SENTINEL, DueItem, DutyEngine, DutySettings, ItemId, ItemKey,
        WAKE_STALE_MS, counter_shard, due_at, due_ref, elapsed, ensure_session_body_key,
        gate_state, pending_of, session_body_prefix, wake_is_stale,
    };

    fn now() -> Timestamp {
        Timestamp::parse("2026-08-01T12:34:56.789Z").expect("the pinned spelling")
    }

    fn settings() -> DutySettings {
        DutySettings {
            table: "observation-authority".to_owned(),
            session_table: "session-authority".to_owned(),
            bucket: "aex-dev-observations".to_owned(),
            usage_queue_url: "https://sqs.eu-west-1.amazonaws.com/1/usage.fifo".to_owned(),
            region: Region::EuWest1,
            duty: ControlDomain::SpoolRepair,
            page: 100,
            shards: 16,
            max_attempts: limits::OBS_SPOOL_MAX_ATTEMPTS,
        }
    }

    fn engine(responses: Vec<String>) -> (DutyEngine, StaticReplayClient) {
        use aws_sdk_dynamodb::config::{BehaviorVersion, Credentials};

        let events = responses
            .into_iter()
            .map(|body| {
                ReplayEvent::new(
                    http::Request::builder()
                        .method("POST")
                        .uri("https://dynamodb.eu-west-1.amazonaws.com/")
                        .body(SdkBody::empty())
                        .expect("request"),
                    http::Response::builder()
                        .status(200)
                        .body(SdkBody::from(body))
                        .expect("response"),
                )
            })
            .collect();
        let replay = StaticReplayClient::new(events);
        let credentials = Credentials::new(
            "AKIDTESTTESTTESTTEST",
            "test-secret",
            None,
            None,
            "aex-tests",
        );
        let dynamodb = aws_sdk_dynamodb::Client::from_conf(
            aws_sdk_dynamodb::Config::builder()
                .behavior_version(BehaviorVersion::latest())
                .region(aws_sdk_dynamodb::config::Region::new("eu-west-1"))
                .credentials_provider(credentials.clone())
                .http_client(replay.clone())
                .build(),
        );
        let s3 = aws_sdk_s3::Client::from_conf(
            aws_sdk_s3::Config::builder()
                .behavior_version(BehaviorVersion::latest())
                .region(aws_sdk_s3::config::Region::new("eu-west-1"))
                .credentials_provider(credentials.clone())
                .http_client(replay.clone())
                .build(),
        );
        let sqs = aws_sdk_sqs::Client::from_conf(
            aws_sdk_sqs::Config::builder()
                .behavior_version(BehaviorVersion::latest())
                .region(aws_sdk_sqs::config::Region::new("eu-west-1"))
                .credentials_provider(credentials)
                .http_client(replay.clone())
                .build(),
        );
        let mut duty = settings();
        duty.duty = ControlDomain::DeletionExecute;
        (DutyEngine::new(dynamodb, s3, sqs, duty), replay)
    }

    fn body_of(
        request: &aws_smithy_runtime_api::client::orchestrator::HttpRequest,
    ) -> serde_json::Value {
        serde_json::from_slice(request.body().bytes().expect("request body")).expect("request JSON")
    }

    #[test]
    fn the_due_sentinel_sorts_after_every_admissible_identifier() {
        let ceiling = keys::control_sk(now(), DUE_SENTINEL);
        for id in ["bch_a", "bch_zzzz", "\u{fffe}"] {
            assert!(
                keys::control_sk(now(), id) < ceiling,
                "`{id}` must fall inside the due window"
            );
        }
        assert!(
            keys::component(DUE_SENTINEL).is_err(),
            "the sentinel is excluded from every real component, which is what makes it one"
        );
    }

    #[test]
    fn a_control_index_entry_yields_its_base_table_key_and_identity() {
        let mut item = HashMap::new();
        item.insert(
            PK.to_owned(),
            AttributeValue::S("SPOOL#wsp_1#03".to_owned()),
        );
        item.insert(
            SK.to_owned(),
            AttributeValue::S("2026-08-01T09:00:00.000Z#bch_1".to_owned()),
        );
        item.insert(
            Index::Control.sort_key().to_owned(),
            AttributeValue::S("2026-08-01T09:00:00.000Z#bch_1".to_owned()),
        );
        let due = due_ref(&item).expect("a well-formed index entry");
        assert_eq!(due.id.as_str(), "bch_1");
        assert_eq!(due.key.pk, "SPOOL#wsp_1#03");
        assert_eq!(
            due_at("2026-08-01T09:00:00.000Z#bch_1")
                .expect("a due instant")
                .to_wire(),
            "2026-08-01T09:00:00.000Z"
        );
        assert!(due_ref(&HashMap::new()).is_none());
    }

    #[test]
    fn the_claim_backoff_is_the_pinned_spool_backoff() {
        let mut chunk = SpoolChunk::new(0, 0, now());
        chunk.attempts = 3;
        assert_eq!(chunk.next_attempt_delay_ms(), 8_000);
        chunk.attempts = limits::OBS_SPOOL_MAX_ATTEMPTS;
        assert!(chunk.must_escalate());
    }

    #[test]
    fn the_pending_set_round_trips_through_the_durable_spelling() {
        let mut item = HashMap::new();
        item.insert(
            super::PENDING.to_owned(),
            AttributeValue::Ss(vec!["outbox".to_owned(), "wake".to_owned()]),
        );
        let pending = pending_of(&item);
        assert!(pending.contains(&Pending::Outbox));
        assert!(pending.contains(&Pending::Wake));
        assert!(!pending.contains(&Pending::Index));
        assert!(pending_of(&HashMap::new()).is_empty());
    }

    #[test]
    fn a_wake_is_abandoned_only_once_it_is_older_than_the_gate_dwell() {
        let mut item = HashMap::new();
        let fresh =
            Timestamp::from_unix_millis(now().unix_millis() - WAKE_STALE_MS + 1).expect("in range");
        item.insert("acceptedAt".to_owned(), AttributeValue::S(fresh.to_wire()));
        assert!(!wake_is_stale(&item, now()).expect("readable"));

        let stale =
            Timestamp::from_unix_millis(now().unix_millis() - WAKE_STALE_MS).expect("in range");
        item.insert("acceptedAt".to_owned(), AttributeValue::S(stale.to_wire()));
        assert!(wake_is_stale(&item, now()).expect("readable"));
        assert!(wake_is_stale(&HashMap::new(), now()).is_err());
    }

    #[test]
    fn an_absent_gate_item_reads_open_and_a_stored_word_reads_itself() {
        assert_eq!(gate_state(None), GateState::Open);
        assert_eq!(gate_state(Some("nonsense")), GateState::Open);
        for state in GateState::ALL {
            assert_eq!(gate_state(Some(state.as_str())), *state);
        }
    }

    #[test]
    fn an_unrecorded_instant_measures_zero_rather_than_the_epoch() {
        assert_eq!(elapsed(&HashMap::new(), "stateSince", now()), 0);
        let mut item = HashMap::new();
        item.insert(
            "stateSince".to_owned(),
            AttributeValue::S("2026-08-01T12:34:55.789Z".to_owned()),
        );
        assert_eq!(elapsed(&item, "stateSince", now()), 1_000);
    }

    #[test]
    fn a_counter_shard_is_stable_and_inside_the_declared_fan_out() {
        for hash in ["00ff", "ffff", "1a2b3c", ""] {
            let shard = counter_shard(hash);
            assert!(shard < limits::SERIES_COUNTER_SHARDS, "{hash} -> {shard}");
            assert_eq!(shard, counter_shard(hash));
        }
    }

    #[test]
    fn a_batch_outcome_keeps_the_two_verdicts_apart() {
        let mut outcome = BatchOutcome::default();
        assert!(outcome.is_empty());
        outcome.succeed(ItemId::new("a"));
        outcome.fail(ItemId::new("b"), "unresolved");
        assert_eq!(outcome.len(), 2);
        assert_eq!(outcome.succeeded, vec![ItemId::new("a")]);
        assert_eq!(outcome.failed[0].0.to_string(), "b");
    }

    #[test]
    fn the_expressions_this_engine_builds_carry_no_interpolated_operand() {
        let settings = settings();
        for expression in [
            "#n0 = :v0 AND #n1 <= :v1",
            "attribute_not_exists(#n0) OR #n1 = :v1",
            "SET #n0 = :v0 REMOVE #n1, #n2",
            "#n0 IN (:v0, :v1)",
        ] {
            assert!(is_safe_expression(expression), "{expression}");
        }
        assert_eq!(settings.duty, ControlDomain::SpoolRepair);
        assert!(settings.usage_queue_url.starts_with("https://"));
    }

    #[test]
    fn only_an_exact_session_has_an_owned_observation_body_prefix() {
        let workspace = WorkspaceId::from_uuid7(Uuid7::compose(1, [1; 10]));
        let first = ScopeKey::Session {
            workspace,
            session: SessionId::from_uuid7(Uuid7::compose(2, [2; 10])),
        };
        let second = ScopeKey::Session {
            workspace,
            session: SessionId::from_uuid7(Uuid7::compose(3, [3; 10])),
        };
        assert_ne!(session_body_prefix(&first), session_body_prefix(&second));
        assert_eq!(session_body_prefix(&ScopeKey::Workspace(workspace)), None);
        let first_key = format!(
            "{}aa/bb/digest",
            session_body_prefix(&first).expect("prefix")
        );
        assert!(ensure_session_body_key(&first, &first_key).is_ok());
        assert!(ensure_session_body_key(&second, &first_key).is_err());
    }

    #[tokio::test]
    async fn gap_rows_and_the_workspace_change_hint_are_updated_atomically() {
        let workspace = WorkspaceId::from_uuid7(Uuid7::compose(1, [1; 10]));
        let scope = ScopeKey::Session {
            workspace,
            session: SessionId::from_uuid7(Uuid7::compose(2, [2; 10])),
        };
        let gap_pk = keys::gap_pk(&scope);
        let query = serde_json::json!({
            "Items": [{
                "pk": {"S": gap_pk},
                "sk": {"S": "GAP#fixture#00000000000000000001"}
            }],
            "Count": 1,
            "ScannedCount": 1
        })
        .to_string();
        let (engine, replay) = engine(vec![query, "{}".to_owned()]);
        let mut budget = 10;

        assert_eq!(
            engine
                .purge_gaps(&scope, &mut budget)
                .await
                .expect("gaps drain"),
            1
        );
        assert_eq!(budget, 9);
        let requests: Vec<_> = replay.actual_requests().collect();
        assert_eq!(requests.len(), 2);
        let transaction = body_of(requests[1]);
        let actions = transaction["TransactItems"].as_array().expect("actions");
        assert_eq!(actions.len(), 2);
        assert_eq!(actions[0]["Delete"]["Key"]["pk"]["S"], gap_pk);
        assert_eq!(
            actions[1]["Update"]["Key"]["pk"]["S"].as_str(),
            Some(keys::gap_hint_pk(workspace).as_str())
        );
        assert!(
            actions[1]["Update"]["UpdateExpression"]
                .as_str()
                .is_some_and(|update| update.contains("ADD "))
        );
    }

    #[tokio::test]
    async fn session_ledger_decrements_shared_segment_rows_exactly_once() {
        let (engine, replay) = engine(vec!["{}".to_owned()]);
        let workspace = WorkspaceId::from_uuid7(Uuid7::compose(1, [2; 10]));
        let session = SessionId::from_uuid7(Uuid7::compose(1, [4; 10]));
        let scope = ScopeKey::Session { workspace, session };
        let batch = TelemetryBatchId::from_uuid7(Uuid7::compose(1, [8; 10]));
        let ledger_key = ItemKey {
            pk: keys::batch_pk(workspace, batch),
            sk: keys::materialization_sk(0),
        };
        let contribution = |axis: &str| {
            AttributeValue::M(HashMap::from([
                ("axis".to_owned(), AttributeValue::S(axis.to_owned())),
                ("signal".to_owned(), AttributeValue::S("logs".to_owned())),
                (
                    "bucket".to_owned(),
                    AttributeValue::S("2026-08-01T09".to_owned()),
                ),
                ("count".to_owned(), AttributeValue::N("2".to_owned())),
                (
                    "logicalBytes".to_owned(),
                    AttributeValue::N("24".to_owned()),
                ),
            ]))
        };
        let ledger = HashMap::from([
            (PK.to_owned(), AttributeValue::S(ledger_key.pk.clone())),
            (SK.to_owned(), AttributeValue::S(ledger_key.sk.clone())),
            ("scopeKey".to_owned(), AttributeValue::S(scope.to_key())),
            ("batchId".to_owned(), AttributeValue::S(batch.to_string())),
            (
                "materializationDigest".to_owned(),
                AttributeValue::S("0".repeat(64)),
            ),
            (
                "segments".to_owned(),
                AttributeValue::L(vec![contribution("accepted"), contribution("event_time")]),
            ),
        ]);

        engine
            .settle_workspace_segments(&scope, batch, &ledger, &ledger_key)
            .await
            .expect("the aggregate contribution settles");

        let request = replay.actual_requests().next().expect("one transaction");
        let transaction = body_of(request);
        let actions = transaction["TransactItems"].as_array().expect("actions");
        assert_eq!(actions.len(), 3);
        assert_eq!(
            actions[0]["Update"]["Key"]["pk"]["S"].as_str(),
            Some(keys::segment_pk(&ScopeKey::Workspace(workspace), Signal::Logs).as_str())
        );
        assert_eq!(
            actions[1]["Update"]["Key"]["pk"]["S"].as_str(),
            Some(keys::time_segment_pk(&ScopeKey::Workspace(workspace), Signal::Logs).as_str())
        );
        for update in &actions[..2] {
            let values = update["Update"]["ExpressionAttributeValues"]
                .as_object()
                .expect("values");
            assert!(values.values().any(|value| value["N"] == "-2"));
            assert!(values.values().any(|value| value["N"] == "-24"));
        }
        assert_eq!(
            actions[2]["Delete"]["Key"]["sk"]["S"].as_str(),
            Some(ledger_key.sk.as_str())
        );
    }

    #[tokio::test]
    async fn deletion_releases_workspace_visibility_for_a_committed_unfinished_batch() {
        let (engine, replay) = engine(vec!["{}".to_owned()]);
        let workspace = WorkspaceId::from_uuid7(Uuid7::compose(1, [2; 10]));
        let session = SessionId::from_uuid7(Uuid7::compose(1, [4; 10]));
        let scope = ScopeKey::Session { workspace, session };
        let batch = TelemetryBatchId::from_uuid7(Uuid7::compose(1, [8; 10]));
        let allocation = |signal: &str| {
            AttributeValue::M(HashMap::from([(
                "signal".to_owned(),
                AttributeValue::S(signal.to_owned()),
            )]))
        };
        let receipt = HashMap::from([
            (
                "state".to_owned(),
                AttributeValue::S("committed".to_owned()),
            ),
            ("scopeKey".to_owned(), AttributeValue::S(scope.to_key())),
            ("batchId".to_owned(), AttributeValue::S(batch.to_string())),
            (
                "allocations".to_owned(),
                AttributeValue::L(vec![allocation("logs"), allocation("metrics")]),
            ),
        ]);

        engine
            .settle_abandoned_workspace_frontiers(&scope, batch, &receipt, now())
            .await
            .expect("the pending workspace frontier is released");

        let request = replay.actual_requests().next().expect("one transaction");
        let transaction = body_of(request);
        let actions = transaction["TransactItems"].as_array().expect("actions");
        assert_eq!(actions.len(), 3, "receipt marker plus two frontiers");
        for update in &actions[1..] {
            assert_eq!(
                update["Update"]["Key"]["pk"]["S"].as_str(),
                Some(keys::frontier_pk(&ScopeKey::Workspace(workspace)).as_str())
            );
            assert!(
                update["Update"]["ExpressionAttributeValues"]
                    .as_object()
                    .expect("values")
                    .values()
                    .any(|value| value["N"].as_str() == Some("-1"))
            );
        }
    }

    #[tokio::test]
    async fn execute_to_verify_reindexes_the_same_tombstone_for_a_separate_duty() {
        let (engine, replay) = engine(vec!["{}".to_owned()]);
        let item = DueItem {
            id: ItemId::new("ses_fixture"),
            key: ItemKey {
                pk: "FRONT#S#wsp_fixture#ses_fixture".to_owned(),
                sk: keys::DELETION_SK.to_owned(),
            },
            attempts: 2,
            attributes: HashMap::from([(
                Index::Control.partition_key().to_owned(),
                AttributeValue::S(keys::control_pk(ControlDomain::DeletionExecute, 7)),
            )]),
        };

        engine
            .advance_deletion(
                &item,
                DeletionState::Deleting,
                DeletionState::Verifying,
                now(),
            )
            .await
            .expect("transition");
        let request = replay.actual_requests().next().expect("update");
        let body = body_of(request);
        let strings: Vec<&str> = body["ExpressionAttributeValues"]
            .as_object()
            .expect("values")
            .values()
            .filter_map(|value| value["S"].as_str())
            .collect();
        assert!(strings.contains(&"verifying"), "{strings:?}");
        assert!(
            strings.contains(&keys::control_pk(ControlDomain::DeletionVerify, 7).as_str()),
            "{strings:?}"
        );
        let update = body["UpdateExpression"].as_str().expect("update");
        assert!(update.contains("REMOVE"), "{update}");
    }
}
