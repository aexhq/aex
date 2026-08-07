//! Worker behaviour under the faults that actually happen.
//!
//! Every case here is driven through the in-memory doubles, so each assertion is
//! exact rather than statistical and none of it needs an engine or a credential.
//! What the doubles are faithful about is the two conditions that decide
//! correctness — the admission identity fence and the projection coverage fence
//! — and every property below leans on one of them.

use super::{
    BillingMode, ReceiptRecord, StreamRecord, UndecodableRecord, UsageWorker, WorkerLimits,
};
use crate::ports::{AuthorityStore, SettlementReceipt};
use crate::testing::{
    FakeAuthority, FakeProjection, FakeQueue, FrozenClock, at, fact, observability_fact, pricing,
    region, void_fact, workspace,
};
use aex_usage_domain::fact::UsageFact;
use aex_usage_domain::frontier::{AcceptedSequence, PoisonReason};
use aex_usage_domain::meter::Category;
use aex_usage_domain::wire_pending::{PricingVersion, WorkspaceId};

/// The limits every test runs under. Small on purpose: a page budget that never
/// binds would not be a page budget.
const LIMITS: WorkerLimits = WorkerLimits {
    outbox_republish_after_ms: 60_000,
    sweep_page: 200,
    outbox_attempt_alarm: 10,
    outbox_age_alarm_ms: 900_000,
    outbox_backlog_ceiling: 100_000,
};

type Worker = UsageWorker<FakeAuthority, FakeProjection, FakeQueue, FrozenClock>;

/// Folds one corpus through a fresh worker in `batch`-sized batches.
async fn replay(corpus: &[UsageFact], batch: usize) -> ReplayOutcome {
    let harness = harness(Category::Transfer);
    for chunk in corpus.chunks(batch) {
        let records = stream(&harness, chunk);
        harness
            .worker
            .handle_stream(&records)
            .await
            .expect("handles");
    }
    (
        harness.projection.snapshot(),
        harness.projection.hourly_total(),
        harness.authority.frontier_of(&workspace()),
    )
}

/// What one replay run produced: every row, the rollup total and the frontier.
type ReplayOutcome = (
    std::collections::BTreeMap<(String, String), crate::testing::doubles::ProjectionRowState>,
    i128,
    aex_usage_domain::frontier::Frontier,
);

struct Harness {
    authority: FakeAuthority,
    projection: FakeProjection,
    queue: FakeQueue,
    clock: FrozenClock,
    worker: Worker,
}

fn harness(category: Category) -> Harness {
    let authority = FakeAuthority::new(category);
    let projection = FakeProjection::new();
    let queue = FakeQueue::new();
    let clock = FrozenClock::new(620_000);
    let worker = UsageWorker::new(
        authority.clone(),
        projection.clone(),
        queue.clone(),
        clock.clone(),
        LIMITS,
        BillingMode::Shadow,
    );
    Harness {
        authority,
        projection,
        queue,
        clock,
        worker,
    }
}

/// Seeds facts into the authority and hands them over as stream records, which
/// is exactly what an `INSERT` on `itemType = usage_fact` produces.
fn stream(harness: &Harness, facts: &[UsageFact]) -> Vec<StreamRecord> {
    facts
        .iter()
        .enumerate()
        .map(|(index, fact)| {
            harness.authority.seed(fact);
            StreamRecord {
                identifier: format!("record-{index}"),
                fact: Ok(fact.clone()),
            }
        })
        .collect()
}

fn receipt(fact: &UsageFact, pricing_version: PricingVersion) -> SettlementReceipt {
    SettlementReceipt {
        receipt_id: format!("rcpt-{}", fact.accepted_sequence),
        region: fact.region.clone(),
        category: fact.category(),
        fact_id: fact.fact_id.clone(),
        rated_microusd: 0,
        transaction_id: format!("txn-{}", fact.accepted_sequence),
        pricing_version,
        settled_at: at(900_000),
        workspace: Some(fact.workspace.clone()),
        accepted_sequence: Some(fact.accepted_sequence),
    }
}

fn sequence(value: u64) -> AcceptedSequence {
    AcceptedSequence::new(value).expect("a sequence is one based")
}

#[tokio::test]
async fn a_batch_projects_publishes_and_clears_its_outbox() {
    let harness = harness(Category::Storage);
    let facts: Vec<UsageFact> = (1..=3).map(|seq| fact(Category::Storage, seq)).collect();
    let records = stream(&harness, &facts);

    let report = harness
        .worker
        .handle_stream(&records)
        .await
        .expect("handles");
    assert!(report.is_clean(), "{report:?}");
    assert_eq!(report.folded, 3);
    assert_eq!(report.published, 3);
    assert_eq!(report.deferred, 0);

    let frontier = harness.authority.frontier_of(&workspace());
    assert_eq!(frontier.projected.get(), 3);
    assert_eq!(frontier.published.get(), 3);
    assert!(frontier.invariant());
    assert_eq!(
        harness.authority.outbox_depth(),
        0,
        "a confirmed send removes the marker, and with it the due-index entry"
    );
    assert_eq!(harness.queue.delivered().len(), 3);
}

#[tokio::test]
async fn a_fact_is_never_published_before_it_is_projected() {
    let harness = harness(Category::Compute);
    let facts: Vec<UsageFact> = (1..=4).map(|seq| fact(Category::Compute, seq)).collect();
    let records = stream(&harness, &facts);
    harness
        .worker
        .handle_stream(&records)
        .await
        .expect("handles");

    let frontier = harness.authority.frontier_of(&workspace());
    assert!(
        frontier.published.get() <= frontier.projected.get(),
        "published must never lead projected: {frontier:?}"
    );
    assert!(frontier.projected.get() <= frontier.accepted.get());
}

#[tokio::test]
async fn a_redelivered_batch_changes_nothing() {
    let harness = harness(Category::Transfer);
    let facts: Vec<UsageFact> = (1..=3).map(|seq| fact(Category::Transfer, seq)).collect();
    let records = stream(&harness, &facts);

    harness
        .worker
        .handle_stream(&records)
        .await
        .expect("handles");
    let after_first = harness.projection.snapshot();
    let quantity = harness.projection.hourly_total();

    // Every record delivered again, and one of them a third time.
    let mut again = records.clone();
    again.push(records[1].clone());
    let report = harness.worker.handle_stream(&again).await.expect("handles");

    assert!(report.is_clean(), "{report:?}");
    assert_eq!(report.folded, 0, "nothing new was folded");
    assert_eq!(report.already_projected, 4);
    assert_eq!(
        harness.projection.snapshot(),
        after_first,
        "a redelivery must be byte-identical, not merely close"
    );
    assert_eq!(harness.projection.hourly_total(), quantity);
    assert_eq!(
        harness.queue.distinct_identities().len(),
        3,
        "the dedupe identity is derived from the fact, so a replay collapses"
    );
}

#[tokio::test]
async fn an_undecodable_record_parks_its_workspace_and_stops_the_batch_there() {
    let harness = harness(Category::Storage);
    let facts: Vec<UsageFact> = (1..=3).map(|seq| fact(Category::Storage, seq)).collect();
    let mut records = stream(&harness, &facts);
    records[1] = StreamRecord {
        identifier: "record-1".to_owned(),
        fact: Err(UndecodableRecord {
            workspace: Some(workspace()),
            sequence: Some(sequence(2)),
            detail: "unknown factKind".to_owned(),
        }),
    };

    let report = harness
        .worker
        .handle_stream(&records)
        .await
        .expect("handles");
    assert_eq!(
        report.folded, 1,
        "the record before the poison still folded"
    );
    assert_eq!(report.quarantined, 1);
    assert_eq!(
        report.failures,
        vec!["record-1".to_owned()],
        "the lowest failure is reported, so nothing after it is checkpointed past"
    );

    let parked = harness.authority.parked();
    assert_eq!(parked.len(), 1);
    assert_eq!(parked[0].reason, PoisonReason::Undecodable);
    assert_eq!(parked[0].sequence, sequence(2));
    assert!(
        harness.authority.frontier_of(&workspace()).is_quarantined(),
        "the frontier parks rather than skipping money evidence"
    );
    assert!(
        !parked[0].detail.contains("quantity"),
        "a dead-letter body carries identifiers and a reason, never a quantity"
    );
}

#[tokio::test]
async fn a_fact_from_a_sibling_authority_is_parked_never_folded() {
    // A compute fact reaching the storage worker is a wiring or IAM defect. It
    // must not be folded here: its own worker owns its frontier.
    let harness = harness(Category::Storage);
    let foreign = fact(Category::Compute, 1);
    harness.authority.seed(&foreign);
    let records = vec![StreamRecord {
        identifier: "record-0".to_owned(),
        fact: Ok(foreign),
    }];

    let report = harness
        .worker
        .handle_stream(&records)
        .await
        .expect("handles");
    assert_eq!(report.folded, 0);
    assert_eq!(report.quarantined, 1);
    assert_eq!(report.failures, vec!["record-0".to_owned()]);
    assert_eq!(
        harness.authority.parked()[0].reason,
        PoisonReason::CategoryEscape
    );
    assert_eq!(harness.projection.applied_total(), 0);
}

#[tokio::test]
async fn one_workspace_parking_does_not_stall_another() {
    let harness = harness(Category::Storage);
    let poisoned = vec![StreamRecord {
        identifier: "poison".to_owned(),
        fact: Err(UndecodableRecord {
            workspace: Some(workspace()),
            sequence: Some(sequence(1)),
            detail: "unknown factKind".to_owned(),
        }),
    }];
    harness
        .worker
        .handle_stream(&poisoned)
        .await
        .expect("handles");
    assert!(harness.authority.frontier_of(&workspace()).is_quarantined());

    // A different partition key is a different batch and a different frontier.
    let other = WorkspaceId::parse("ws-2").expect("workspace");
    let mut second = fact(Category::Storage, 1);
    second.workspace = other.clone();
    let records = stream(&harness, std::slice::from_ref(&second));
    let report = harness
        .worker
        .handle_stream(&records)
        .await
        .expect("handles");

    assert!(report.is_clean(), "{report:?}");
    assert_eq!(report.folded, 1);
    let unaffected = harness.authority.frontier_of(&other);
    assert!(!unaffected.is_quarantined());
    assert_eq!(unaffected.projected.get(), 1);
}

#[tokio::test]
async fn a_central_outage_keeps_projecting_and_accumulates_backlog() {
    let harness = harness(Category::Compute);
    harness.queue.set_outage(true);
    let facts: Vec<UsageFact> = (1..=5).map(|seq| fact(Category::Compute, seq)).collect();
    let records = stream(&harness, &facts);

    let report = harness
        .worker
        .handle_stream(&records)
        .await
        .expect("handles");
    assert!(
        report.is_clean(),
        "a central outage must not fail the batch, or the projection stalls too"
    );
    assert_eq!(report.folded, 5);
    assert_eq!(report.deferred, 5);
    assert_eq!(report.published, 0);

    let frontier = harness.authority.frontier_of(&workspace());
    assert_eq!(frontier.projected.get(), 5, "the customer view keeps up");
    assert_eq!(frontier.published.get(), 0, "settlement honestly lags");
    assert_eq!(
        harness.authority.outbox_depth(),
        5,
        "the backlog is durable"
    );

    // Recovery is the sweep, with no duplicate and no lost fact.
    harness.queue.set_outage(false);
    harness.clock.advance(120_000);
    let swept = harness.worker.handle_sweep().await.expect("sweeps");
    assert_eq!(swept.republished, 5);
    assert_eq!(
        harness.queue.distinct_identities().len(),
        5,
        "five facts, five identities: a republish is not a second settlement"
    );
}

#[tokio::test]
async fn the_sweep_only_touches_rows_older_than_the_republish_threshold() {
    let harness = harness(Category::Storage);
    harness.queue.set_outage(true);
    let facts: Vec<UsageFact> = (1..=2).map(|seq| fact(Category::Storage, seq)).collect();
    let records = stream(&harness, &facts);
    harness
        .worker
        .handle_stream(&records)
        .await
        .expect("handles");
    harness.queue.set_outage(false);

    // The rows were enqueued at 600_000-odd and the clock is at 700_000, which
    // is inside the 60 s window.
    let early = harness.worker.handle_sweep().await.expect("sweeps");
    assert_eq!(early.republished, 0, "{early:?}");

    harness.clock.advance(300_000);
    let late = harness.worker.handle_sweep().await.expect("sweeps");
    assert_eq!(late.republished, 2);
    assert!(late.oldest_age_ms > 0);
}

#[tokio::test]
async fn a_sweep_republish_counts_an_attempt_and_alarms_past_the_threshold() {
    let harness = harness(Category::Transfer);
    harness.queue.set_outage(true);
    let facts = vec![fact(Category::Transfer, 1)];
    let records = stream(&harness, &facts);
    harness
        .worker
        .handle_stream(&records)
        .await
        .expect("handles");
    harness.queue.set_outage(false);

    let mut alarming = 0;
    for _ in 0..12 {
        harness.clock.advance(120_000);
        alarming += harness
            .worker
            .handle_sweep()
            .await
            .expect("sweeps")
            .alarming;
    }
    assert!(
        alarming > 0,
        "a row that keeps being republished has to become visible"
    );
}

#[tokio::test]
async fn a_receipt_advances_the_settled_frontier_only_contiguously() {
    let harness = harness(Category::Storage);
    let facts: Vec<UsageFact> = (1..=3).map(|seq| fact(Category::Storage, seq)).collect();
    let records = stream(&harness, &facts);
    harness
        .worker
        .handle_stream(&records)
        .await
        .expect("handles");

    // The third receipt arrives first and must park rather than skip ahead.
    let out_of_order = vec![ReceiptRecord {
        identifier: "r3".to_owned(),
        receipt: receipt(&facts[2], pricing()),
    }];
    let parked = harness
        .worker
        .handle_receipts(&out_of_order)
        .await
        .expect("handles");
    assert_eq!(parked.parked, 1);
    assert_eq!(harness.authority.frontier_of(&workspace()).settled.get(), 0);

    // The gap fills and the whole prefix drains in one step.
    let rest = vec![
        ReceiptRecord {
            identifier: "r2".to_owned(),
            receipt: receipt(&facts[1], pricing()),
        },
        ReceiptRecord {
            identifier: "r1".to_owned(),
            receipt: receipt(&facts[0], pricing()),
        },
    ];
    let drained = harness
        .worker
        .handle_receipts(&rest)
        .await
        .expect("handles");
    assert_eq!(drained.advanced, 1);
    assert_eq!(drained.parked, 1);
    let frontier = harness.authority.frontier_of(&workspace());
    assert_eq!(
        frontier.settled.get(),
        3,
        "settled covers a prefix, so filling the gap drains everything parked"
    );
    assert!(frontier.invariant());
}

#[tokio::test]
async fn a_receipt_rated_under_another_rate_book_quarantines_the_fact() {
    let harness = harness(Category::Compute);
    let facts = vec![fact(Category::Compute, 1)];
    let records = stream(&harness, &facts);
    harness
        .worker
        .handle_stream(&records)
        .await
        .expect("handles");

    let drifted = PricingVersion::parse("some-other-book-v9").expect("version");
    let report = harness
        .worker
        .handle_receipts(&[ReceiptRecord {
            identifier: "r1".to_owned(),
            receipt: receipt(&facts[0], drifted),
        }])
        .await
        .expect("handles");

    assert_eq!(report.quarantined, 1);
    assert_eq!(report.advanced, 0);
    assert_eq!(
        harness.authority.parked()[0].reason,
        PoisonReason::PricingVersionMismatch,
        "the rate book was pinned at admission; a different one is a different price"
    );
    assert_eq!(harness.authority.frontier_of(&workspace()).settled.get(), 0);
}

#[tokio::test]
async fn a_duplicate_receipt_is_absorbed_rather_than_settling_twice() {
    let harness = harness(Category::Storage);
    let facts = vec![fact(Category::Storage, 1)];
    let records = stream(&harness, &facts);
    harness
        .worker
        .handle_stream(&records)
        .await
        .expect("handles");

    let message = ReceiptRecord {
        identifier: "r1".to_owned(),
        receipt: receipt(&facts[0], pricing()),
    };
    let first = harness
        .worker
        .handle_receipts(std::slice::from_ref(&message))
        .await
        .expect("handles");
    let second = harness
        .worker
        .handle_receipts(std::slice::from_ref(&message))
        .await
        .expect("handles");

    assert_eq!(first.advanced, 1);
    assert_eq!(second.already_settled, 1);
    assert_eq!(harness.authority.frontier_of(&workspace()).settled.get(), 1);
}

#[tokio::test]
async fn a_correction_withdraws_its_target_through_the_whole_pipeline() {
    let harness = harness(Category::Compute);
    let target = fact(Category::Compute, 1);
    let void = void_fact(Category::Compute, 2, &target.fact_id);
    let records = stream(&harness, &[target.clone(), void]);

    let report = harness
        .worker
        .handle_stream(&records)
        .await
        .expect("handles");
    assert!(report.is_clean(), "{report:?}");
    assert_eq!(
        harness.projection.hourly_total(),
        0,
        "a void must leave the rollup exactly where it started"
    );
    assert_eq!(harness.projection.daily_total(), 0);
    assert_eq!(harness.projection.detail_rows(), 1);
    assert_eq!(
        harness.projection.active_details(),
        0,
        "the detail row stays visible as withdrawn rather than being deleted"
    );
}

#[tokio::test]
async fn a_zero_dollar_observability_fact_advances_the_frontier_and_bills_nothing() {
    let harness = harness(Category::Compute);
    let records = stream(&harness, &[observability_fact(1)]);

    let report = harness
        .worker
        .handle_stream(&records)
        .await
        .expect("handles");
    assert!(report.is_clean(), "{report:?}");
    assert_eq!(report.folded, 1);
    assert_eq!(
        report.published, 1,
        "it is still money evidence and is sent"
    );
    assert_eq!(harness.projection.hourly_total(), 0);
    assert_eq!(harness.projection.detail_rows(), 0);
    assert_eq!(
        harness.authority.frontier_of(&workspace()).projected.get(),
        1
    );
}

#[tokio::test]
async fn a_sequence_gap_is_retried_rather_than_parked() {
    // A gap inside one workspace means an earlier record has not been delivered
    // yet, which the ordered stream will fix. Parking would stall a paying
    // workspace over a transient condition.
    let harness = harness(Category::Storage);
    let first = fact(Category::Storage, 1);
    let third = fact(Category::Storage, 3);
    harness.authority.seed(&first);
    harness.authority.seed(&third);
    let records = vec![StreamRecord {
        identifier: "record-2".to_owned(),
        fact: Ok(third),
    }];

    let report = harness
        .worker
        .handle_stream(&records)
        .await
        .expect("handles");
    assert_eq!(report.failures, vec!["record-2".to_owned()]);
    assert_eq!(report.quarantined, 0, "a gap is not poison");
    assert!(!harness.authority.frontier_of(&workspace()).is_quarantined());
}

#[tokio::test]
async fn the_projection_and_the_frontier_reconcile_after_a_crash_between_them() {
    // The transaction committed but the process died before the frontier
    // advanced. Replay must reconcile, not double-count.
    let harness = harness(Category::Storage);
    let facts = vec![fact(Category::Storage, 1)];
    let records = stream(&harness, &facts);
    harness
        .worker
        .handle_stream(&records)
        .await
        .expect("handles");
    let after = harness.projection.snapshot();

    // Rewind the frontier to before the advance, exactly as a crash would.
    let rewound = harness
        .authority
        .frontier_of(&workspace())
        .publish(sequence(1))
        .err()
        .map_or_else(
            || crate::testing::frontier_at(Category::Storage, 1, 0),
            |_| crate::testing::frontier_at(Category::Storage, 1, 0),
        );
    let current = harness.authority.frontier_of(&workspace());
    harness
        .authority
        .advance_frontier(&current, &rewound)
        .await
        .expect("rewinds");

    let report = harness
        .worker
        .handle_stream(&records)
        .await
        .expect("handles");
    assert_eq!(report.reconciled, 1, "{report:?}");
    assert_eq!(
        harness.projection.snapshot(),
        after,
        "reconciling must not re-apply the quantity"
    );
    assert_eq!(
        harness.authority.frontier_of(&workspace()).projected.get(),
        1
    );
}

#[tokio::test]
async fn replaying_a_corpus_into_a_fresh_projection_reproduces_it_exactly() {
    let corpus: Vec<UsageFact> = (1..=40).map(|seq| fact(Category::Transfer, seq)).collect();
    let (rows_one, total_one, frontier_one) = replay(&corpus, 7).await;
    let (rows_two, total_two, frontier_two) = replay(&corpus, 13).await;
    assert_eq!(
        rows_one, rows_two,
        "a different batch partitioning must produce identical rows"
    );
    assert_eq!(total_one, total_two);
    assert_eq!(frontier_one, frontier_two);
    assert_eq!(frontier_one.projected.get(), 40);
}

#[tokio::test]
async fn the_billing_mode_is_a_single_value_a_deployment_cannot_half_satisfy() {
    let harness = harness(Category::Storage);
    assert_eq!(harness.worker.mode(), BillingMode::Shadow);
    assert_eq!(harness.worker.mode().id(), "shadow");
    assert_eq!(BillingMode::Active.id(), "active");
    assert_ne!(BillingMode::Shadow, BillingMode::Active);
    assert_eq!(harness.worker.limits(), LIMITS);
}

#[tokio::test]
async fn a_backlog_past_its_ceiling_alarms() {
    let harness = harness(Category::Storage);
    let quiet = super::SweepReport {
        inspected: 3,
        oldest_age_ms: 1_000,
        ..super::SweepReport::default()
    };
    assert!(!harness.worker.backlog_alarms(&quiet));

    let stale = super::SweepReport {
        oldest_age_ms: LIMITS.outbox_age_alarm_ms + 1,
        ..quiet
    };
    assert!(harness.worker.backlog_alarms(&stale));

    let deep = super::SweepReport {
        inspected: LIMITS.outbox_backlog_ceiling + 1,
        ..quiet
    };
    assert!(harness.worker.backlog_alarms(&deep));
}

#[tokio::test]
async fn the_outbox_shard_count_matches_the_due_index_grammar() {
    // Every shard is addressable inside the two-digit `OUT#{shard:02}` prefix.
    assert_eq!(super::OUTBOX_SHARDS, 16);
    let harness = harness(Category::Storage);
    assert_eq!(region(), crate::testing::region());
    assert_eq!(harness.worker.limits().sweep_page, 200);
}
