//! The usage use cases: admit, project, publish, settle, sweep, quarantine and
//! rebuild.
//!
//! Each one is a small orchestration over [`crate::ports`]. The ordering between
//! them is the load-bearing part and is stated once here:
//!
//! ```text
//! admit  ->  project  ->  publish  ->  settle
//!            (frontier.projected)      (frontier.settled)
//!                        (frontier.published)
//! ```
//!
//! A fact is never published before it is projected, which is what makes
//! `published <= projected` true by construction rather than by convention. The
//! customer's projection may lead settlement, and the coverage vector says so
//! rather than hiding it.
//!
//! Two failures are deliberately *not* errors:
//!
//! - a redelivered fact whose sequence is already folded is a no-op, because a
//!   `DynamoDB` stream redelivery is an expected condition; and
//! - a central outage during publish is `Deferred`, not a failure, because
//!   failing the batch would stall the projection too. Usage admission is never
//!   gated by a central outage (`U-18`).

#[cfg(test)]
mod tests;

use aex_usage_domain::fact::{FactDraft, UsageFact};
use aex_usage_domain::frontier::{AcceptedSequence, Frontier, FrontierError, PoisonReason};
use aex_usage_domain::meter::{Category, PublicCategory};
use aex_usage_domain::projection::Generation;
use aex_usage_domain::wire_pending::{Timestamp, WorkspaceId};

use crate::outbox::{OutboxError, OutboxMessage};
use crate::ports::{
    Admission, Applied, AuthorityStore, Clock, CoverageAdvance, FactLocation, PortError,
    ProjectionStore, RatingQueue, ReceiptOutcome, SettlementReceipt,
};
use crate::projection::{FoldContext, FoldError, fold_fact};

/// Why a use case did not complete.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum UseCaseError {
    /// A fact belonging to a sibling authority reached this worker.
    ///
    /// A wiring or IAM defect, never a data condition: the fact is money
    /// evidence and its own worker owns its frontier, so folding it here would
    /// put it beyond that frontier's reach.
    #[error("`{fact_category}` fact reached the `{worker_category}` worker")]
    CategoryEscape {
        /// Where the fact belongs.
        fact_category: &'static str,
        /// Which worker received it.
        worker_category: &'static str,
    },
    /// The sequence being folded is not the one after the projected frontier.
    ///
    /// Within a workspace the stream is ordered, so a gap means an earlier
    /// record has not been delivered yet. Retryable, and alarmed if it persists.
    #[error("sequence {found} arrived while {expected} was expected")]
    SequenceGap {
        /// The next sequence the frontier can fold.
        expected: u64,
        /// The sequence that arrived.
        found: u64,
    },
    /// A correction's target could not be found in this authority.
    #[error("correction target `{target}` is not an admitted fact of this authority")]
    TargetUnknown {
        /// The target the correction names.
        target: String,
    },
    /// A settlement receipt disagrees with the fact it settles.
    #[error(
        "receipt for `{fact}` disagrees on {field}: fact `{fact_value}`, receipt `{receipt_value}`"
    )]
    ReceiptMismatch {
        /// The fact.
        fact: String,
        /// Which field disagreed.
        field: &'static str,
        /// What the fact holds.
        fact_value: String,
        /// What the receipt claims.
        receipt_value: String,
    },
    /// The frontier refused an advance.
    #[error(transparent)]
    Frontier(#[from] FrontierError),
    /// The fold refused.
    #[error(transparent)]
    Fold(#[from] FoldError),
    /// The outbox message could not be built.
    #[error(transparent)]
    Outbox(#[from] OutboxError),
    /// A port failed.
    #[error(transparent)]
    Port(#[from] PortError),
}

impl UseCaseError {
    /// The poison reason this failure quarantines under, when it is terminal.
    ///
    /// `None` means the record should be retried rather than parked. Getting
    /// this wrong in either direction is expensive: parking a transient failure
    /// stalls a paying workspace, and retrying a permanent one is the infinite
    /// poison loop the previous implementation had.
    #[must_use]
    pub const fn poison(&self) -> Option<PoisonReason> {
        match self {
            Self::CategoryEscape { .. } => Some(PoisonReason::CategoryEscape),
            // A correction whose target this authority does not hold, and a
            // fold that refused, are both invariant violations: the fact is
            // internally consistent but the history it names is not.
            Self::TargetUnknown { .. } | Self::Fold(_) => Some(PoisonReason::InvariantViolated),
            Self::ReceiptMismatch { .. } => Some(PoisonReason::PricingVersionMismatch),
            Self::Port(error) if error.terminal() => Some(PoisonReason::Undecodable),
            Self::SequenceGap { .. } | Self::Frontier(_) | Self::Outbox(_) | Self::Port(_) => None,
        }
    }
}

/// Admits one draft into its authority.
///
/// The whole use case is one call, because admission is one transaction. The
/// value is in what it refuses: the authority assigns the sequence, so a
/// replayed producer message cannot forge a frontier position.
#[derive(Debug)]
pub struct RecordFact<A, C> {
    authority: A,
    clock: C,
}

impl<A: AuthorityStore, C: Clock> RecordFact<A, C> {
    /// Binds the use case to one authority.
    pub const fn new(authority: A, clock: C) -> Self {
        Self { authority, clock }
    }

    /// Admits one draft.
    ///
    /// # Errors
    ///
    /// [`UseCaseError::CategoryEscape`] for a foreign category and
    /// [`UseCaseError::Port`] for anything the store reports.
    pub async fn execute(&self, draft: &FactDraft) -> Result<Admission, UseCaseError> {
        let category = draft.authority.category;
        if category != self.authority.category() {
            return Err(UseCaseError::CategoryEscape {
                fact_category: category.id(),
                worker_category: self.authority.category().id(),
            });
        }
        Ok(self.authority.admit(draft, self.clock.now()).await?)
    }

    /// Admits a batch, stopping at the first failure.
    ///
    /// Stopping is deliberate: the drafts in one batch share a workspace
    /// sequence, so continuing past a failure would leave a gap the projection
    /// cannot fold.
    ///
    /// # Errors
    ///
    /// As [`RecordFact::execute`].
    pub async fn execute_batch(
        &self,
        drafts: &[FactDraft],
    ) -> Result<Vec<Admission>, UseCaseError> {
        let mut admitted = Vec::with_capacity(drafts.len());
        for draft in drafts {
            admitted.push(self.execute(draft).await?);
        }
        Ok(admitted)
    }
}

/// What projecting one fact produced.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Projected {
    /// The fold committed and the frontier advanced.
    Folded,
    /// The projection already covered this sequence; the frontier was advanced
    /// to match. This is the crash-between-apply-and-advance recovery path.
    Reconciled,
    /// The frontier already covered this sequence. A pure no-op.
    AlreadyProjected,
}

/// Folds one admitted fact into the query projection and advances the frontier.
#[derive(Debug)]
pub struct ProjectCategory<A, P> {
    authority: A,
    projection: P,
}

impl<A: AuthorityStore, P: ProjectionStore> ProjectCategory<A, P> {
    /// Binds the use case to one authority and the shared projection.
    pub const fn new(authority: A, projection: P) -> Self {
        Self {
            authority,
            projection,
        }
    }

    /// Projects one fact.
    ///
    /// # Errors
    ///
    /// [`UseCaseError::CategoryEscape`] for a foreign fact,
    /// [`UseCaseError::SequenceGap`] when an earlier record has not arrived,
    /// [`UseCaseError::TargetUnknown`] for an unresolvable correction, and
    /// [`UseCaseError::Port`] for anything the stores report.
    pub async fn execute(&self, fact: &UsageFact) -> Result<Projected, UseCaseError> {
        self.guard(fact)?;
        let frontier = self.authority.frontier(&fact.workspace).await?;
        if fact.accepted_sequence.get() <= frontier.projected.get() {
            return Ok(Projected::AlreadyProjected);
        }
        if fact.accepted_sequence.get() != frontier.projected.get() + 1 {
            return Err(UseCaseError::SequenceGap {
                expected: frontier.projected.get() + 1,
                found: fact.accepted_sequence.get(),
            });
        }

        let target = self.resolve_target(fact).await?;
        let generation = self.projection.current_generation().await?;
        let transaction = fold_fact(
            fact,
            FoldContext {
                generation,
                frontier: &frontier,
                target: target.as_ref(),
            },
        )?;
        let applied = self.projection.apply(&transaction).await?;

        let advanced = frontier.project(fact.accepted_sequence)?;
        self.authority
            .advance_frontier(&frontier, &advanced)
            .await?;
        Ok(match applied {
            Applied::Committed => Projected::Folded,
            Applied::AlreadyCovered => Projected::Reconciled,
        })
    }

    /// Refuses a fact belonging to a sibling authority.
    fn guard(&self, fact: &UsageFact) -> Result<(), UseCaseError> {
        if fact.category() == self.authority.category() {
            return Ok(());
        }
        Err(UseCaseError::CategoryEscape {
            fact_category: fact.category().id(),
            worker_category: self.authority.category().id(),
        })
    }

    /// Reads the fact a correction withdraws.
    async fn resolve_target(&self, fact: &UsageFact) -> Result<Option<UsageFact>, UseCaseError> {
        let Some(correction) = fact.kind.correction() else {
            return Ok(None);
        };
        let location = self
            .authority
            .locate(&correction.target)
            .await?
            .ok_or_else(|| UseCaseError::TargetUnknown {
                target: correction.target.to_string(),
            })?;
        let target = self
            .authority
            .fact_at(&location.workspace, location.sequence)
            .await?
            .ok_or_else(|| UseCaseError::TargetUnknown {
                target: correction.target.to_string(),
            })?;
        Ok(Some(target))
    }
}

/// What publishing one fact produced.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Published {
    /// Delivered, the frontier advanced and the outbox marker removed.
    Delivered,
    /// The frontier already covered this sequence.
    AlreadyPublished,
    /// Central is unreachable. The outbox row stays and the sweep retries.
    ///
    /// Not a failure: failing here would stall the projection as well, and a
    /// refused usage fact is unrecoverable money evidence.
    Deferred,
}

/// Delivers one admitted fact to the central settlement queue.
#[derive(Debug)]
pub struct PublishOutbox<A, Q> {
    authority: A,
    queue: Q,
}

impl<A: AuthorityStore, Q: RatingQueue> PublishOutbox<A, Q> {
    /// Binds the use case to one authority and the central queue.
    pub const fn new(authority: A, queue: Q) -> Self {
        Self { authority, queue }
    }

    /// Publishes one fact.
    ///
    /// # Errors
    ///
    /// [`UseCaseError::Outbox`] when the message cannot be built and
    /// [`UseCaseError::Port`] for a non-retryable store failure. A retryable
    /// queue failure is [`Published::Deferred`] rather than an error.
    pub async fn execute(&self, fact: &UsageFact) -> Result<Published, UseCaseError> {
        let frontier = self.authority.frontier(&fact.workspace).await?;
        if fact.accepted_sequence.get() <= frontier.published.get() {
            return Ok(Published::AlreadyPublished);
        }
        let message = OutboxMessage::for_fact(fact)?;
        match self.queue.publish(&message).await {
            Ok(()) => {}
            Err(error) if error.retryable() => return Ok(Published::Deferred),
            Err(error) => return Err(error.into()),
        }
        let advanced = frontier.publish(fact.accepted_sequence)?;
        self.authority
            .advance_frontier(&frontier, &advanced)
            .await?;
        // Only after a confirmed send and a confirmed frontier advance. The
        // reverse order would let a crash lose the marker with the fact
        // undelivered, which the sweep could never notice.
        self.authority
            .discard_outbox(&fact.workspace, fact.accepted_sequence)
            .await?;
        Ok(Published::Delivered)
    }
}

/// What applying one settlement receipt produced.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Settled {
    /// The receipt landed and the settled frontier advanced past it.
    Advanced,
    /// The receipt landed but an earlier one has not; it is parked.
    Parked,
    /// The receipt was already stored and the frontier already past it.
    AlreadySettled,
    /// The receipt disagreed with the fact it settles; the fact is quarantined.
    Quarantined,
}

/// Records a central settlement receipt and advances the settled frontier.
#[derive(Debug)]
pub struct ApplyReceipt<A, P> {
    authority: A,
    projection: P,
}

impl<A: AuthorityStore, P: ProjectionStore> ApplyReceipt<A, P> {
    /// Binds the use case to one authority and the shared projection.
    pub const fn new(authority: A, projection: P) -> Self {
        Self {
            authority,
            projection,
        }
    }

    /// Applies one receipt.
    ///
    /// # Errors
    ///
    /// [`UseCaseError::TargetUnknown`] when the receipt names a fact this
    /// authority does not hold, and [`UseCaseError::Port`] for store failures.
    /// A pricing-version disagreement is [`Settled::Quarantined`] rather than an
    /// error, because parking the frontier is the honest outcome and the caller
    /// must not retry it.
    pub async fn execute(&self, receipt: &SettlementReceipt) -> Result<Settled, UseCaseError> {
        let location = self.locate(receipt).await?;
        let fact = self
            .authority
            .fact_at(&location.workspace, location.sequence)
            .await?
            .ok_or_else(|| UseCaseError::TargetUnknown {
                target: receipt.fact_id.to_string(),
            })?;

        if let Err(mismatch) = verify(receipt, &fact) {
            let reason = mismatch
                .poison()
                .unwrap_or(PoisonReason::PricingVersionMismatch);
            self.authority
                .quarantine(
                    &location.workspace,
                    location.sequence,
                    reason,
                    &mismatch.to_string(),
                )
                .await?;
            return Ok(Settled::Quarantined);
        }

        let stored = self.authority.record_receipt(receipt, &location).await?;
        let frontier = self.authority.frontier(&location.workspace).await?;
        if location.sequence.get() <= frontier.settled.get() {
            return Ok(Settled::AlreadySettled);
        }
        if location.sequence.get() != frontier.settled.get() + 1 {
            // A receipt out of order is parked, never applied out of order:
            // `settled` only ever advances contiguously, so the number a
            // customer sees is always a position the whole prefix covers.
            let _ = stored;
            return Ok(Settled::Parked);
        }

        let drained = self.drain(&location.workspace, frontier.clone()).await?;
        self.authority.advance_frontier(&frontier, &drained).await?;
        self.projection
            .advance_coverage(&CoverageAdvance {
                generation: self.projection.current_generation().await?,
                workspace: location.workspace.clone(),
                public_category: coverage_category(fact.category()),
                settled: drained.settled,
                settled_through: Some(receipt.settled_at),
                updated_at: receipt.settled_at,
            })
            .await?;
        Ok(match stored {
            ReceiptOutcome::Recorded | ReceiptOutcome::AlreadyRecorded => Settled::Advanced,
        })
    }

    /// Resolves the fact a receipt settles, preferring the receipt's own hint.
    async fn locate(&self, receipt: &SettlementReceipt) -> Result<FactLocation, UseCaseError> {
        if let (Some(workspace), Some(sequence)) =
            (receipt.workspace.clone(), receipt.accepted_sequence)
        {
            return Ok(FactLocation {
                workspace,
                sequence,
            });
        }
        self.authority
            .locate(&receipt.fact_id)
            .await?
            .ok_or_else(|| UseCaseError::TargetUnknown {
                target: receipt.fact_id.to_string(),
            })
    }

    /// Advances `settled` over every contiguous parked receipt.
    async fn drain(
        &self,
        workspace: &WorkspaceId,
        frontier: Frontier,
    ) -> Result<Frontier, UseCaseError> {
        let mut current = frontier;
        loop {
            let next = AcceptedSequence::new(current.settled.get() + 1)?;
            if next.get() > current.published.get().max(current.accepted.get()) {
                break;
            }
            if !self.authority.has_receipt(workspace, next).await? {
                break;
            }
            current = current.settle(next)?;
        }
        Ok(current)
    }
}

/// Refuses a receipt that disagrees with the fact it settles.
fn verify(receipt: &SettlementReceipt, fact: &UsageFact) -> Result<(), UseCaseError> {
    if receipt.fact_id != fact.fact_id {
        return Err(UseCaseError::ReceiptMismatch {
            fact: fact.fact_id.to_string(),
            field: "factId",
            fact_value: fact.fact_id.to_string(),
            receipt_value: receipt.fact_id.to_string(),
        });
    }
    if receipt.region != fact.region {
        return Err(UseCaseError::ReceiptMismatch {
            fact: fact.fact_id.to_string(),
            field: "region",
            fact_value: fact.region.to_string(),
            receipt_value: receipt.region.to_string(),
        });
    }
    if receipt.category != fact.category() {
        return Err(UseCaseError::ReceiptMismatch {
            fact: fact.fact_id.to_string(),
            field: "category",
            fact_value: fact.category().id().to_owned(),
            receipt_value: receipt.category.id().to_owned(),
        });
    }
    if receipt.pricing_version != fact.pricing_version {
        // The rate book was pinned at admission. A receipt rated under a
        // different one is not a rounding difference, it is a different price.
        return Err(UseCaseError::ReceiptMismatch {
            fact: fact.fact_id.to_string(),
            field: "pricingVersion",
            fact_value: fact.pricing_version.to_string(),
            receipt_value: receipt.pricing_version.to_string(),
        });
    }
    Ok(())
}

/// The public category one authority's coverage row is keyed by.
#[must_use]
pub const fn coverage_category(category: Category) -> PublicCategory {
    match category {
        Category::Storage => PublicCategory::Storage,
        Category::Compute => PublicCategory::Compute,
        Category::Transfer => PublicCategory::DataTransfer,
    }
}

/// Parks a workspace's frontier behind a record that cannot be folded.
///
/// Nothing is ever skipped to unblock a frontier: skipping would silently lose
/// money evidence, and parking is visible in the customer's coverage vector.
#[derive(Debug)]
pub struct QuarantineFact<A> {
    authority: A,
}

impl<A: AuthorityStore> QuarantineFact<A> {
    /// Binds the use case to one authority.
    pub const fn new(authority: A) -> Self {
        Self { authority }
    }

    /// Parks one position.
    ///
    /// # Errors
    ///
    /// [`UseCaseError::Port`] for anything the store reports.
    pub async fn execute(
        &self,
        workspace: &WorkspaceId,
        sequence: AcceptedSequence,
        reason: PoisonReason,
        detail: &str,
    ) -> Result<(), UseCaseError> {
        self.authority
            .quarantine(workspace, sequence, reason, detail)
            .await?;
        Ok(())
    }
}

/// What one rebuild pass covered.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct RebuildReport {
    /// Facts folded into the new generation.
    pub folded: u64,
    /// Facts skipped because the new generation already covered them.
    pub already_covered: u64,
    /// The highest sequence folded.
    pub highest: u64,
}

/// Rebuilds one workspace's projection into the next generation.
///
/// The rebuild writes generation `n+1` under its own prefix, so generation `n`
/// keeps serving reads untouched. The pointer flip is a separate, verified step
/// this use case deliberately does not perform: a rebuild that cut over as a
/// side effect of finishing could cut over to a partial fold.
#[derive(Debug)]
pub struct RebuildProjection<A, P> {
    authority: A,
    projection: P,
}

impl<A: AuthorityStore, P: ProjectionStore> RebuildProjection<A, P> {
    /// Binds the use case to one authority and the shared projection.
    pub const fn new(authority: A, projection: P) -> Self {
        Self {
            authority,
            projection,
        }
    }

    /// Folds one workspace's whole fact history into `generation`.
    ///
    /// # Errors
    ///
    /// [`UseCaseError::Fold`] when a stored fact cannot be folded and
    /// [`UseCaseError::Port`] for store failures. An interrupted rebuild resumes
    /// by being run again: every fold is fenced by the same coverage condition,
    /// so a fact already folded into `generation` is skipped rather than
    /// double-counted.
    pub async fn execute(
        &self,
        workspace: &WorkspaceId,
        generation: Generation,
    ) -> Result<RebuildReport, UseCaseError> {
        let authoritative = self.authority.frontier(workspace).await?;
        let mut rebuilt = Frontier::empty(
            authoritative.region.clone(),
            workspace.clone(),
            authoritative.category,
        );
        let mut report = RebuildReport::default();
        for step in 1..=authoritative.accepted.get() {
            let sequence = AcceptedSequence::new(step)?;
            rebuilt = rebuilt.admit(sequence)?;
            let Some(fact) = self.authority.fact_at(workspace, sequence).await? else {
                return Err(UseCaseError::Port(PortError::NotFound {
                    what: "usage_fact",
                    id: format!("{workspace}#{step}"),
                }));
            };
            let target = match fact.kind.correction() {
                Some(correction) => {
                    let location = self
                        .authority
                        .locate(&correction.target)
                        .await?
                        .ok_or_else(|| UseCaseError::TargetUnknown {
                            target: correction.target.to_string(),
                        })?;
                    self.authority
                        .fact_at(&location.workspace, location.sequence)
                        .await?
                }
                None => None,
            };
            let transaction = fold_fact(
                &fact,
                FoldContext {
                    generation,
                    frontier: &rebuilt,
                    target: target.as_ref(),
                },
            )?;
            match self.projection.apply(&transaction).await? {
                Applied::Committed => report.folded += 1,
                Applied::AlreadyCovered => report.already_covered += 1,
            }
            rebuilt = rebuilt.project(sequence)?;
            report.highest = step;
        }
        Ok(report)
    }
}

/// Republishes outbox rows central has not acknowledged.
#[derive(Debug)]
pub struct SweepOutbox<A, Q, C> {
    authority: A,
    queue: Q,
    clock: C,
}

/// What one sweep pass did.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SweepReport {
    /// Rows inspected.
    pub inspected: u64,
    /// Rows republished.
    pub republished: u64,
    /// Rows central still refused.
    pub deferred: u64,
    /// Rows whose attempt count crossed the alarm threshold.
    pub alarming: u64,
    /// The oldest undelivered row's age in milliseconds.
    pub oldest_age_ms: u64,
}

impl<A: AuthorityStore, Q: RatingQueue, C: Clock> SweepOutbox<A, Q, C> {
    /// Binds the sweep to one authority and the central queue.
    pub const fn new(authority: A, queue: Q, clock: C) -> Self {
        Self {
            authority,
            queue,
            clock,
        }
    }

    /// Sweeps one shard.
    ///
    /// Republishing uses the same `MessageDeduplicationId`, so it is a no-op
    /// inside `SQS`'s dedupe window and is absorbed by the central inbox key
    /// beyond it. That is why a sweep can be aggressive without risking a
    /// duplicate settlement.
    ///
    /// # Errors
    ///
    /// [`UseCaseError::Port`] for a non-retryable store failure. A retryable
    /// queue failure is counted as deferred rather than raised.
    pub async fn execute(
        &self,
        shard: u8,
        older_than: Timestamp,
        limit: usize,
        attempt_alarm: u32,
    ) -> Result<SweepReport, UseCaseError> {
        let now = self.clock.now();
        let mut report = SweepReport::default();
        for entry in self.authority.due_outbox(shard, older_than, limit).await? {
            report.inspected += 1;
            report.oldest_age_ms = report
                .oldest_age_ms
                .max(entry.enqueued_at.millis_until(now).unwrap_or(0));
            let Some(fact) = self
                .authority
                .fact_at(&entry.workspace, entry.accepted_sequence)
                .await?
            else {
                // An outbox row with no fact is a table defect, not a delivery
                // problem. It is left in place and reported rather than deleted:
                // deleting it would erase the only evidence that it existed.
                return Err(UseCaseError::Port(PortError::NotFound {
                    what: "usage_fact",
                    id: entry.fact_id.to_string(),
                }));
            };
            let message = OutboxMessage::for_fact(&fact)?;
            let attempts = entry.attempts.saturating_add(1);
            if attempts > attempt_alarm {
                report.alarming += 1;
            }
            match self.queue.publish(&message).await {
                Ok(()) => report.republished += 1,
                Err(error) if error.retryable() => {
                    report.deferred += 1;
                }
                Err(error) => return Err(error.into()),
            }
            self.authority
                .note_outbox_attempt(&entry.workspace, entry.accepted_sequence, attempts, now)
                .await?;
        }
        Ok(report)
    }
}
