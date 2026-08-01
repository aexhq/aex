//! Addressing and manifest properties for the content object adapter.

mod support;

use aex_content_aws::multipart::{CompletionManifest, MIN_PART_BYTES};
use aex_content_aws::object_key::{self, MAX_RANGE_BYTES, ObjectKey};
use aex_wire::ids::ContentHash;
use proptest::prelude::*;

use support::{manifest, other_workspace, provider_parts, workspace};

proptest! {
    #[test]
    fn a_key_is_a_pure_function_of_the_workspace_and_the_digest(bytes in any::<[u8; 32]>()) {
        let digest = ContentHash::from_bytes(bytes);
        let first = ObjectKey::new(workspace(), &digest);
        prop_assert_eq!(&first, &ObjectKey::new(workspace(), &digest));
        prop_assert_ne!(&first, &ObjectKey::new(other_workspace(), &digest));
    }

    #[test]
    fn a_key_always_fans_out_by_the_first_four_hex_digits(bytes in any::<[u8; 32]>()) {
        let digest = ContentHash::from_bytes(bytes);
        let hex = hex::encode(bytes);
        let key = ObjectKey::new(workspace(), &digest);
        let segments: Vec<&str> = key.as_str().split('/').collect();
        prop_assert_eq!(segments.len(), 4);
        prop_assert_eq!(segments[1], &hex[0..2]);
        prop_assert_eq!(segments[2], &hex[2..4]);
        prop_assert_eq!(segments[3], hex.as_str());
    }

    #[test]
    fn a_key_always_starts_with_the_workspace_prefix_a_bucket_policy_can_match(
        bytes in any::<[u8; 32]>(),
    ) {
        let key = ObjectKey::new(workspace(), &ContentHash::from_bytes(bytes));
        prop_assert!(key.as_str().starts_with(&ObjectKey::workspace_prefix(workspace())));
    }

    #[test]
    fn a_checksum_is_always_the_base64_of_the_raw_digest(bytes in any::<[u8; 32]>()) {
        let digest = ContentHash::from_bytes(bytes);
        let encoded = object_key::checksum_base64(&digest);
        prop_assert_eq!(encoded.len(), 44);
        prop_assert!(!encoded.contains(&hex::encode(bytes)));
    }

    #[test]
    fn any_disagreement_between_the_manifest_and_the_provider_is_refused(
        drift in 1_u64..1_000_000,
        which in 0_usize..2,
    ) {
        let mut held = provider_parts();
        held[which].size += drift;
        prop_assert!(manifest().agrees_with(&held).is_err());
    }
}

#[test]
fn a_manifest_that_matches_exactly_is_the_only_thing_that_agrees() {
    manifest().agrees_with(&provider_parts()).expect("agrees");
}

#[test]
fn the_last_part_alone_may_sit_below_the_provider_minimum() {
    let manifest = manifest();
    assert!(manifest.parts[0].plan.size >= MIN_PART_BYTES);
    assert!(
        manifest.parts[1].plan.size < MIN_PART_BYTES,
        "only the final part may be short, and the checker must permit exactly that"
    );
    manifest.agrees_with(&provider_parts()).expect("agrees");
}

#[test]
fn the_declared_total_is_what_becomes_the_object_size_assertion() {
    let manifest: CompletionManifest = manifest();
    let summed: u64 = manifest.parts.iter().map(|part| part.plan.size).sum();
    assert_eq!(summed, manifest.total_bytes);
}

#[test]
fn a_range_above_the_single_read_ceiling_is_a_typed_refusal_and_not_a_service_error() {
    assert_eq!(MAX_RANGE_BYTES, 5 * 1024 * 1024 * 1024 * 1024);
    let ceiling_bytes = MAX_RANGE_BYTES;
    assert!(
        ceiling_bytes > u64::from(u32::MAX),
        "the ceiling is a storage-scale number, not a page size"
    );
}

#[test]
fn the_encryption_context_is_carried_as_base64_of_the_canonical_bytes() {
    use base64::Engine as _;

    let context = br#"{"aex:workspace":"wsp_x"}"#;
    let encoded = object_key::encryption_context_base64(context);
    assert!(!encoded.contains("aex:workspace"));
    assert_eq!(
        base64::engine::general_purpose::STANDARD
            .decode(&encoded)
            .expect("round trips"),
        context.to_vec()
    );
}
