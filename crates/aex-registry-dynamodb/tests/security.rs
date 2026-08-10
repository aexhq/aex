//! Tenancy and least-visibility cases for `regional-registry`.

mod support;

use aex_content_domain::identity::RegistryKind;
use aex_registry_dynamodb::codec::{decode_pointer, decode_upload, encode_pointer, encode_upload};
use aex_registry_dynamodb::keys;
use aex_session_dynamodb::attr::CodecError;

use support::{other_workspace, pointer, upload, workspace};

#[test]
fn a_pointer_from_another_tenant_is_refused_after_read() {
    let encoded = encode_pointer(&pointer()).expect("encodes");
    let error = decode_pointer(&encoded, other_workspace()).expect_err("another tenant");
    assert!(matches!(error, CodecError::WrongTenant { .. }), "{error}");
}

#[test]
fn an_upload_from_another_tenant_is_refused_after_read() {
    let encoded = encode_upload(&upload()).head;
    let error = decode_upload(&encoded, other_workspace()).expect_err("another tenant");
    assert!(matches!(error, CodecError::WrongTenant { .. }), "{error}");
}

#[test]
fn two_workspaces_can_never_share_a_registry_partition() {
    assert_ne!(
        keys::kind_partition(workspace(), RegistryKind::Tool),
        keys::kind_partition(other_workspace(), RegistryKind::Tool)
    );
}

#[test]
fn a_pointer_row_carries_a_digest_and_never_a_body() {
    let encoded = encode_pointer(&pointer()).expect("encodes");
    for name in encoded.keys() {
        let lowered = name.to_lowercase();
        assert!(
            !lowered.contains("cipher") && !lowered.contains("body") && !lowered.contains("inline"),
            "a registry pointer carried `{name}`"
        );
    }
    assert!(encoded.contains_key("contentDigest"));
}

#[test]
fn an_upload_row_records_what_the_caller_declared_and_never_the_bytes() {
    let encoded = encode_upload(&upload()).head;
    assert!(encoded.contains_key("declaredSha256"));
    assert!(encoded.contains_key("declaredSizeBytes"));
    for name in encoded.keys() {
        assert!(
            !name.to_lowercase().contains("cipher"),
            "an upload row carried `{name}`"
        );
    }
}

#[test]
fn a_list_query_cannot_reach_an_upload_or_a_receipt() {
    // Uploads and receipts live in their own partitions, so the one query a
    // listing issues cannot return either however the page is sized.
    let listing = keys::kind_partition(workspace(), RegistryKind::File);
    assert!(listing.starts_with("REG#"));
    assert!(keys::upload(support::upload_id()).pk.starts_with("UPLOAD#"));
    assert!(
        keys::receipt(workspace(), "registry.set:file", &"0".repeat(64))
            .expect("a key")
            .pk
            .starts_with("IDEM#")
    );
}
