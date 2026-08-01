//! Key, ordering and codec properties for `regional-content`.

mod support;

use aex_content_dynamodb::codec::{
    decode_descriptor, decode_grant, encode_descriptor, encode_grant,
};
use aex_content_dynamodb::keys;
use aex_content_dynamodb::wire_pending::{Blake3Digest, PinOwner, body_hex};
use aex_wire::ids::ContentHash;
use aex_wire::types::Timestamp;
use proptest::prelude::*;

use support::{descriptor, digest, grant, now, object_descriptor, workspace};

proptest! {
    #[test]
    fn a_scan_bucket_is_stable_and_always_inside_the_declared_range(bytes in any::<[u8; 32]>()) {
        let hex = hex::encode(bytes);
        let first = keys::bucket_of(&hex).expect("a bucket");
        prop_assert_eq!(first, keys::bucket_of(&hex).expect("a bucket"));
        prop_assert!(first < keys::GC_BUCKETS);
    }

    #[test]
    fn a_scan_sort_key_orders_by_time_and_then_by_digest(
        first_millis in 0_i64..4_000_000_000_000,
        second_millis in 0_i64..4_000_000_000_000,
        first_bytes in any::<[u8; 32]>(),
        second_bytes in any::<[u8; 32]>(),
    ) {
        let first_at = Timestamp::from_unix_millis(first_millis).expect("in range");
        let second_at = Timestamp::from_unix_millis(second_millis).expect("in range");
        let first_hex = hex::encode(first_bytes);
        let second_hex = hex::encode(second_bytes);
        let first = keys::gc_scan_sort(first_at, &first_hex).expect("a key");
        let second = keys::gc_scan_sort(second_at, &second_hex).expect("a key");
        if first_millis < second_millis {
            prop_assert!(first < second, "{first} !< {second}");
        } else if first_millis == second_millis {
            prop_assert_eq!(first < second, first_hex < second_hex);
        }
    }

    #[test]
    fn two_workspaces_never_share_a_content_partition(bytes in any::<[u8; 32]>()) {
        let digest = ContentHash::from_bytes(bytes);
        let mine = keys::content_partition(workspace(), &digest);
        let theirs = keys::content_partition(support::other_workspace(), &digest);
        prop_assert_ne!(mine, theirs);
    }

    #[test]
    fn a_pin_identity_that_could_forge_a_key_is_always_refused(
        identity in "\\PC*",
    ) {
        let owner = PinOwner::Export(identity.clone());
        let outcome = keys::root_pin(workspace(), Blake3Digest::from_bytes([1; 32]), &owner);
        let admissible = !identity.is_empty()
            && identity.len() <= 256
            && !identity.chars().any(|character| {
                character == '#'
                    || character.is_control()
                    || character == '\u{ffff}'
                    || character == '\u{fffe}'
            });
        prop_assert_eq!(outcome.is_ok(), admissible, "identity `{}`", identity);
    }

    #[test]
    fn a_descriptor_round_trips_over_arbitrary_sizes(size in 0_u64..1_000_000_000_000) {
        let mut original = descriptor();
        original.size_bytes = size;
        let encoded = encode_descriptor(&original).expect("encodes");
        let decoded = decode_descriptor(&encoded, original.workspace).expect("decodes");
        prop_assert_eq!(decoded, original);
    }

    #[test]
    fn a_grant_round_trips_over_arbitrary_ranges(
        start in 0_u64..1_000_000,
        length in 1_u64..1_000_000,
    ) {
        let mut original = grant();
        original.range_start = start;
        original.range_end_exclusive = start + length;
        original.authorized_bytes = length;
        let encoded = encode_grant(&original).expect("encodes");
        prop_assert_eq!(decode_grant(&encoded).expect("decodes"), original);
    }
}

#[test]
fn an_object_backed_descriptor_keeps_both_checksums_apart() {
    let original = object_descriptor();
    let encoded = encode_descriptor(&original).expect("encodes");
    let decoded = decode_descriptor(&encoded, original.workspace).expect("decodes");
    let object = decoded.object.expect("an object location");
    assert_ne!(
        object.checksum_sha256, object.checksum_crc64_nvme,
        "the composite SHA-256 and the full-object CRC64NVME are different facts \
         and must never be stored in one attribute"
    );
}

#[test]
fn the_key_form_of_a_digest_is_the_bare_hex_and_the_stored_form_is_the_wire_spelling() {
    let body = digest(0xab);
    let partition = keys::content_partition(workspace(), &body);
    assert!(partition.ends_with(&body_hex(&body)));
    assert!(
        !partition.contains("sha256:"),
        "the family is already fixed by the key template"
    );
    let encoded = encode_descriptor(&descriptor()).expect("encodes");
    assert_eq!(
        encoded["digestSha256"].as_s().expect("a digest"),
        &body.to_wire()
    );
}

#[test]
fn a_grant_ttl_never_precedes_the_expiry_it_belongs_to() {
    let grant = grant();
    let ttl = aex_content_dynamodb::codec::ttl_epoch_seconds(grant.expires_at);
    assert!(ttl > grant.expires_at.unix_millis().div_euclid(1_000));
    assert_eq!(
        ttl - grant.expires_at.unix_millis().div_euclid(1_000),
        keys::GRANT_TTL_GRACE_SECONDS
    );
}

#[test]
fn a_hold_always_pushes_a_candidate_a_full_day_past_staging() {
    let candidate = aex_content_dynamodb::codec::GcCandidate {
        workspace: workspace(),
        digest: digest(1),
        epoch: 1,
        staged_at: now(),
        object_key: None,
        object_etag: None,
        size_bytes: 1,
        attempt_count: 0,
    };
    let not_before = candidate.not_before().expect("in range");
    assert_eq!(
        not_before.unix_millis() - now().unix_millis(),
        keys::CANDIDATE_HOLD_SECONDS * 1_000
    );
}
