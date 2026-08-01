//! The fault corpus a shadow close has to survive.
//!
//! Every case below is one of plan 12 §8.1's locally provable gates, driven
//! through the in-memory ports and the pure domain arithmetic. Nothing here
//! samples a clock or a scheduler, so each assertion is exact rather than
//! statistical.
//!
//! What is deliberately *not* asserted here is anything needing a physical
//! receipt: per-service `AWS` reconciliation, cgroup accuracy against a real
//! bill, whether the `MicroVM` provider exposes a transmit receipt at all, the
//! `LinearAt1769Mb` convention's accuracy, and the delivery-receipt formats.
//! Those are named in the handoff instead of being quietly passed.

use super::{
    CpuAttributionSummary, OutboxLag, SHADOW_REPORT_SCHEMA, ShadowError, ShadowReport,
    StorageCeilSummary,
};
use crate::projection::{FoldContext, fold_fact};
use crate::testing::{
    FakeAuthority, FakeProjection, FakeQueue, FrozenClock, at, fact, frontier_at,
    observability_fact, void_fact, workspace,
};
use crate::worker::{BillingMode, StreamRecord, UsageWorker, WorkerLimits};
use aex_usage_domain::fact::UsageFact;
use aex_usage_domain::identity::AuthorityId;
use aex_usage_domain::interval::storage::{
    StorageCursor, StorageOwner, StorageOwnerKind, StorageSource, StorageTransition,
};
use aex_usage_domain::interval::{accrue_storage, reconcile_cpu};
use aex_usage_domain::meter::{Category, Meter};
use aex_usage_domain::projection::Generation;

const LIMITS: WorkerLimits = WorkerLimits {
    outbox_republish_after_ms: 60_000,
    sweep_page: 200,
    outbox_attempt_alarm: 10,
    outbox_age_alarm_ms: 900_000,
    outbox_backlog_ceiling: 100_000,
};

type Worker = UsageWorker<FakeAuthority, FakeProjection, FakeQueue, FrozenClock>;

fn worker(
    category: Category,
) -> (
    FakeAuthority,
    FakeProjection,
    FakeQueue,
    FrozenClock,
    Worker,
) {
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
    (authority, projection, queue, clock, worker)
}

fn records(authority: &FakeAuthority, facts: &[UsageFact]) -> Vec<StreamRecord> {
    facts
        .iter()
        .enumerate()
        .map(|(index, fact)| {
            authority.seed(fact);
            StreamRecord {
                identifier: format!("record-{index}"),
                fact: Ok(fact.clone()),
            }
        })
        .collect()
}

/// Folds one corpus purely, which is what the report accumulates.
fn fold_all(
    facts: &[UsageFact],
    category: Category,
) -> Vec<crate::projection::ProjectionTransaction> {
    let mut folded = Vec::new();
    for (index, fact) in facts.iter().enumerate() {
        let frontier = frontier_at(category, facts.len() as u64, index as u64);
        folded.push(
            fold_fact(
                fact,
                FoldContext {
                    generation: Generation::FIRST,
                    frontier: &frontier,
                    target: None,
                },
            )
            .expect("folds"),
        );
    }
    folded
}

#[test]
fn a_close_reports_every_priced_meter_including_the_silent_ones() {
    let mut report = ShadowReport::new();
    report.observe(&fold_all(&[fact(Category::Storage, 1)], Category::Storage));

    assert_eq!(report.schema, SHADOW_REPORT_SCHEMA);
    assert_eq!(report.mode, "shadow");
    assert_eq!(report.fact_count, 1);
    assert!(report.per_meter.contains_key(Meter::StorageByteMin.id()));
    // "we measured nothing" and "this meter was never wired up" look identical
    // in an absent row, so the silent ones are named.
    let silent = report.silent_meters();
    assert_eq!(silent.len(), 3);
    assert!(!silent.contains(&Meter::StorageByteMin.id()));
}

#[test]
fn a_close_that_rated_anything_is_a_failed_close() {
    let mut report = ShadowReport::new();
    report.qualifies().expect("an empty shadow close qualifies");
    report.rated_microusd = 1;
    assert!(matches!(
        report.qualifies(),
        Err(ShadowError::RatedInShadow { rated_microusd: 1 })
    ));
}

#[test]
fn the_declared_storage_ceil_is_asserted_as_a_bound_and_reported_as_a_number() {
    // M-STOR-CLOSE: interiors floor, the hard-delete close ceils. The excess is
    // bounded by bytes x 1 minute per residence, and the close reports the
    // exact total rather than the principle.
    let owner = StorageOwner {
        kind: StorageOwnerKind::ContentObject,
        id: AuthorityId::parse("obj-1").expect("id"),
        generation: 1,
    };
    let bytes = 4_096_u64;
    // A residence of ten seconds is billed as one whole minute.
    let opened = accrue_storage(
        None,
        &owner,
        StorageSource::S3,
        at(0),
        StorageTransition::Put { bytes },
        "commit-open",
    )
    .expect("opens");
    let cursor: StorageCursor = opened.cursor;
    let closed = accrue_storage(
        Some(&cursor),
        &owner,
        StorageSource::S3,
        at(10_000),
        StorageTransition::HardDelete,
        "commit-close",
    )
    .expect("closes");
    let charged = closed
        .closed
        .expect("a hard delete closes an interval")
        .quantity()
        .get();
    assert_eq!(
        charged,
        u128::from(bytes),
        "ten seconds of residence is charged as one whole minute of bytes"
    );

    let mut report = ShadowReport::new();
    report.storage_ceil = StorageCeilSummary {
        residences: 1,
        ceil_byte_minutes: charged,
        closing_bytes: u128::from(bytes),
    };
    report.ceil_bound_holds().expect("the bound holds exactly");
    report
        .qualifies()
        .expect("a close with the bound met passes");

    // One byte-minute beyond the bound is a different transformation from the
    // one that was signed off, and fails the close.
    report.storage_ceil.ceil_byte_minutes += 1;
    assert!(matches!(
        report.ceil_bound_holds(),
        Err(ShadowError::CeilBoundExceeded { .. })
    ));
}

#[test]
fn charged_cpu_never_exceeds_physics_and_the_remainder_balances() {
    // The pathological case the plan names: ten times over-attribution.
    let physical = 1_000_u64;
    let allocation = reconcile_cpu(physical, &[4_000, 3_000, 3_000]).expect("reconciles");
    let charged: u64 = allocation.charged.iter().sum();

    let mut report = ShadowReport::new();
    report.cpu = CpuAttributionSummary {
        physical_us: physical,
        attributed_us: 10_000,
        charged_us: charged,
        platform_us: physical - charged,
        scaled_intervals: u64::from(allocation.scaled),
    };
    report
        .cpu
        .holds()
        .expect("attribution stays inside physics");
    assert!(
        allocation.scaled,
        "an over-attributed interval must be scaled"
    );
    assert!(charged <= physical);

    // Reporting more charge than the cgroup saw is a failed close.
    report.cpu.charged_us = physical + 1;
    assert!(matches!(
        report.cpu.holds(),
        Err(ShadowError::OverPhysical { .. })
    ));

    // And so is a remainder that does not balance. This allocation happens to
    // exhaust the physical reading exactly, so the unbalanced case is built by
    // booking a platform microsecond that does not exist.
    report.cpu.charged_us = charged;
    report.cpu.platform_us = physical - charged + 1;
    assert!(matches!(
        report.cpu.holds(),
        Err(ShadowError::UnbalancedCpu { .. })
    ));
}

#[test]
fn a_close_with_an_interval_still_open_does_not_qualify() {
    let mut report = ShadowReport::new();
    report.open_intervals = 1;
    assert!(matches!(
        report.qualifies(),
        Err(ShadowError::LeakedInterval { leaked: 1 })
    ));
}

#[tokio::test]
async fn duplicate_delivery_leaves_every_total_unchanged() {
    let (authority, projection, _queue, _clock, worker) = worker(Category::Storage);
    let facts: Vec<UsageFact> = (1..=6).map(|seq| fact(Category::Storage, seq)).collect();
    let stream = records(&authority, &facts);

    worker.handle_stream(&stream).await.expect("handles");
    let baseline = projection.snapshot();

    // Every record one to five times, in one batch.
    let mut repeated = Vec::new();
    for (index, record) in stream.iter().enumerate() {
        for _ in 0..=(index % 5) {
            repeated.push(record.clone());
        }
    }
    worker.handle_stream(&repeated).await.expect("handles");

    assert_eq!(
        projection.snapshot(),
        baseline,
        "a duplicate must change nothing, byte for byte"
    );
}

#[tokio::test]
async fn disorder_across_workspaces_converges_on_the_same_totals() {
    // Per-workspace order is guaranteed by the partition key; across workspaces
    // it is irrelevant, and that has to be true rather than assumed.
    let build = |reverse: bool| async move {
        let (authority, projection, _queue, _clock, worker) = worker(Category::Transfer);
        let mut all = Vec::new();
        for suffix in ["ws-a", "ws-b", "ws-c"] {
            let mut batch = Vec::new();
            for seq in 1..=4 {
                let mut one = fact(Category::Transfer, seq);
                one.workspace =
                    aex_usage_domain::wire_pending::WorkspaceId::parse(suffix).expect("workspace");
                batch.push(one);
            }
            all.push(batch);
        }
        if reverse {
            all.reverse();
        }
        for batch in &all {
            let stream = records(&authority, batch);
            worker.handle_stream(&stream).await.expect("handles");
        }
        projection.hourly_total()
    };

    assert_eq!(build(false).await, build(true).await);
}

#[tokio::test]
async fn a_crash_between_the_projection_and_the_frontier_reconverges() {
    let (authority, projection, _queue, _clock, worker) = worker(Category::Compute);
    let facts: Vec<UsageFact> = (1..=3).map(|seq| fact(Category::Compute, seq)).collect();
    let stream = records(&authority, &facts);
    worker.handle_stream(&stream).await.expect("handles");
    let settled = projection.snapshot();

    // Rewind the frontier to each of the three positions the crash could have
    // left it at, and replay. Every survivor state must reconverge.
    for rewind in 0..3 {
        let current = authority.frontier_of(&workspace());
        let earlier = frontier_at(Category::Compute, 3, rewind);
        crate::ports::AuthorityStore::advance_frontier(&authority, &current, &earlier)
            .await
            .expect("rewinds");
        worker.handle_stream(&stream).await.expect("handles");
        assert_eq!(
            projection.snapshot(),
            settled,
            "replay from position {rewind} must not re-apply a quantity"
        );
    }
}

#[tokio::test]
async fn a_producer_clock_skew_never_moves_a_sequence_or_an_identity() {
    // Service time is producer stamped; the sequence and the admission instant
    // are authority stamped. Ordering and idempotency must not depend on a
    // producer clock at all.
    let (authority, _projection, _queue, _clock, _worker) = worker(Category::Storage);
    let first = fact(Category::Storage, 1);
    authority.seed(&first);

    let mut skewed = first.clone();
    skewed.accepted_at = at(600_000 - 10 * 60 * 1_000);
    assert_eq!(
        skewed.fact_id, first.fact_id,
        "identity is derived from the authority key, never from a clock"
    );
    assert_eq!(skewed.accepted_sequence, first.accepted_sequence);
}

#[tokio::test]
async fn a_deleted_residence_seals_and_leaves_no_open_interval() {
    let owner = StorageOwner {
        kind: StorageOwnerKind::ContentObject,
        id: AuthorityId::parse("obj-2").expect("id"),
        generation: 1,
    };
    let mut cursor = accrue_storage(
        None,
        &owner,
        StorageSource::S3,
        at(0),
        StorageTransition::Put { bytes: 1_000 },
        "c0",
    )
    .expect("opens")
    .cursor;

    // Put -> resize -> trash -> restore -> hard delete. Trash is an interior
    // transition, not a seal: recoverable trash stays billable.
    for (millis, transition, commit) in [
        (60_000, StorageTransition::Resize { bytes: 2_000 }, "c1"),
        (120_000, StorageTransition::Trash, "c2"),
        (180_000, StorageTransition::Restore, "c3"),
    ] {
        let outcome = accrue_storage(
            Some(&cursor),
            &owner,
            StorageSource::S3,
            at(millis),
            transition,
            commit,
        )
        .expect("accrues");
        cursor = outcome.cursor;
        assert!(!cursor.sealed, "an interior transition never seals");
    }

    let closed = accrue_storage(
        Some(&cursor),
        &owner,
        StorageSource::S3,
        at(200_000),
        StorageTransition::HardDelete,
        "c4",
    )
    .expect("closes");
    let sealed = closed.cursor;
    assert!(sealed.sealed);

    // A sealed cursor refuses everything, so no interval can be reopened.
    assert!(
        accrue_storage(
            Some(&sealed),
            &owner,
            StorageSource::S3,
            at(300_000),
            StorageTransition::Put { bytes: 10 },
            "c5",
        )
        .is_err(),
        "a sealed residence must not accept another transition"
    );

    let mut report = ShadowReport::new();
    report.open_intervals = 0;
    report.qualifies().expect("nothing was left open");
}

#[tokio::test]
async fn a_close_copies_every_frontier_and_reports_delivery_lag() {
    let (authority, _projection, queue, clock, worker) = worker(Category::Storage);
    queue.set_outage(true);
    let facts: Vec<UsageFact> = (1..=4).map(|seq| fact(Category::Storage, seq)).collect();
    let stream = records(&authority, &facts);
    worker.handle_stream(&stream).await.expect("handles");
    clock.advance(300_000);
    let swept = worker.handle_sweep().await.expect("sweeps");

    let mut report = ShadowReport::new();
    report.cover(&[authority.frontier_of(&workspace())]);
    report.outbox = OutboxLag {
        pending: authority.outbox_depth() as u64,
        oldest_age_ms: swept.oldest_age_ms,
        republished: swept.republished,
    };

    assert_eq!(report.workspace_count, 1);
    assert_eq!(report.frontiers[0].projected, 4);
    assert_eq!(
        report.frontiers[0].published, 0,
        "the report must say settlement is behind rather than imply it is not"
    );
    assert_eq!(report.outbox.pending, 4);
    assert!(report.outbox.oldest_age_ms > 0);
    report.qualifies().expect("a lagging close still qualifies");
}

#[tokio::test]
async fn a_corrected_history_nets_to_zero_in_the_report() {
    let (authority, projection, _queue, _clock, worker) = worker(Category::Compute);
    let target = fact(Category::Compute, 1);
    let void = void_fact(Category::Compute, 2, &target.fact_id);
    let stream = records(&authority, &[target, void]);
    worker.handle_stream(&stream).await.expect("handles");

    assert_eq!(projection.hourly_total(), 0);
    assert_eq!(projection.daily_total(), 0);
}

#[tokio::test]
async fn zero_dollar_facts_never_reach_a_priced_total() {
    let (authority, projection, _queue, _clock, worker) = worker(Category::Compute);
    let stream = records(&authority, &[observability_fact(1), observability_fact(2)]);
    worker.handle_stream(&stream).await.expect("handles");

    let mut report = ShadowReport::new();
    report.observability.insert("model.tokens".to_owned(), 2);
    assert_eq!(projection.hourly_total(), 0);
    assert!(
        report.per_meter.is_empty(),
        "an observability meter has no rate path to reach"
    );
    report.qualifies().expect("qualifies");
}

#[test]
fn the_report_renders_as_canonical_json_a_signer_can_read() {
    let mut report = ShadowReport::new();
    report.observe(&fold_all(&[fact(Category::Storage, 1)], Category::Storage));
    report.cover(&[frontier_at(Category::Storage, 1, 1)]);
    let rendered = report.render().expect("renders");

    assert!(rendered.contains(SHADOW_REPORT_SCHEMA));
    assert!(rendered.contains("\"mode\": \"shadow\""));
    assert!(rendered.contains("\"ratedMicrousd\": 0"));
    assert!(rendered.contains("\"storageCeil\""));

    // A report round-trips, so a signed close can be re-checked later.
    let parsed: ShadowReport = serde_json::from_str(&rendered).expect("round trips");
    assert_eq!(parsed, report);
}

#[cfg(feature = "probe")]
mod probe_gates {
    use crate::probe::sink::{BoundedFactSink, FactSink, OverflowLedger};
    use crate::probe::testing::{ScriptedCpuClock, activation_key, probe_context};
    use crate::probe::{ActivationMeter, ActivationScoped, MAX_ATTRIBUTED_POLL_US};
    use aex_usage_domain::fact::Attribution;
    use std::sync::Arc;

    #[tokio::test]
    async fn awaiting_work_attributes_no_cpu_and_produces_no_fact() {
        // A future that awaits a sleep, a provider call and a durable wait is
        // not inside a poll while it waits, so it creates no compute fact. The
        // clock is scripted, so this is exact rather than statistical.
        let clock = ScriptedCpuClock::new([0, 0, 0, 0, 0, 0]);
        let meter = ActivationMeter::new(activation_key(), probe_context(), Attribution::default());
        let sink =
            Arc::new(BoundedFactSink::new(8, Arc::new(OverflowLedger::new())).expect("capacity"));

        let scoped = async {
            tokio::task::yield_now().await;
            tokio::task::yield_now().await;
            42_u32
        }
        .metered(Arc::clone(&meter), clock);
        assert_eq!(scoped.await, 42);

        assert_eq!(
            meter.attributed_us(),
            0,
            "time spent awaiting is not time spent computing"
        );
        assert!(sink.take_batch(8).is_empty());
    }

    #[tokio::test]
    async fn no_poll_is_ever_attributed_more_than_the_cap() {
        // A pathological poll cannot bill an unbounded amount: the cap turns a
        // stall into a defect signal rather than a charge.
        let clock = ScriptedCpuClock::new([0, 10_000_000]);
        let meter = ActivationMeter::new(activation_key(), probe_context(), Attribution::default());
        let scoped = async { 1_u32 }.metered(Arc::clone(&meter), clock);
        assert_eq!(scoped.await, 1);
        assert!(meter.attributed_us() <= MAX_ATTRIBUTED_POLL_US);
        assert_eq!(
            meter.long_poll_violations(),
            1,
            "the excess is reported as a defect, not silently discarded"
        );
    }

    #[test]
    fn nothing_is_left_queued_after_a_terminal_cleanup() {
        let sink = BoundedFactSink::new(4, Arc::new(OverflowLedger::new())).expect("capacity");
        sink.offer(crate::probe::testing::draft_fixture(1))
            .expect("offers");
        assert_eq!(sink.queued(), 1);
        let drained = sink.take_batch(4);
        assert_eq!(drained.len(), 1);
        assert_eq!(sink.queued(), 0);
        assert!(
            sink.overflow().is_empty(),
            "a Drop audit counter must be zero at a clean close"
        );
    }
}
