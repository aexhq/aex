//! The three event modes every usage worker serves.
//!
//! One binary, three modes (`U-09`): `stream` from the authority's own
//! `DynamoDB` stream, `sweep` from a one-minute schedule, and `receipt` from the
//! category's settlement queue. Area 9's deployable inventory is frozen at three
//! usage workers, all three modes need identical credentials, and all three sit
//! inside the identical category boundary — so they are one role rather than a
//! fourth deployable.
//!
//! This module is category-generic over [`crate::ports`]. Each worker binary
//! binds it to exactly one authority adapter, which is what makes "no worker can
//! write a sibling authority" a link-graph fact rather than a review promise:
//! there is no code path here that names a table at all.
//!
//! # The partial-batch rule
//!
//! On any failure the batch reports the **lowest** failed record's identifier.
//! Lambda then redelivers that record and everything after it. Reporting only
//! the individual failures would checkpoint past a gap, and a gap in a
//! contiguous sequence is money evidence that never gets folded.

#[cfg(test)]
mod tests;

use std::fmt;

use aex_usage_domain::fact::UsageFact;
use aex_usage_domain::frontier::{AcceptedSequence, PoisonReason};
use aex_usage_domain::wire_pending::{Timestamp, WorkspaceId};

use crate::ports::{AuthorityStore, Clock, ProjectionStore, RatingQueue, SettlementReceipt};
use crate::use_cases::{
    ApplyReceipt, ProjectCategory, PublishOutbox, Published, QuarantineFact, Settled, SweepOutbox,
    SweepReport, UseCaseError,
};

/// How many shards the outbox due index is spread over.
pub const OUTBOX_SHARDS: u8 = 16;

/// Whether a deployment may produce a chargeable rating request.
///
/// `A11-METER` makes the charging gate a configuration a deployment cannot half
/// satisfy, which is why the two arms are one value rather than two booleans:
/// there is no representable state where both are set.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BillingMode {
    /// Facts are admitted, projected and published to the shadow rating queue.
    /// Finance posts zero. This is the default until `METER-03` passes.
    Shadow,
    /// Facts are published to the live rating queue.
    Active,
}

impl BillingMode {
    /// The stable identifier a report and a log line carry.
    #[must_use]
    pub const fn id(self) -> &'static str {
        match self {
            Self::Shadow => "shadow",
            Self::Active => "active",
        }
    }
}

impl fmt::Display for BillingMode {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.id())
    }
}

/// The named limits a worker runs under.
///
/// No value here has a default. A defaulted money-relevant limit is a silent
/// policy decision, and every one of these is a policy decision.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WorkerLimits {
    /// How long an undelivered outbox row waits before the sweep republishes it.
    pub outbox_republish_after_ms: u64,
    /// The largest number of rows one sweep reads per shard per run.
    pub sweep_page: usize,
    /// The attempt count above which a row alarms.
    pub outbox_attempt_alarm: u32,
    /// The undelivered age above which the backlog alarms.
    pub outbox_age_alarm_ms: u64,
    /// The undelivered count above which the backlog alarms.
    pub outbox_backlog_ceiling: u64,
}

/// One record as the stream handed it over.
///
/// The decode happens in the adapter, so this carries either the decoded fact or
/// the reason it could not be decoded. Keeping the failure typed is what stops a
/// permanent decode failure being retried as if it were transient.
#[derive(Debug, Clone)]
pub struct StreamRecord {
    /// The stream's own identifier for this record, reported in a partial batch.
    pub identifier: String,
    /// The decoded fact, or why it could not be decoded.
    pub fact: Result<UsageFact, UndecodableRecord>,
}

/// A stream record that could not be decoded into a fact.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UndecodableRecord {
    /// The workspace the key names, when the key itself decoded.
    pub workspace: Option<WorkspaceId>,
    /// The sequence the key names, when the key itself decoded.
    pub sequence: Option<AcceptedSequence>,
    /// What went wrong, with no quantity, evidence or attribution in it.
    pub detail: String,
}

/// What one stream batch did.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct StreamReport {
    /// Facts folded into the projection.
    pub folded: u64,
    /// Facts whose fold was already covered and were reconciled.
    pub reconciled: u64,
    /// Facts already past the frontier.
    pub already_projected: u64,
    /// Facts delivered to central.
    pub published: u64,
    /// Facts projected but not delivered, because central refused.
    pub deferred: u64,
    /// Records parked behind a quarantine.
    pub quarantined: u64,
    /// The identifiers Lambda must redeliver, lowest failure first.
    pub failures: Vec<String>,
}

impl StreamReport {
    /// Whether the batch is fully checkpointed.
    #[must_use]
    pub fn is_clean(&self) -> bool {
        self.failures.is_empty()
    }
}

/// What one receipt batch did.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ReceiptReport {
    /// Receipts that advanced the settled frontier.
    pub advanced: u64,
    /// Receipts stored but parked behind a gap.
    pub parked: u64,
    /// Receipts already covered.
    pub already_settled: u64,
    /// Receipts that disagreed with their fact and parked it.
    pub quarantined: u64,
    /// The identifiers the queue must redeliver.
    pub failures: Vec<String>,
}

/// One settlement receipt as the queue handed it over.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReceiptRecord {
    /// The queue's own identifier for this message.
    pub identifier: String,
    /// The receipt.
    pub receipt: SettlementReceipt,
}

/// The category-generic worker every usage binary composes.
#[derive(Debug)]
pub struct UsageWorker<A, P, Q, C> {
    project: ProjectCategory<A, P>,
    publish: PublishOutbox<A, Q>,
    settle: ApplyReceipt<A, P>,
    sweep: SweepOutbox<A, Q, C>,
    quarantine: QuarantineFact<A>,
    clock: C,
    limits: WorkerLimits,
    mode: BillingMode,
}

impl<A, P, Q, C> UsageWorker<A, P, Q, C>
where
    A: AuthorityStore + Clone,
    P: ProjectionStore + Clone,
    Q: RatingQueue + Clone,
    C: Clock + Clone,
{
    /// Composes the worker over one authority, the shared projection and the
    /// central queue.
    pub fn new(
        authority: A,
        projection: P,
        queue: Q,
        clock: C,
        limits: WorkerLimits,
        mode: BillingMode,
    ) -> Self {
        Self {
            project: ProjectCategory::new(authority.clone(), projection.clone()),
            publish: PublishOutbox::new(authority.clone(), queue.clone()),
            settle: ApplyReceipt::new(authority.clone(), projection),
            sweep: SweepOutbox::new(authority.clone(), queue, clock.clone()),
            quarantine: QuarantineFact::new(authority),
            clock,
            limits,
            mode,
        }
    }

    /// Which billing mode this composition runs in.
    #[must_use]
    pub const fn mode(&self) -> BillingMode {
        self.mode
    }

    /// The limits this composition runs under.
    #[must_use]
    pub const fn limits(&self) -> WorkerLimits {
        self.limits
    }

    /// Handles one stream batch.
    ///
    /// # Errors
    ///
    /// Only a failure that cannot be attributed to one record: a quarantine
    /// write that itself failed leaves the batch unable to make progress
    /// honestly, so it is raised rather than swallowed.
    pub async fn handle_stream(
        &self,
        records: &[StreamRecord],
    ) -> Result<StreamReport, UseCaseError> {
        let mut report = StreamReport::default();
        for record in records {
            match self.handle_record(record, &mut report).await? {
                RecordVerdict::Done => {}
                RecordVerdict::Retry => {
                    // The lowest failure wins: everything at or after it is
                    // redelivered, so a gap is never checkpointed past.
                    if report.failures.is_empty() {
                        report.failures.push(record.identifier.clone());
                    }
                    break;
                }
            }
        }
        Ok(report)
    }

    /// Handles one record, folding the outcome into the batch report.
    async fn handle_record(
        &self,
        record: &StreamRecord,
        report: &mut StreamReport,
    ) -> Result<RecordVerdict, UseCaseError> {
        let fact = match &record.fact {
            Ok(fact) => fact,
            Err(undecodable) => {
                // A decode failure is permanent. Retrying it forever is the
                // poison loop; skipping it loses money evidence. Parking is the
                // only honest third option.
                if let (Some(workspace), Some(sequence)) =
                    (&undecodable.workspace, undecodable.sequence)
                {
                    self.quarantine
                        .execute(
                            workspace,
                            sequence,
                            PoisonReason::Undecodable,
                            &undecodable.detail,
                        )
                        .await?;
                }
                report.quarantined += 1;
                return Ok(RecordVerdict::Retry);
            }
        };

        match self.project.execute(fact).await {
            Ok(outcome) => match outcome {
                crate::use_cases::Projected::Folded => report.folded += 1,
                crate::use_cases::Projected::Reconciled => report.reconciled += 1,
                crate::use_cases::Projected::AlreadyProjected => {
                    report.already_projected += 1;
                }
            },
            Err(error) => return self.park(fact, error, report).await,
        }

        match self.publish.execute(fact).await {
            Ok(Published::Delivered) => report.published += 1,
            // A central outage must not stall the projection, so the record is
            // checkpointed and the sweep owns delivery from here.
            Ok(Published::Deferred) => report.deferred += 1,
            Ok(Published::AlreadyPublished) => {}
            Err(error) => return self.park(fact, error, report).await,
        }
        Ok(RecordVerdict::Done)
    }

    /// Parks a record whose failure is terminal, or asks for a redelivery.
    async fn park(
        &self,
        fact: &UsageFact,
        error: UseCaseError,
        report: &mut StreamReport,
    ) -> Result<RecordVerdict, UseCaseError> {
        if let Some(reason) = error.poison() {
            self.quarantine
                .execute(
                    &fact.workspace,
                    fact.accepted_sequence,
                    reason,
                    &error.to_string(),
                )
                .await?;
            report.quarantined += 1;
        }
        Ok(RecordVerdict::Retry)
    }

    /// Sweeps every shard once.
    ///
    /// # Errors
    ///
    /// [`UseCaseError`] from the underlying sweep. A retryable queue failure is
    /// counted as deferred rather than raised, so a central outage produces a
    /// report rather than a failed invocation.
    pub async fn handle_sweep(&self) -> Result<SweepReport, UseCaseError> {
        let now = self.clock.now();
        let older_than = older_than(now, self.limits.outbox_republish_after_ms);
        let mut total = SweepReport::default();
        for shard in 0..OUTBOX_SHARDS {
            let report = self
                .sweep
                .execute(
                    shard,
                    older_than,
                    self.limits.sweep_page,
                    self.limits.outbox_attempt_alarm,
                )
                .await?;
            total.inspected += report.inspected;
            total.republished += report.republished;
            total.deferred += report.deferred;
            total.alarming += report.alarming;
            total.unpublishable += report.unpublishable;
            total.oldest_age_ms = total.oldest_age_ms.max(report.oldest_age_ms);
        }
        Ok(total)
    }

    /// Whether one sweep's result crosses a backlog alarm.
    #[must_use]
    pub const fn backlog_alarms(&self, report: &SweepReport) -> bool {
        report.oldest_age_ms > self.limits.outbox_age_alarm_ms
            || report.inspected > self.limits.outbox_backlog_ceiling
    }

    /// Handles one settlement receipt batch.
    ///
    /// Unlike the stream, receipts have no ordering guarantee, so one failing
    /// message fails only itself: central truth never waits on this
    /// acknowledgement and a lost receipt is replayed from the central outbox.
    ///
    /// # Errors
    ///
    /// [`UseCaseError`] only for a failure that cannot be attributed to one
    /// message.
    pub async fn handle_receipts(
        &self,
        records: &[ReceiptRecord],
    ) -> Result<ReceiptReport, UseCaseError> {
        let mut report = ReceiptReport::default();
        for record in records {
            match self.settle.execute(&record.receipt).await {
                Ok(Settled::Advanced) => report.advanced += 1,
                Ok(Settled::Parked) => report.parked += 1,
                Ok(Settled::AlreadySettled) => report.already_settled += 1,
                Ok(Settled::Quarantined) => report.quarantined += 1,
                Err(error) if error.poison().is_some() => {
                    report.quarantined += 1;
                    report.failures.push(record.identifier.clone());
                }
                Err(_) => report.failures.push(record.identifier.clone()),
            }
        }
        Ok(report)
    }
}

/// What the batch loop should do with a record.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RecordVerdict {
    /// Checkpoint past it.
    Done,
    /// Redeliver it and everything after it.
    Retry,
}

/// The instant a sweep considers a row overdue at.
fn older_than(now: Timestamp, republish_after_ms: u64) -> Timestamp {
    let millis = now
        .unix_millis()
        .saturating_sub(i64::try_from(republish_after_ms).unwrap_or(i64::MAX));
    Timestamp::from_unix_millis(millis.max(0)).unwrap_or(now)
}
