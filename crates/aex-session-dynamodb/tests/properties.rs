//! Key, ordering, codec and cursor properties.
//!
//! These are the cases that hold over arbitrary inputs rather than over one
//! example: a padded sequence orders the same way its value does, a component
//! can never smuggle a separator, and a round trip is the identity.

mod support;

use aex_session_dynamodb::component::{Component, SEQUENCE_WIDTH, due_shard, gc_bucket, sequence};
use aex_session_dynamodb::keys;
use aex_session_dynamodb::measure::{INLINE_PLAINTEXT_CEILING, Placement, placement};
use aex_session_dynamodb::paging::{CursorBinding, CursorKey, PagePosition, mint, verify};
use aex_wire::types::Timestamp;
use proptest::prelude::*;

use support::{now, organization, session, workspace};

proptest! {
    #[test]
    fn a_padded_sequence_orders_lexicographically_exactly_as_it_orders_numerically(
        left in any::<u64>(),
        right in any::<u64>(),
    ) {
        prop_assert_eq!(
            sequence(left).cmp(&sequence(right)),
            left.cmp(&right),
            "padding must not change the order of {} and {}",
            left,
            right
        );
    }

    #[test]
    fn a_padded_sequence_is_always_exactly_the_pinned_width(value in any::<u64>()) {
        prop_assert_eq!(sequence(value).len(), SEQUENCE_WIDTH);
    }

    #[test]
    fn a_journal_sort_key_orders_exactly_as_its_sequence_does(
        left in any::<u64>(),
        right in any::<u64>(),
    ) {
        prop_assert_eq!(
            keys::journal_sort_key(left).cmp(&keys::journal_sort_key(right)),
            left.cmp(&right)
        );
    }

    #[test]
    fn a_fixed_width_timestamp_orders_lexicographically_as_it_orders_chronologically(
        left in 0i64..4_000_000_000_000i64,
        right in 0i64..4_000_000_000_000i64,
    ) {
        let left_stamp = Timestamp::from_unix_millis(left).expect("in range");
        let right_stamp = Timestamp::from_unix_millis(right).expect("in range");
        prop_assert_eq!(
            left_stamp.to_wire().cmp(&right_stamp.to_wire()),
            left.cmp(&right)
        );
    }

    #[test]
    fn a_fixed_width_timestamp_round_trips_through_its_wire_spelling(
        millis in 0i64..4_000_000_000_000i64,
    ) {
        let stamp = Timestamp::from_unix_millis(millis).expect("in range");
        prop_assert_eq!(Timestamp::parse(&stamp.to_wire()).expect("parses"), stamp);
        prop_assert_eq!(stamp.to_wire().len(), 24);
    }

    #[test]
    fn a_component_accepts_exactly_the_text_that_cannot_forge_a_key(text in ".{0,80}") {
        let forbidden = text.is_empty()
            || text.len() > 256
            || text.chars().any(aex_session_dynamodb::component::is_forbidden);
        prop_assert_eq!(Component::parse(&text).is_err(), forbidden, "text: {:?}", text);
    }

    #[test]
    fn a_validated_component_never_contains_the_separator(text in "[^#\u{0}\u{ffff}]{1,64}") {
        if let Ok(component) = Component::parse(&text) {
            prop_assert!(!component.as_str().contains('#'));
        }
    }

    #[test]
    fn a_due_shard_is_always_inside_its_range(id in "[a-z0-9_]{1,32}", shards in 1u64..256) {
        let shard = due_shard(&id, shards);
        prop_assert!(shard < shards);
        prop_assert_eq!(shard, due_shard(&id, shards));
    }

    #[test]
    fn a_gc_bucket_is_always_inside_its_range(digest in "[0-9a-f]{64}", buckets in 1u16..1024) {
        let bucket = gc_bucket(&digest, buckets).expect("a hex digest");
        prop_assert!(bucket < buckets);
    }

    #[test]
    fn the_placement_boundary_never_moves(bytes in 0usize..200_000) {
        let expected = if bytes <= INLINE_PLAINTEXT_CEILING {
            Placement::Inline
        } else {
            Placement::ObjectStore
        };
        prop_assert_eq!(placement(bytes), expected);
    }

    #[test]
    fn a_cursor_round_trips_for_every_position_it_can_name(
        pk in "[A-Z]{1,8}#[a-z0-9_]{1,24}",
        sk in "[A-Z]{1,8}#[a-z0-9_]{1,24}",
    ) {
        let key = CursorKey::new(vec![3u8; 32]).expect("a long enough key");
        let binding = CursorBinding {
            resource: "sessions",
            organization: organization(),
            workspace: workspace(),
        };
        let position = PagePosition { pk, sk, index_pk: None, index_sk: None };
        let cursor = mint(&key, binding, &position, now()).expect("mints");
        prop_assert_eq!(
            verify(&key, binding, &cursor, now()).expect("verifies"),
            position
        );
    }
}

#[test]
fn every_declared_item_type_matches_the_generation_definition() {
    // The closed vocabulary in `keys` and the one in
    // `migrations/regional/tables/session-authority.json` must be the same set,
    // because the codec rejects anything outside it and the table definition is
    // what a reviewer reads.
    let definition = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../migrations/regional/tables/session-authority.json"),
    )
    .expect("the generation definition is checked in");
    let document: serde_json::Value =
        serde_json::from_str(&definition).expect("the definition is JSON");
    let declared: Vec<&str> = document["itemTypes"]
        .as_array()
        .expect("itemTypes is an array")
        .iter()
        .map(|value| value.as_str().expect("an item type is a string"))
        .collect();
    assert_eq!(declared, keys::ITEM_TYPES);
}

#[test]
fn a_session_key_and_an_agent_key_can_never_land_in_the_same_partition() {
    let session = session();
    let head = keys::head(session);
    let control = keys::agent_control(session, support::root_agent());
    assert_ne!(head.pk, control.pk);
    assert!(control.pk.starts_with("AGENT#"));
}
