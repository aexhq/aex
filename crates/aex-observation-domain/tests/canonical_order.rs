//! G1(3) canonicalization determinism and G1(4) the order tuple.

use aex_observation_domain::canonical::{CanonicalValue, batch_intent_digest, canonical_bytes};
use aex_observation_domain::order::{Direction, OrderBy, OrderTuple};
use aex_observation_domain::signal::Signal;
use aex_wire::ids::PrefixedId;
use aex_wire::types::Timestamp;
use proptest::prelude::*;
use std::collections::BTreeMap;

fn instant(millis: i64) -> Timestamp {
    Timestamp::from_unix_millis(millis).expect("fixture instant is representable")
}

/// Five distinct, valid `UUIDv7` Crockford suffixes, ascending. The generated
/// cases index into this table rather than formatting a suffix, because an
/// invented suffix is not a `UUIDv7` and would fail identifier validation.
const SUFFIXES: [&str; 5] = [
    "0000000001e40r2081040g2081",
    "0000000002e81840g2081040g2",
    "0000000003ec1r60r30c1g60r3",
    "0000000004eg2881040g208104",
    "0000000005em2ra1850m2ga185",
];

fn observation_id(suffix: &str) -> aex_wire::ids::ObservationId {
    PrefixedId::parse(&format!("obs_{suffix}")).expect("fixture observation id parses")
}

#[test]
fn map_order_does_not_change_canonical_bytes() {
    let mut forward = BTreeMap::new();
    forward.insert("b".to_owned(), CanonicalValue::Int(2));
    forward.insert("a".to_owned(), CanonicalValue::Str("x".into()));
    let mut reverse = BTreeMap::new();
    reverse.insert("a".to_owned(), CanonicalValue::Str("x".into()));
    reverse.insert("b".to_owned(), CanonicalValue::Int(2));

    let left = canonical_bytes(&CanonicalValue::Map(forward)).expect("canonicalizes");
    let right = canonical_bytes(&CanonicalValue::Map(reverse)).expect("canonicalizes");
    assert_eq!(left, right);
    assert_eq!(String::from_utf8(left).expect("utf8"), r#"{"a":"x","b":2}"#);
}

#[test]
fn canonicalization_is_deterministic_across_repeated_calls() {
    let value = CanonicalValue::Map(BTreeMap::from([
        ("z".to_owned(), CanonicalValue::Bool(true)),
        ("a".to_owned(), CanonicalValue::Null),
        (
            "nested".to_owned(),
            CanonicalValue::Array(vec![
                CanonicalValue::Int(-1),
                CanonicalValue::Str("é".into()),
            ]),
        ),
    ]));
    let first = canonical_bytes(&value).expect("canonicalizes");
    for _ in 0..8 {
        assert_eq!(canonical_bytes(&value).expect("canonicalizes"), first);
    }
}

#[test]
fn the_intent_digest_covers_the_whole_batch_and_its_binding() {
    let observations = vec![
        CanonicalValue::Str("first".into()),
        CanonicalValue::Str("second".into()),
    ];
    let binding = aex_observation_domain::canonical::BatchBinding {
        principal_id: "wsk_0000000001e40r2081040g2081",
        method: "POST",
        canonical_route: "/v1/telemetry/logs",
        workspace_id: "ws_0000000001e40r2081040g2081",
        scope: "S#ses_0000000001e40r2081040g2081",
    };
    let base = batch_intent_digest(&binding, &observations).expect("digests");

    // Reordering the observations changes the digest: order is part of the batch.
    let swapped = vec![observations[1].clone(), observations[0].clone()];
    assert_ne!(
        base,
        batch_intent_digest(&binding, &swapped).expect("digests")
    );

    // Every binding field is load-bearing.
    let mut other = binding;
    other.scope = "W#ws_0000000001e40r2081040g2081";
    assert_ne!(
        base,
        batch_intent_digest(&other, &observations).expect("digests")
    );

    // The same input digests identically, every time.
    assert_eq!(
        base,
        batch_intent_digest(&binding, &observations).expect("digests")
    );
}

#[test]
fn signal_rank_is_stable_and_total() {
    let ranks: Vec<u8> = Signal::ALL.iter().map(|signal| signal.rank()).collect();
    assert_eq!(ranks, vec![0, 1, 2, 3, 4]);
    assert_eq!(Signal::Events.rank(), 0);
    assert_eq!(Signal::Traces.rank(), 4);
    for signal in Signal::ALL {
        assert_eq!(Signal::parse(signal.as_str()), Some(*signal));
    }
    assert_eq!(Signal::parse("telemetry"), None);
}

fn tuple(primary: i64, signal: Signal, suffix: &str, revision: u64) -> OrderTuple {
    OrderTuple::new(instant(primary), signal, observation_id(suffix), revision)
}

#[test]
fn the_order_tuple_breaks_ties_by_rank_then_id_then_revision() {
    let base = tuple(1_000, Signal::Logs, "0000000001e40r2081040g2081", 0);
    let later_time = tuple(1_001, Signal::Logs, "0000000001e40r2081040g2081", 0);
    let later_rank = tuple(1_000, Signal::Spans, "0000000001e40r2081040g2081", 0);
    let later_id = tuple(1_000, Signal::Logs, "0000000002e81840g2081040g2", 0);
    let later_revision = tuple(1_000, Signal::Logs, "0000000001e40r2081040g2081", 1);

    assert!(base < later_time);
    assert!(base < later_rank);
    assert!(base < later_id);
    assert!(base < later_revision);
    assert!(later_rank < later_time, "time dominates rank");
    assert!(later_id < later_rank, "rank dominates id");
    assert!(later_revision < later_id, "id dominates revision");
}

#[test]
fn descending_traversal_is_the_exact_reverse_of_ascending() {
    let mut tuples = vec![
        tuple(3, Signal::Metrics, "0000000003ec1r60r30c1g60r3", 0),
        tuple(1, Signal::Logs, "0000000001e40r2081040g2081", 0),
        tuple(1, Signal::Logs, "0000000001e40r2081040g2081", 2),
        tuple(2, Signal::Traces, "0000000002e81840g2081040g2", 0),
    ];
    let mut ascending = tuples.clone();
    ascending.sort_by(|a, b| Direction::Ascending.compare(a, b));
    tuples.sort_by(|a, b| Direction::Descending.compare(a, b));
    let mut reversed = ascending.clone();
    reversed.reverse();
    assert_eq!(tuples, reversed);
}

#[test]
fn order_by_names_the_component_the_walk_is_keyed_on() {
    assert_eq!(OrderBy::Time.as_str(), "time");
    assert_eq!(OrderBy::Accepted.as_str(), "accepted");
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    /// The tuple is a total order: antisymmetric, transitive and total.
    #[test]
    fn the_tuple_is_a_total_order(
        raw in proptest::collection::vec(
            (0_i64..8, 0_usize..5, 0_u8..4, 0_u64..3),
            2..12,
        )
    ) {
        let built: Vec<OrderTuple> = raw
            .iter()
            .map(|(millis, signal, id, revision)| {
                let suffix = SUFFIXES[usize::from(*id)];
                tuple(*millis, Signal::ALL[*signal], suffix, *revision)
            })
            .collect();
        for left in &built {
            for right in &built {
                prop_assert_eq!(
                    left.cmp(right).reverse(),
                    right.cmp(left),
                    "antisymmetry"
                );
                if left == right {
                    prop_assert_eq!(left.cmp(right), std::cmp::Ordering::Equal);
                }
            }
        }
        let mut sorted = built.clone();
        sorted.sort();
        for window in sorted.windows(2) {
            prop_assert!(window[0] <= window[1]);
        }
    }

    /// Sorting is stable in the sense that the tuple fully determines position:
    /// sorting twice cannot move an element.
    #[test]
    fn sorting_is_idempotent(
        raw in proptest::collection::vec((0_i64..4, 0_usize..5, 0_u8..3, 0_u64..2), 1..10)
    ) {
        let built: Vec<OrderTuple> = raw
            .iter()
            .map(|(millis, signal, id, revision)| {
                let suffix = SUFFIXES[usize::from(*id)];
                tuple(*millis, Signal::ALL[*signal], suffix, *revision)
            })
            .collect();
        let mut once = built.clone();
        once.sort();
        let mut twice = once.clone();
        twice.sort();
        prop_assert_eq!(once, twice);
    }
}
