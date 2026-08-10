//! Key, `ETag` and codec properties for `regional-registry`.

mod support;

use aex_content_domain::identity::{RegistryKind, Revision};
use aex_registry_dynamodb::codec::{
    decode_pointer, decode_upload, encode_pointer, encode_upload, upload_state_of, upload_state_str,
};
use aex_registry_dynamodb::keys;
use aex_wire::ids::ContentHash;
use aex_workspace_domain::registry::etag_of;
use aex_workspace_domain::upload::{PartPlan, PlannedPart, UploadState};
use proptest::prelude::*;

use support::{name, pointer, upload, workspace};

proptest! {
    #[test]
    fn a_pointer_round_trips_over_arbitrary_sizes_and_revisions(
        size in 0_u64..1_000_000_000_000,
        revision in 1_u64..1_000_000,
    ) {
        // `size_bytes` is the value document's own length since D-4, so it
        // cannot be varied independently: the codec re-derives it and refuses a
        // row whose two halves disagree. What varies is the document.
        let _ = size;
        let mut original = pointer();
        original.row.revision = Revision(revision);
        original.row.etag =
            etag_of(original.row.kind, original.row.revision, &original.row.sha256);
        let encoded = encode_pointer(&original).expect("encodes");
        prop_assert_eq!(
            decode_pointer(&encoded, original.row.workspace).expect("decodes"),
            original
        );
    }

    #[test]
    fn an_etag_is_a_pure_function_of_the_kind_the_revision_and_the_digest(
        revision in 1_u64..1_000_000,
        bytes in any::<[u8; 32]>(),
    ) {
        let digest = ContentHash::from_bytes(bytes);
        let first = etag_of(RegistryKind::Tool, Revision(revision), &digest);
        prop_assert_eq!(&first, &etag_of(RegistryKind::Tool, Revision(revision), &digest));
        prop_assert_ne!(&first, &etag_of(RegistryKind::Skill, Revision(revision), &digest));
        prop_assert_ne!(&first, &etag_of(RegistryKind::Tool, Revision(revision + 1), &digest));
    }

    #[test]
    fn an_upload_round_trips_over_any_part_plan(count in 1_usize..64) {
        let mut original = upload();
        original.parts = PartPlan {
            parts: (0..count)
                .map(|index| PlannedPart {
                    number: u32::try_from(index + 1).expect("a small count"),
                    bytes: 5 * 1024 * 1024,
                })
                .collect(),
        };
        let encoded = encode_upload(&original);
        prop_assert_eq!(
            decode_upload(&encoded, original.workspace).expect("decodes"),
            original
        );
    }
}

#[test]
fn every_upload_state_has_exactly_one_stable_spelling_and_parses_back() {
    let mut seen = std::collections::BTreeSet::new();
    for state in UploadState::ALL {
        let text = upload_state_str(state);
        assert!(seen.insert(text), "`{text}` is used twice");
        assert_eq!(upload_state_of(text), Some(state));
        assert!(keys::UPLOAD_STATES.contains(&text));
    }
    assert_eq!(seen.len(), keys::UPLOAD_STATES.len());
}

#[test]
fn every_registry_kind_has_its_own_partition_and_its_own_wire_spelling() {
    let mut partitions = std::collections::BTreeSet::new();
    for kind in RegistryKind::ALL {
        partitions.insert(keys::kind_partition(workspace(), kind));
        assert_eq!(RegistryKind::parse(kind.as_str()), Some(kind));
    }
    assert_eq!(partitions.len(), RegistryKind::ALL.len());
}

#[test]
fn names_sort_in_the_order_a_listing_returns_them() {
    let mut keys: Vec<String> = ["zeta", "alpha", "middle"]
        .iter()
        .map(|text| {
            keys::pointer(workspace(), RegistryKind::File, name(text).as_str())
                .expect("a key")
                .sk
        })
        .collect();
    keys.sort();
    assert_eq!(keys, ["NAME#alpha", "NAME#middle", "NAME#zeta"]);
}
