//! Fold properties.
//!
//! Every assertion here is about a rule that costs money when it is wrong: what
//! a rollup accumulates, what a correction withdraws, what the fence is written
//! against, and what never reaches a customer-readable row.

use super::{
    DIMENSION_HASH_HEX, DimensionTuple, FoldContext, FoldError, ProjectionCondition, ProjectionRow,
    Sign, SignedDelta, fold_fact, quantity_total, signed,
};
use crate::testing::{
    fact, frontier_at, measurement_for, observability_fact, replace_fact, void_fact,
};
use aex_usage_domain::fact::UsageFact;
use aex_usage_domain::frontier::Frontier;
use aex_usage_domain::keys::ItemValue;
use aex_usage_domain::meter::{Category, PublicCategory};
use aex_usage_domain::projection::Generation;

fn context<'a>(frontier: &'a Frontier, target: Option<&'a UsageFact>) -> FoldContext<'a> {
    FoldContext {
        generation: Generation::FIRST,
        frontier,
        target,
    }
}

#[test]
fn a_measured_fact_folds_into_two_rollups_one_detail_and_the_fence() {
    let fact = fact(Category::Storage, 1);
    let frontier = frontier_at(Category::Storage, 1, 0);
    let transaction = fold_fact(&fact, context(&frontier, None)).expect("folds");

    let rows: Vec<ProjectionRow> = transaction.writes.iter().map(|write| write.row).collect();
    assert_eq!(
        rows,
        vec![
            ProjectionRow::HourlyAggregate,
            ProjectionRow::DailyAggregate,
            ProjectionRow::Detail,
            ProjectionRow::Coverage,
        ],
        "the coverage fence is always last, so a partial apply is impossible"
    );

    let quantity = fact.kind.measurement().expect("measured").quantity().get();
    for write in &transaction.writes[..2] {
        assert_eq!(
            write.add["quantity"].magnitude, quantity,
            "a rollup accumulates the exact measured quantity"
        );
        assert_eq!(write.add["quantity"].sign, Sign::Add);
        assert_eq!(write.add["factCount"].magnitude, 1);
    }
    assert!(
        transaction.writes[2].add.is_empty(),
        "a detail row is a put"
    );
}

#[test]
fn the_hourly_and_daily_rollups_share_a_partition_and_a_group() {
    let fact = fact(Category::Compute, 1);
    let frontier = frontier_at(Category::Compute, 1, 0);
    let transaction = fold_fact(&fact, context(&frontier, None)).expect("folds");
    let hourly = &transaction.writes[0];
    let daily = &transaction.writes[1];

    assert_eq!(hourly.key.pk, daily.key.pk, "one query reads both rollups");
    let hash = DimensionTuple::of(&fact)
        .expect("measured")
        .hash()
        .expect("hashes");
    for write in [hourly, daily] {
        let sort = write.key.sk.as_deref().expect("a rollup has a sort key");
        assert!(
            sort.ends_with(&hash),
            "`{sort}` must be grouped by the declared dimension tuple"
        );
    }
    assert_ne!(hourly.key.sk, daily.key.sk);
}

#[test]
fn the_dimension_tuple_is_stored_so_a_reader_never_inverts_the_hash() {
    let fact = fact(Category::Transfer, 1);
    let frontier = frontier_at(Category::Transfer, 1, 0);
    let transaction = fold_fact(&fact, context(&frontier, None)).expect("folds");
    let stored = transaction.writes[0]
        .set
        .get("dimensions")
        .expect("the tuple is stored");
    assert_eq!(
        stored,
        &DimensionTuple::of(&fact).expect("measured").to_item()
    );
}

#[test]
fn the_dimension_hash_is_stable_and_separates_distinct_tuples() {
    let first = DimensionTuple::of(&fact(Category::Storage, 1)).expect("measured");
    let again = DimensionTuple::of(&fact(Category::Storage, 1)).expect("measured");
    assert_eq!(first.hash().expect("hashes"), again.hash().expect("hashes"));
    assert_eq!(first.hash().expect("hashes").len(), DIMENSION_HASH_HEX);

    let mut other = first.clone();
    other.basis = "reserved".to_owned();
    assert_ne!(
        first.hash().expect("hashes"),
        other.hash().expect("hashes"),
        "reserved and consumed are different provenance and must not merge"
    );
}

#[test]
fn every_transaction_carries_exactly_one_coverage_fence_at_the_previous_position() {
    let fact = fact(Category::Storage, 4);
    let frontier = frontier_at(Category::Storage, 4, 3);
    let transaction = fold_fact(&fact, context(&frontier, None)).expect("folds");

    let fences = transaction
        .writes
        .iter()
        .filter(|write| write.row == ProjectionRow::Coverage)
        .count();
    assert_eq!(fences, 1);
    assert_eq!(
        transaction.fence().condition,
        ProjectionCondition::ProjectedSequenceIs {
            previous: frontier.projected
        },
        "the fence is what makes a redelivered record a no-op"
    );
    assert_eq!(
        transaction.fence().set["projectedSequence"],
        ItemValue::number(4u64)
    );
}

#[test]
fn a_fact_folded_out_of_order_is_refused_rather_than_fenced_against_a_wrong_position() {
    let fact = fact(Category::Storage, 5);
    let frontier = frontier_at(Category::Storage, 5, 3);
    assert!(matches!(
        fold_fact(&fact, context(&frontier, None)),
        Err(FoldError::FrontierMisaligned {
            projected: 3,
            sequence: 5
        })
    ));
}

#[test]
fn a_zero_dollar_observability_fact_folds_into_no_priced_rollup() {
    let fact = observability_fact(1);
    let frontier = frontier_at(Category::Compute, 1, 0);
    let transaction = fold_fact(&fact, context(&frontier, None)).expect("folds");

    assert_eq!(
        transaction.writes.len(),
        1,
        "model tokens are zero-dollar; the only effect is a frontier advance"
    );
    assert_eq!(transaction.writes[0].row, ProjectionRow::Coverage);
    assert!(quantity_total(std::slice::from_ref(&transaction)).is_empty());
}

#[test]
fn a_void_subtracts_its_target_and_deactivates_the_target_detail_row() {
    let target = fact(Category::Compute, 1);
    let void = void_fact(Category::Compute, 2, &target.fact_id);
    let frontier = frontier_at(Category::Compute, 2, 1);
    let transaction =
        fold_fact(&void, context(&frontier, Some(&target))).expect("folds with its target");

    let rows: Vec<ProjectionRow> = transaction.writes.iter().map(|write| write.row).collect();
    assert_eq!(
        rows,
        vec![
            ProjectionRow::HourlyAggregate,
            ProjectionRow::DailyAggregate,
            ProjectionRow::DetailWithdrawal,
            ProjectionRow::Coverage,
        ],
        "a void carries no quantity of its own; it only withdraws its target"
    );
    let quantity = target
        .kind
        .measurement()
        .expect("measured")
        .quantity()
        .get();
    assert_eq!(transaction.writes[0].add["quantity"].sign, Sign::Subtract);
    assert_eq!(transaction.writes[0].add["quantity"].magnitude, quantity);
    assert_eq!(transaction.writes[0].add["factCount"].sign, Sign::Subtract);
    assert_eq!(
        transaction.writes[2].set["active"],
        ItemValue::Bool(false),
        "the row stays visible as withdrawn rather than vanishing"
    );
    assert_eq!(transaction.writes[2].condition, ProjectionCondition::Exists);
}

#[test]
fn a_measurement_and_its_void_cancel_exactly() {
    let target = fact(Category::Storage, 1);
    let void = void_fact(Category::Storage, 2, &target.fact_id);
    let first = frontier_at(Category::Storage, 2, 0);
    let second = frontier_at(Category::Storage, 2, 1);
    let folded = vec![
        fold_fact(&target, context(&first, None)).expect("folds"),
        fold_fact(&void, context(&second, Some(&target))).expect("folds"),
    ];
    let totals = quantity_total(&folded);
    assert!(!totals.is_empty(), "the fold must have moved something");
    for total in totals.values() {
        assert_eq!(
            *total, 0,
            "a void must leave the rollup exactly where it was"
        );
    }
}

#[test]
fn a_replace_adds_its_own_measurement_and_withdraws_the_target() {
    let target = fact(Category::Transfer, 1);
    let replacement = replace_fact(Category::Transfer, 2, &target.fact_id, 7_777);
    let frontier = frontier_at(Category::Transfer, 2, 1);
    let transaction =
        fold_fact(&replacement, context(&frontier, Some(&target))).expect("folds with its target");

    let rows: Vec<ProjectionRow> = transaction.writes.iter().map(|write| write.row).collect();
    assert_eq!(
        rows,
        vec![
            ProjectionRow::HourlyAggregate,
            ProjectionRow::DailyAggregate,
            ProjectionRow::Detail,
            ProjectionRow::HourlyAggregate,
            ProjectionRow::DailyAggregate,
            ProjectionRow::DetailWithdrawal,
            ProjectionRow::Coverage,
        ]
    );
    let net: i128 = quantity_total(std::slice::from_ref(&transaction))
        .values()
        .sum();
    let expected = 7_777_i128
        - i128::try_from(
            target
                .kind
                .measurement()
                .expect("measured")
                .quantity()
                .get(),
        )
        .expect("in range");
    assert_eq!(
        net, expected,
        "the rollup moves by the difference, not by the restated value"
    );
    assert_eq!(
        transaction.writes[2].set["correctionOf"],
        ItemValue::text(target.fact_id.to_string()),
        "public detail shows the correction lineage"
    );
}

#[test]
fn a_correction_without_its_target_is_refused_rather_than_folded_blind() {
    let target = fact(Category::Compute, 1);
    let void = void_fact(Category::Compute, 2, &target.fact_id);
    let frontier = frontier_at(Category::Compute, 2, 1);
    assert!(matches!(
        fold_fact(&void, context(&frontier, None)),
        Err(FoldError::TargetRequired { .. })
    ));

    let wrong = fact(Category::Compute, 9);
    assert!(matches!(
        fold_fact(&void, context(&frontier, Some(&wrong))),
        Err(FoldError::TargetMismatch { .. })
    ));
}

#[test]
fn a_rollup_can_only_ever_accumulate_its_own_public_category() {
    for category in Category::ALL {
        let fact = fact(category, 1);
        let frontier = frontier_at(category, 1, 0);
        let transaction = fold_fact(&fact, context(&frontier, None)).expect("folds");
        let expected = fact.public_category().expect("measured");
        assert_eq!(
            transaction.writes[0].condition,
            ProjectionCondition::CategoryMatches { category: expected }
        );
        assert!(
            transaction.writes[0].key.pk.contains(expected.id()),
            "the category is in the partition key as well as the condition"
        );
    }
}

#[test]
fn compute_and_memory_are_two_public_categories_in_one_authority() {
    // A memory fact and a compute fact share a frontier but never a rollup: the
    // projection reports four public categories over three authorities.
    let memory = fact(Category::Compute, 1);
    assert_eq!(memory.public_category(), Some(PublicCategory::Memory));
    let frontier = frontier_at(Category::Compute, 1, 0);
    let transaction = fold_fact(&memory, context(&frontier, None)).expect("folds");
    assert!(transaction.writes[0].key.pk.contains("#memory#"));
    assert!(
        transaction.fence().key.pk.ends_with("#compute"),
        "the coverage row is per authority sequence, not per public category"
    );
}

#[test]
fn a_detail_row_carries_both_halves_of_the_ttl_pair() {
    let fact = fact(Category::Storage, 1);
    let frontier = frontier_at(Category::Storage, 1, 0);
    let transaction = fold_fact(&fact, context(&frontier, None)).expect("folds");
    let detail = &transaction.writes[2];
    assert!(detail.set.contains_key("expiresAtEpochSeconds"));
    assert!(
        detail.set.contains_key("expiresAt"),
        "TTL is never the fence, so the reader gets an explicit instant to check"
    );
    assert_eq!(detail.condition, ProjectionCondition::WriteOnce);
}

#[test]
fn no_projection_row_carries_provider_evidence() {
    // The projection is customer-readable. Cgroup readings, receipt bodies and
    // internal amplification factors stay in the authority.
    let fact = fact(Category::Compute, 1);
    let frontier = frontier_at(Category::Compute, 1, 0);
    let transaction = fold_fact(&fact, context(&frontier, None)).expect("folds");
    for write in &transaction.writes {
        for forbidden in [
            "evidence",
            "sourceReceiptId",
            "intentHash",
            "physicalUs",
            "attributedUs",
            "amplification",
        ] {
            assert!(
                !write.set.contains_key(forbidden),
                "`{forbidden}` is provider evidence and must not reach the projection"
            );
        }
    }
}

#[test]
fn folding_the_same_fact_twice_produces_an_identical_transaction() {
    let fact = fact(Category::Storage, 3);
    let frontier = frontier_at(Category::Storage, 3, 2);
    assert_eq!(
        fold_fact(&fact, context(&frontier, None)).expect("folds"),
        fold_fact(&fact, context(&frontier, None)).expect("folds"),
        "a redelivered record must produce the same writes, or the fence would \
         be masking a real difference"
    );
}

#[test]
fn two_generations_never_share_a_row() {
    let fact = fact(Category::Storage, 1);
    let frontier = frontier_at(Category::Storage, 1, 0);
    let live = fold_fact(
        &fact,
        FoldContext {
            generation: Generation::FIRST,
            frontier: &frontier,
            target: None,
        },
    )
    .expect("folds");
    let rebuilt = fold_fact(
        &fact,
        FoldContext {
            generation: Generation::FIRST.next().expect("advances"),
            frontier: &frontier,
            target: None,
        },
    )
    .expect("folds");
    for (old, new) in live.writes.iter().zip(rebuilt.writes.iter()) {
        assert_ne!(
            old.key, new.key,
            "a rebuild must not touch the generation still being read"
        );
    }
}

#[test]
fn a_frontier_from_another_authority_cannot_fence_a_fact() {
    let fact = fact(Category::Storage, 1);
    let foreign = frontier_at(Category::Compute, 1, 0);
    assert!(matches!(
        fold_fact(&fact, context(&foreign, None)),
        Err(FoldError::FrontierForeign { .. })
    ));
}

#[test]
fn a_signed_delta_renders_exactly_as_an_add_clause_reads_it() {
    assert_eq!(SignedDelta::add(42).render(), "42");
    assert_eq!(SignedDelta::subtract(42).render(), "-42");
    assert_eq!(SignedDelta::add(0).render(), "0");
    let quantity = measurement_for(Category::Storage, 4_096).quantity();
    assert_eq!(
        signed(quantity, Sign::Subtract).render(),
        format!("-{quantity}")
    );
}
