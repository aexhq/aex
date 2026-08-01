//! Due-index distribution, priority ageing and codec round-trip properties.

mod support;

use aex_wire::types::Timestamp;
use aex_work_dynamodb::codec::{decode_work, encode_work};
use aex_work_dynamodb::keys;
use proptest::prelude::*;

use support::{record, workspace};

proptest! {
    #[test]
    fn every_work_identity_lands_inside_the_declared_shard_range(id in "wrk_[a-z0-9]{1,24}") {
        let shard = keys::shard_of(&id);
        prop_assert!(u64::from(shard) < keys::DUE_SHARDS);
        prop_assert_eq!(shard, keys::shard_of(&id));
    }

    #[test]
    fn the_due_sort_key_orders_by_effective_due_time_before_identity(
        first in 0i64..4_000_000_000_000i64,
        second in 0i64..4_000_000_000_000i64,
    ) {
        let earlier = Timestamp::from_unix_millis(first.min(second)).expect("in range");
        let later = Timestamp::from_unix_millis(first.max(second)).expect("in range");
        let low = keys::due_sort(earlier, "wrk_zzzz").expect("a key");
        let high = keys::due_sort(later, "wrk_aaaa").expect("a key");
        if earlier < later {
            prop_assert!(low < high, "{low} did not precede {high}");
        }
    }

    #[test]
    fn a_higher_priority_band_never_sorts_after_the_same_due_time_at_a_lower_band(
        millis in 2_000_000i64..4_000_000_000_000i64,
        band in 0u8..4,
    ) {
        let due = Timestamp::from_unix_millis(millis).expect("in range");
        let urgent = keys::effective_due_at(due, band).expect("a band");
        let slower = keys::effective_due_at(due, band + 1).expect("a band");
        prop_assert!(
            slower < urgent,
            "a lower band must take a longer lead, which is what ages it in"
        );
    }

    #[test]
    fn a_work_record_round_trips_for_every_attempt_and_fence(
        attempt in 0u64..1_000,
        fence in 0u64..1_000,
    ) {
        let mut original = record();
        original.attempt = attempt;
        original.max_attempts = attempt + 1;
        original.fence = fence;
        let encoded = encode_work(&original).expect("encodes");
        prop_assert_eq!(decode_work(&encoded, workspace()).expect("decodes"), original);
    }
}

#[test]
fn the_declared_kind_vocabulary_matches_the_generation_definition_payload_schemas() {
    // Every kind the table admits must have a declared payload schema, or the
    // codec would reject a row the table considers valid.
    for kind in keys::KINDS {
        assert!(
            aex_work_dynamodb::codec::payload_schema(kind).is_some(),
            "`{kind}` has no declared payload schema"
        );
    }
}

#[test]
fn the_item_type_vocabulary_matches_the_generation_definition() {
    let definition = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../migrations/regional/tables/regional-work.json"),
    )
    .expect("the generation definition is checked in");
    let document: serde_json::Value =
        serde_json::from_str(&definition).expect("the definition is JSON");
    let declared: Vec<&str> = document["itemTypes"]
        .as_array()
        .expect("an array")
        .iter()
        .map(|value| value.as_str().expect("a string"))
        .collect();
    assert_eq!(declared, keys::ITEM_TYPES);
}

#[test]
fn the_due_index_spreads_a_realistic_insert_burst_over_most_of_its_shards() {
    let mut counts = std::collections::BTreeMap::new();
    for index in 0..6_400 {
        *counts
            .entry(keys::shard_of(&format!("wrk_{index:08}")))
            .or_insert(0_usize) += 1;
    }
    assert_eq!(
        counts.len(),
        usize::try_from(keys::DUE_SHARDS).expect("64 fits"),
        "a burst must reach every shard, or the headroom analysis is wrong"
    );
    let hottest = counts.values().copied().max().expect("non-empty");
    assert!(
        hottest < 6_400 / 16,
        "one shard absorbed {hottest} of 6400 inserts"
    );
}
