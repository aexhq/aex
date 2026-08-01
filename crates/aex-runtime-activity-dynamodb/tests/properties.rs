//! Key and codec properties for `runtime-activity`.

mod support;

use aex_runtime_activity_dynamodb::codec::{
    decode_generation, decode_intent, decode_receipt, encode_generation, encode_intent,
    encode_probe, encode_receipt,
};
use aex_runtime_activity_dynamodb::keys;
use aex_runtime_control::generation::GenerationState;
use aex_wire::types::Timestamp;
use proptest::prelude::*;

use support::{generation, head, intent, probe, receipt, workspace};

proptest! {
    #[test]
    fn a_generation_head_round_trips_over_arbitrary_open_operation_counts(
        open in 0_u32..10_000,
    ) {
        let mut original = head(GenerationState::Running);
        original.open_operations = open;
        let encoded = encode_generation(&original).expect("encodes");
        prop_assert_eq!(
            decode_generation(&encoded, original.workspace).expect("decodes"),
            original
        );
    }

    #[test]
    fn a_shard_is_a_pure_function_of_the_generation(byte in any::<u8>()) {
        let first = keys::shard_of(generation(byte));
        prop_assert_eq!(first, keys::shard_of(generation(byte)));
        prop_assert!(u64::from(first) < keys::DUE_SHARDS);
    }

    #[test]
    fn the_due_sort_key_orders_by_evaluation_time(
        first in 0_i64..4_000_000_000_000,
        second in 0_i64..4_000_000_000_000,
    ) {
        let earlier = keys::due_sort(
            Timestamp::from_unix_millis(first).expect("in range"),
            generation(1),
        );
        let later = keys::due_sort(
            Timestamp::from_unix_millis(second).expect("in range"),
            generation(1),
        );
        prop_assert_eq!(earlier < later, first < second);
    }
}

#[test]
fn every_state_that_can_still_change_stays_in_the_due_index() {
    for state in GenerationState::ALL {
        let encoded = encode_generation(&head(state)).expect("encodes");
        assert_eq!(
            encoded.contains_key(keys::DUE_PK),
            keys::is_evaluable(state),
            "`{}` is on the wrong side of the index",
            keys::state_str(state)
        );
    }
}

#[test]
fn an_intent_and_a_receipt_round_trip_and_share_an_identity() {
    let intent = intent();
    let encoded = encode_intent(&intent).expect("encodes");
    assert_eq!(
        decode_intent(&encoded, intent.workspace).expect("decodes"),
        intent
    );

    let receipt = receipt();
    let encoded = encode_receipt(&receipt).expect("encodes");
    assert_eq!(
        decode_receipt(&encoded, receipt.workspace).expect("decodes"),
        receipt
    );
    assert_eq!(
        intent.intent_id, receipt.intent_id,
        "an intent and its receipt are matched by identity, never by ordering"
    );
}

#[test]
fn a_probe_carries_a_ttl_and_a_receipt_does_not() {
    assert!(encode_probe(&probe()).contains_key("expiresAtEpochSeconds"));
    assert!(
        !encode_receipt(&receipt())
            .expect("encodes")
            .contains_key("expiresAtEpochSeconds")
    );
}

#[test]
fn a_row_from_another_tenant_is_refused_after_read() {
    let encoded = encode_generation(&head(GenerationState::Running)).expect("encodes");
    assert!(decode_generation(&encoded, support::other_workspace()).is_err());
    let _ = workspace();
}
