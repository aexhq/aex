//! Record properties for `regional-secret-keystore`.

mod support;

use aex_secret_keystore_dynamodb::branch_key::{self, BranchKeyId, RecordKind, decode};
use aex_secret_keystore_dynamodb::store::ActiveBranchKey;
use aex_session_dynamodb::attr::s;
use proptest::prelude::*;

use support::{KMS_ARN, active_record, branch_key_id, workspace};

proptest! {
    #[test]
    fn every_version_identity_round_trips_through_its_sort_key(uuid in "[0-9a-f]{8}") {
        let kind = RecordKind::Version(uuid.clone());
        prop_assert_eq!(RecordKind::parse(&kind.as_sort_key()), Some(kind));
    }

    #[test]
    fn a_branch_key_identity_is_the_workspace_identity(byte in any::<u8>()) {
        let identity = branch_key_id(byte);
        let expected = workspace(byte).to_string();
        prop_assert_eq!(identity.as_str(), expected.as_str());
    }

    #[test]
    fn a_record_round_trips_over_arbitrary_hierarchy_versions(version in 1_u64..1_000) {
        let mut item = active_record(1);
        item.insert(
            branch_key::HIERARCHY_VERSION.to_owned(),
            aex_session_dynamodb::attr::n(version),
        );
        let decoded = decode(&item).expect("decodes");
        prop_assert_eq!(decoded.hierarchy_version, version);
    }
}

#[test]
fn an_active_record_projects_into_exactly_what_an_administrator_needs() {
    let record = decode(&active_record(1)).expect("decodes");
    let active: ActiveBranchKey = record.into();
    assert_eq!(active.branch_key_id, BranchKeyId::of(workspace(1)));
    assert_eq!(active.kms_arn, KMS_ARN);
    assert!(active.version.is_some());
    assert_eq!(active.create_time, "2026-08-01T12:34:56.789Z");
}

/// D-4. The projection used to drop `enc`, which is why nothing on a request
/// path could seal: the one field a first seal needs was the one field the
/// administrator's view discarded.
#[test]
fn the_projection_carries_the_wrapped_material_a_first_seal_needs() {
    let item = active_record(1);
    let stored = item[branch_key::ENC].as_b().expect("wrapped bytes").clone();
    let active: ActiveBranchKey = decode(&item).expect("decodes").into();
    assert_eq!(active.wrapped_material, stored.into_inner());
    assert!(!active.wrapped_material.is_empty());
}

/// Wrapped material is not openable outside `KMS`, but a wrapped key in a log
/// is still key material in a log.
#[test]
fn the_debug_rendering_names_the_length_and_never_the_material() {
    let active: ActiveBranchKey = decode(&active_record(1)).expect("decodes").into();
    let rendered = format!("{active:?}");
    assert!(rendered.contains("<wrapped, 64 bytes>"), "{rendered}");
    assert!(
        !rendered.contains("170, 170"),
        "the material must never reach a rendering: {rendered}"
    );
}

#[test]
fn custom_context_pairs_are_read_back_without_their_storage_prefix() {
    let mut item = active_record(1);
    item.insert(
        format!("{}aex:plane", branch_key::CUSTOM_CONTEXT_PREFIX),
        s("prd"),
    );
    let decoded = decode(&item).expect("decodes");
    assert_eq!(decoded.custom_context["aex:plane"], "prd");
    assert!(
        !decoded
            .custom_context
            .contains_key("aws-crypto-ec:aex:plane")
    );
}
