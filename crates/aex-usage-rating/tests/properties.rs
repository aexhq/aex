//! Public exact-accumulation identity evidence.

use aex_usage_rating::accumulate;
use aex_usage_rating::storage_close::{StorageCloseKind, shadow_storage_close};
use num_traits::Zero as _;

#[test]
fn empty_exact_accumulation_is_the_additive_identity() {
    assert!(accumulate([]).is_zero());
}

#[test]
fn storage_close_rounding_is_directional_and_strictly_bounded() {
    let bytes = 17_u128;
    let elapsed_ms = 61_001_u128;
    let interior = shadow_storage_close(bytes, elapsed_ms, StorageCloseKind::Interior)
        .expect("interior receipt");
    let hard_delete = shadow_storage_close(bytes, elapsed_ms, StorageCloseKind::HardDelete)
        .expect("hard-delete receipt");
    assert_eq!(interior.billed_byte_minutes, bytes);
    assert_eq!(hard_delete.billed_byte_minutes, bytes * 2);
    assert!(interior.rounding_error_byte_milliseconds < interior.error_bound_byte_milliseconds);
    assert!(
        hard_delete.rounding_error_byte_milliseconds < hard_delete.error_bound_byte_milliseconds
    );
}
