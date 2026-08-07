//! Use-case behaviour: admission identity, rebuild, and the retryable/terminal
//! split that decides whether a workspace parks or waits.

use super::{
    ApplyReceipt, ProjectCategory, Projected, PublishOutbox, Published, RebuildProjection,
    RecordFact, SweepOutbox, UseCaseError, coverage_category,
};
use crate::ports::{Admission, PortError, ProjectionStore, SettlementReceipt};
use crate::testing::{
    FakeAuthority, FakeProjection, FakeQueue, FrozenClock, at, draft, fact, measurement_for,
    pricing, void_fact, workspace,
};
use aex_usage_domain::fact::FactKind;
use aex_usage_domain::frontier::AcceptedSequence;
use aex_usage_domain::meter::{Category, PublicCategory};
use aex_usage_domain::projection::Generation;

fn clock() -> FrozenClock {
    FrozenClock::new(620_000)
}

#[tokio::test]
async fn the_authority_assigns_the_sequence_a_producer_never_supplies_one() {
    let authority = FakeAuthority::new(Category::Storage);
    let record = RecordFact::new(authority.clone(), clock());

    let mut admitted = Vec::new();
    for index in 1..=3_u64 {
        let draft = draft(
            Category::Storage,
            &format!("res-{index}"),
            FactKind::Measured(measurement_for(Category::Storage, 1_000 + index)),
        );
        match record.execute(&draft).await.expect("admits") {
            Admission::Admitted(fact) => admitted.push(fact.accepted_sequence.get()),
            other => panic!("expected an admission, got {other:?}"),
        }
    }
    assert_eq!(
        admitted,
        vec![1, 2, 3],
        "the sequence is contiguous and authority assigned, so a replayed \
         producer message cannot forge a frontier position"
    );
}

#[tokio::test]
async fn a_producer_retry_replays_and_a_changed_quantity_conflicts() {
    let authority = FakeAuthority::new(Category::Transfer);
    let record = RecordFact::new(authority, clock());
    let first = draft(
        Category::Transfer,
        "crossing-1",
        FactKind::Measured(measurement_for(Category::Transfer, 4_096)),
    );

    let Admission::Admitted(original) = record.execute(&first).await.expect("admits") else {
        panic!("the first offer is an admission");
    };
    let Admission::Replayed(replayed) = record.execute(&first).await.expect("replays") else {
        panic!("an identical retry is a replay, not a second row");
    };
    assert_eq!(original.fact_id, replayed.fact_id);
    assert_eq!(original.accepted_sequence, replayed.accepted_sequence);

    // The same identity with a different measurement is a producer defect, and
    // two quantities cannot both be true of one physical measurement.
    let drifted = draft(
        Category::Transfer,
        "crossing-1",
        FactKind::Measured(measurement_for(Category::Transfer, 8_192)),
    );
    let outcome = record.execute(&drifted).await.expect("resolves");
    assert!(
        matches!(outcome, Admission::IdentityConflict { .. }),
        "{outcome:?}"
    );
}

#[tokio::test]
async fn a_foreign_category_draft_never_reaches_the_store() {
    let authority = FakeAuthority::new(Category::Storage);
    let record = RecordFact::new(authority.clone(), clock());
    let foreign = draft(
        Category::Compute,
        "res-1",
        FactKind::Measured(measurement_for(Category::Compute, 4_096)),
    );
    let error = record.execute(&foreign).await.expect_err("is refused");
    assert!(matches!(error, UseCaseError::CategoryEscape { .. }));
    assert_eq!(authority.outbox_depth(), 0, "nothing was written");
}

#[tokio::test]
async fn a_retryable_store_failure_is_not_a_reason_to_park_a_workspace() {
    let authority = FakeAuthority::new(Category::Storage);
    let projection = FakeProjection::new();
    let fact = fact(Category::Storage, 1);
    authority.seed(&fact);
    authority.set_unavailable(true);

    let project = ProjectCategory::new(authority.clone(), projection);
    let error = project.execute(&fact).await.expect_err("is unavailable");
    assert!(
        error.poison().is_none(),
        "a throttled table is transient; parking would stall a paying workspace"
    );
    assert!(matches!(
        error,
        UseCaseError::Port(PortError::Unavailable { .. })
    ));
    assert!(authority.parked().is_empty());
}

#[tokio::test]
async fn a_projection_outage_leaves_the_frontier_exactly_where_it_was() {
    let authority = FakeAuthority::new(Category::Compute);
    let projection = FakeProjection::new();
    let fact = fact(Category::Compute, 1);
    authority.seed(&fact);
    projection.set_unavailable(true);

    let project = ProjectCategory::new(authority.clone(), projection.clone());
    assert!(project.execute(&fact).await.is_err());
    assert_eq!(
        authority.frontier_of(&workspace()).projected.get(),
        0,
        "the frontier never advances past a fold that did not happen"
    );

    projection.set_unavailable(false);
    assert_eq!(
        project.execute(&fact).await.expect("folds"),
        Projected::Folded
    );
    assert_eq!(authority.frontier_of(&workspace()).projected.get(), 1);
}

#[tokio::test]
async fn publishing_is_deferred_by_an_outage_and_never_loses_the_marker() {
    let authority = FakeAuthority::new(Category::Storage);
    let queue = FakeQueue::new();
    let projection = FakeProjection::new();
    let fact = fact(Category::Storage, 1);
    authority.seed(&fact);
    ProjectCategory::new(authority.clone(), projection)
        .execute(&fact)
        .await
        .expect("folds");

    queue.set_outage(true);
    let publish = PublishOutbox::new(authority.clone(), queue.clone());
    assert_eq!(
        publish.execute(&fact).await.expect("defers"),
        Published::Deferred
    );
    assert_eq!(
        authority.outbox_depth(),
        1,
        "the marker survives, so the sweep can find it"
    );
    assert_eq!(authority.frontier_of(&workspace()).published.get(), 0);

    queue.set_outage(false);
    assert_eq!(
        publish.execute(&fact).await.expect("delivers"),
        Published::Delivered
    );
    assert_eq!(authority.outbox_depth(), 0);
    assert_eq!(authority.frontier_of(&workspace()).published.get(), 1);
    assert_eq!(
        publish.execute(&fact).await.expect("is a no-op"),
        Published::AlreadyPublished
    );
}

#[tokio::test]
async fn a_sweep_never_reads_more_than_its_page_budget() {
    let authority = FakeAuthority::new(Category::Storage);
    let queue = FakeQueue::new();
    for sequence in 1..=10 {
        authority.seed(&fact(Category::Storage, sequence));
    }
    let sweep = SweepOutbox::new(authority.clone(), queue, clock());
    let shard = crate::testing::doubles::shard_of(&workspace());
    let report = sweep
        .execute(shard, at(700_000), 4, 10)
        .await
        .expect("sweeps");
    assert_eq!(
        report.inspected, 4,
        "a page budget that never binds is not a budget"
    );
}

#[tokio::test]
async fn a_rebuild_writes_the_next_generation_without_touching_the_live_one() {
    let authority = FakeAuthority::new(Category::Storage);
    let projection = FakeProjection::new();
    let facts: Vec<_> = (1..=4).map(|seq| fact(Category::Storage, seq)).collect();
    for fact in &facts {
        authority.seed(fact);
    }

    // Fold the live generation first.
    let project = ProjectCategory::new(authority.clone(), projection.clone());
    for fact in &facts {
        project.execute(fact).await.expect("folds");
    }
    let live_total = projection.hourly_total();
    let live_rows: Vec<(String, String)> = projection
        .snapshot()
        .keys()
        .filter(|key| key.0.starts_with("G0000"))
        .cloned()
        .collect();

    let next = Generation::FIRST.next().expect("advances");
    let rebuild = RebuildProjection::new(authority.clone(), projection.clone());
    let report = rebuild.execute(&workspace(), next).await.expect("rebuilds");
    assert_eq!(report.folded, 4);
    assert_eq!(report.highest, 4);

    // Generation n is byte-identical; generation n+1 carries the same totals.
    let after = projection.snapshot();
    for key in &live_rows {
        assert!(after.contains_key(key), "generation n row {key:?} vanished");
    }
    let rebuilt_total: i128 = after
        .iter()
        .filter(|(key, _)| key.0.starts_with("G0001") && key.1.starts_with("H#"))
        .filter_map(|(_, row)| row.counters.get("quantity").copied())
        .sum();
    assert_eq!(
        rebuilt_total, live_total,
        "a rebuild that disagreed with the generation it replaces would be the \
         one thing a cutover must never do"
    );

    // Running it again is a resume, not a double count.
    let resumed = rebuild.execute(&workspace(), next).await.expect("resumes");
    assert_eq!(resumed.folded, 0);
    assert_eq!(resumed.already_covered, 4);
    let unchanged: i128 = projection
        .snapshot()
        .iter()
        .filter(|(key, _)| key.0.starts_with("G0001") && key.1.starts_with("H#"))
        .filter_map(|(_, row)| row.counters.get("quantity").copied())
        .sum();
    assert_eq!(unchanged, rebuilt_total);
}

#[tokio::test]
async fn a_rebuild_reproduces_a_corrected_history_exactly() {
    let authority = FakeAuthority::new(Category::Compute);
    let projection = FakeProjection::new();
    let target = fact(Category::Compute, 1);
    let void = void_fact(Category::Compute, 2, &target.fact_id);
    authority.seed(&target);
    authority.seed(&void);

    let next = Generation::FIRST.next().expect("advances");
    RebuildProjection::new(authority, projection.clone())
        .execute(&workspace(), next)
        .await
        .expect("rebuilds");

    let total: i128 = projection
        .snapshot()
        .iter()
        .filter(|(key, _)| key.0.starts_with("G0001") && key.1.starts_with("H#"))
        .filter_map(|(_, row)| row.counters.get("quantity").copied())
        .sum();
    assert_eq!(
        total, 0,
        "the rebuild must re-apply the void, not just the measurement"
    );
}

#[tokio::test]
async fn a_customer_read_of_a_new_generation_sees_the_pointer_move() {
    let projection = FakeProjection::new();
    assert_eq!(
        projection.current_generation().await.expect("reads"),
        Generation::FIRST
    );
    let next = Generation::FIRST.next().expect("advances");
    projection.set_generation(next);
    assert_eq!(projection.current_generation().await.expect("reads"), next);
}

#[tokio::test]
async fn settling_copies_the_settled_position_into_the_coverage_row() {
    let authority = FakeAuthority::new(Category::Storage);
    let projection = FakeProjection::new();
    let fact = fact(Category::Storage, 1);
    authority.seed(&fact);
    ProjectCategory::new(authority.clone(), projection.clone())
        .execute(&fact)
        .await
        .expect("folds");
    PublishOutbox::new(authority.clone(), FakeQueue::new())
        .execute(&fact)
        .await
        .expect("publishes");

    ApplyReceipt::new(authority, projection.clone())
        .execute(&SettlementReceipt {
            receipt_id: "rcpt-1".to_owned(),
            region: fact.region.clone(),
            category: fact.category(),
            fact_id: fact.fact_id.clone(),
            rated_microusd: 0,
            transaction_id: "txn-1".to_owned(),
            pricing_version: pricing(),
            settled_at: at(900_000),
            workspace: None,
            accepted_sequence: None,
        })
        .await
        .expect("settles");

    let coverage = projection
        .row(
            &format!("G0000#{}#storage", crate::testing::workspace()),
            "COVERAGE",
        )
        .expect("the coverage row exists");
    assert_eq!(
        coverage.values["settledSequence"],
        aex_usage_domain::keys::ItemValue::number(1_u64),
        "the customer view is what tells them settlement caught up"
    );
    assert!(coverage.values.contains_key("settledThrough"));
}

#[test]
fn each_authority_reports_coverage_under_exactly_one_public_category() {
    assert_eq!(
        coverage_category(Category::Storage),
        PublicCategory::Storage
    );
    assert_eq!(
        coverage_category(Category::Compute),
        PublicCategory::Compute
    );
    assert_eq!(
        coverage_category(Category::Transfer),
        PublicCategory::DataTransfer
    );
    // Memory has no coverage row of its own: it shares compute's sequence.
    assert_eq!(PublicCategory::Memory.category(), Category::Compute);
}

#[test]
fn a_sequence_is_one_based_so_zero_can_mean_nothing_admitted() {
    assert!(AcceptedSequence::new(0).is_err());
    assert_eq!(AcceptedSequence::ORIGIN.get(), 0);
}
