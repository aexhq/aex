//! Slice S-1.6 — the budget algebra: `I1`, `I2`, exact conservation, roll-up, structure.

use aex_brain_domain::budget::{
    BudgetError, BudgetGrant, BudgetNode, DIMENSIONS, Dimension, DimensionVector, Kind,
    StructuralLimits, launch_session_grant,
};
use aex_brain_test_support::journal_gen::only;
use proptest::prelude::*;

fn arb_vector() -> impl Strategy<Value = DimensionVector> {
    proptest::array::uniform7(0_u64..1_000).prop_map(DimensionVector)
}

proptest! {
    /// `I1` — `reserved + used <= limit`, every node, every dimension, after every legal
    /// operation.
    #[test]
    fn i1_holds_after_every_legal_operation(
        limit in arb_vector(),
        grants in proptest::collection::vec(arb_vector(), 0..6),
        charges in proptest::collection::vec((0_usize..7, 0_u64..200), 0..12),
    ) {
        let mut node = BudgetNode::root(limit);
        let structural = StructuralLimits::default();
        let mut children = Vec::new();
        for grant in grants {
            if let Ok(child) = node.spawn(grant, structural) {
                children.push(child);
            }
            prop_assert!(node.invariant_i1(), "{node:?}");
        }
        for (index, quantity) in charges {
            let _ = node.consume(DIMENSIONS[index], quantity);
            prop_assert!(node.invariant_i1(), "{node:?}");
        }
        for child in &children {
            let _ = node.release_child(child);
            prop_assert!(node.invariant_i1(), "{node:?}");
        }
    }

    /// `I2` — a grant never exceeds the parent's free headroom at grant time, and a spawn
    /// that would break it is refused rather than clamped.
    #[test]
    fn i2_is_enforced_at_grant_time(limit in arb_vector(), grant in arb_vector()) {
        let mut node = BudgetNode::root(limit);
        let free_before: Vec<u64> = DIMENSIONS.iter().map(|&d| node.free(d)).collect();
        let fits = DIMENSIONS
            .iter()
            .enumerate()
            .all(|(index, &dimension)| grant.get(dimension) <= free_before[index]);
        match node.spawn(grant, StructuralLimits::default()) {
            Ok(child) => {
                prop_assert!(fits, "a spawn that does not fit must not succeed");
                prop_assert_eq!(child.limit, grant);
                prop_assert_eq!(child.reserved, DimensionVector::ZERO);
                prop_assert_eq!(child.used, DimensionVector::ZERO);
                for &dimension in &DIMENSIONS {
                    prop_assert_eq!(
                        node.reserved.get(dimension),
                        grant.get(dimension),
                        "{:?}",
                        dimension
                    );
                }
            }
            Err(BudgetError::Exhausted { .. }) => prop_assert!(!fits),
            Err(other) => return Err(TestCaseError::fail(format!("{other:?}"))),
        }
    }

    /// Concurrent dimensions are exactly conserved across the tree: whatever a child was
    /// granted comes back in full when it terminates.
    #[test]
    fn concurrent_dimensions_are_exactly_conserved(
        limit in arb_vector(),
        grant in arb_vector(),
        child_used in arb_vector(),
    ) {
        let mut node = BudgetNode::root(limit);
        let before = node;
        let Ok(mut child) = node.spawn(grant, StructuralLimits::default()) else {
            return Ok(());
        };
        for &dimension in &DIMENSIONS {
            let _ = child.consume(dimension, child_used.get(dimension));
        }
        node.release_child(&child).expect("a spawned child releases");
        for &dimension in &DIMENSIONS {
            prop_assert_eq!(
                node.reserved.get(dimension),
                before.reserved.get(dimension),
                "{:?} reservation must return in full",
                dimension
            );
            match dimension.kind() {
                // Conserved: the child's actual use rolls up; the unspent remainder returns.
                Kind::Conserved => prop_assert_eq!(
                    node.used.get(dimension),
                    before.used.get(dimension) + child.used.get(dimension),
                    "{:?} must roll up",
                    dimension
                ),
                // Concurrent: nothing rolls up at all.
                Kind::Concurrent => prop_assert_eq!(
                    node.used.get(dimension),
                    before.used.get(dimension),
                    "{:?} must not roll up",
                    dimension
                ),
            }
        }
    }
}

/// Unspent conserved budget returns to the parent automatically.
#[test]
fn unspent_conserved_budget_returns_to_the_parent() {
    let mut parent = BudgetNode::root(only(Dimension::CostMicroUsd, 1_000));
    let mut child = parent
        .spawn(
            only(Dimension::CostMicroUsd, 400),
            StructuralLimits::default(),
        )
        .expect("400 of 1000 fits");
    assert_eq!(parent.free(Dimension::CostMicroUsd), 600);

    child
        .consume(Dimension::CostMicroUsd, 150)
        .expect("the child spends 150 of its 400");
    parent.release_child(&child).expect("the child terminates");

    // 400 was reserved, 150 was actually spent: 250 comes back without any explicit refund.
    assert_eq!(parent.used.get(Dimension::CostMicroUsd), 150);
    assert_eq!(parent.reserved.get(Dimension::CostMicroUsd), 0);
    assert_eq!(parent.free(Dimension::CostMicroUsd), 850);
}

/// A release of a grant that was never reserved here is refused rather than patched over.
#[test]
fn releasing_an_unreserved_grant_is_refused() {
    let mut parent = BudgetNode::root(only(Dimension::ActiveChildren, 4));
    let stranger = BudgetNode::root(only(Dimension::ActiveChildren, 2));
    let error = parent
        .release_child(&stranger)
        .expect_err("the tree already diverged; do not patch over it");
    assert!(
        matches!(error, BudgetError::ReleaseUnderflow { .. }),
        "{error:?}"
    );
}

/// Depth and fanout are structural: checked at spawn, never accumulated.
#[test]
fn the_structural_limits_are_checked_not_accumulated() {
    let structural = StructuralLimits {
        max_depth: 2,
        max_fanout: 8,
    };
    let mut root = BudgetNode::root(DimensionVector::uniform(1_000));
    let mut depth_one = root
        .spawn(DimensionVector::uniform(10), structural)
        .expect("depth 1 is admitted");
    let mut depth_two = depth_one
        .spawn(DimensionVector::uniform(5), structural)
        .expect("depth 2 is admitted");
    let error = depth_two
        .spawn(DimensionVector::uniform(1), structural)
        .expect_err("depth 3 exceeds the limit");
    assert!(
        matches!(
            error,
            BudgetError::DepthExceeded {
                depth: 3,
                max_depth: 2
            }
        ),
        "{error:?}"
    );

    BudgetNode::check_fanout(8, structural).expect("the limit itself is admitted");
    let error = BudgetNode::check_fanout(9, structural).expect_err("one over is refused");
    assert!(
        matches!(
            error,
            BudgetError::FanoutExceeded {
                requested: 9,
                max_fanout: 8
            }
        ),
        "{error:?}"
    );
}

/// Under BYOK a model call consumes `provider_calls`, never `cost_micro_usd`.
///
/// Model tokens are zero-dollar observability facts, so charging them to the money
/// dimension would bill a customer for their own provider account.
#[test]
fn a_model_call_consumes_provider_calls_and_not_money() {
    let mut node = BudgetNode::root(launch_session_grant(50_000));
    let before = node.used.get(Dimension::CostMicroUsd);
    node.consume(Dimension::ProviderCalls, 1)
        .expect("a provider call is admitted");
    assert_eq!(node.used.get(Dimension::ProviderCalls), 1);
    assert_eq!(node.used.get(Dimension::CostMicroUsd), before);
}

/// The nine dimensions split exactly as the plan states: seven accumulate, two are
/// structural.
#[test]
fn the_dimension_taxonomy_matches_the_decision_record() {
    assert_eq!(DIMENSIONS.len(), 7);
    let conserved: Vec<Dimension> = DIMENSIONS
        .into_iter()
        .filter(|dimension| dimension.kind() == Kind::Conserved)
        .collect();
    let concurrent: Vec<Dimension> = DIMENSIONS
        .into_iter()
        .filter(|dimension| dimension.kind() == Kind::Concurrent)
        .collect();
    assert_eq!(
        conserved,
        vec![
            Dimension::TotalChildrenCreated,
            Dimension::ProviderCalls,
            Dimension::HandsCalls,
            Dimension::CostMicroUsd
        ]
    );
    assert_eq!(
        concurrent,
        vec![
            Dimension::ActiveChildren,
            Dimension::QueuedChildren,
            Dimension::RetainedResultBytes
        ]
    );
    // Indices are load-bearing: a vector index must never move under a stored record.
    for (index, dimension) in DIMENSIONS.into_iter().enumerate() {
        assert_eq!(dimension.index(), index, "{dimension:?}");
    }
}

/// Exhaustion in each dimension is reported against that dimension, not a generic error.
#[test]
fn exhaustion_names_the_dimension_that_is_binding() {
    for &dimension in &DIMENSIONS {
        let mut node = BudgetNode::root(only(dimension, 1));
        node.consume(dimension, 1).expect("the first unit fits");
        let error = node
            .consume(dimension, 1)
            .expect_err("the second unit does not");
        assert!(
            matches!(error, BudgetError::Exhausted { dimension: named, .. } if named == dimension),
            "{error:?}"
        );
    }
}

// --- platform-paid spend: reserve before, reconcile after ---------------------
//
// `CostMicroUsd` is the one money dimension in the tree and it has never been
// consumed, because under BYOK a model call does not touch it. A platform-paid
// tool call is the first thing that should, and these pin the shape it touches
// it with.

#[test]
fn a_reservation_is_charged_before_the_call_and_refunded_by_what_was_not_spent() {
    let mut grant = BudgetGrant::ZERO;
    grant.set(Dimension::CostMicroUsd, 1_000);
    let mut node = BudgetNode::root(grant);

    let permit = node.reserve_spend(400).expect("the session has headroom");
    assert_eq!(permit.reserved(), 400);
    assert_eq!(
        node.free(Dimension::CostMicroUsd),
        600,
        "the reservation is charged before the call, not after it: a session that \
         only charged on settlement could overshoot by its whole concurrency width"
    );

    node.reconcile_spend(permit, 150).expect("settles");
    assert_eq!(
        node.free(Dimension::CostMicroUsd),
        850,
        "the unspent remainder returns to the session"
    );
}

#[test]
fn a_session_with_no_headroom_is_refused_before_anything_is_dispatched() {
    let mut grant = BudgetGrant::ZERO;
    grant.set(Dimension::CostMicroUsd, 100);
    let mut node = BudgetNode::root(grant);

    let held = node
        .reserve_spend(100)
        .expect("the first call fits exactly");
    let refusal = node
        .reserve_spend(1)
        .expect_err("a session with nothing left must be refused");
    assert!(
        matches!(
            refusal,
            BudgetError::Exhausted {
                dimension: Dimension::CostMicroUsd,
                ..
            }
        ),
        "{refusal:?}"
    );
    node.reconcile_spend(held, 100).expect("settles");
}

#[test]
fn a_reconciliation_larger_than_its_reservation_is_refused_and_stays_charged() {
    // The reservation is what authorised the call, so a larger actual means money
    // left that was never granted. Refunding would credit the session for money it
    // spent; charging the excess would let a caller raise its own ceiling by
    // reporting a bigger number afterwards. Neither: the reservation stays charged
    // in full and the caller is told.
    let mut grant = BudgetGrant::ZERO;
    grant.set(Dimension::CostMicroUsd, 1_000);
    let mut node = BudgetNode::root(grant);

    let permit = node.reserve_spend(200).expect("headroom");
    let refusal = node
        .reconcile_spend(permit, 500)
        .expect_err("an overspend must be refused");
    assert_eq!(
        refusal,
        BudgetError::ReconcileOverspend {
            reserved: 200,
            actual: 500,
        }
    );
    assert_eq!(
        node.free(Dimension::CostMicroUsd),
        800,
        "the reservation stays charged in full"
    );
}

#[test]
fn spend_is_conserved_up_the_tree_rather_than_copied_to_every_child() {
    // 256 subagents share one session's money. The property is inherited from the
    // dimension's own arithmetic rather than added here, which is the reason this
    // extends the budget instead of building a second bound beside it.
    let mut grant = BudgetGrant::ZERO;
    grant.set(Dimension::CostMicroUsd, 1_000);
    let mut root = BudgetNode::root(grant);

    let mut child_grant = BudgetGrant::ZERO;
    child_grant.set(Dimension::CostMicroUsd, 600);
    let mut child = root
        .spawn(child_grant, StructuralLimits::default())
        .expect("the child fits");

    assert_eq!(
        root.free(Dimension::CostMicroUsd),
        400,
        "what the child holds is not also the parent's to spend"
    );
    let permit = child.reserve_spend(600).expect("the child's whole grant");
    assert_eq!(child.free(Dimension::CostMicroUsd), 0);
    assert_eq!(
        root.free(Dimension::CostMicroUsd),
        400,
        "a child exhausting its grant does not exhaust its parent's"
    );
    child.reconcile_spend(permit, 600).expect("settles");
}
