//! Tenancy, leakage and least-visibility cases for `regional-content`.

mod support;

use aex_content_dynamodb::codec::{
    self, decode_descriptor, decode_inline_body, encode_descriptor, encode_gc_candidate,
    encode_grant, encode_inline_body,
};
use aex_content_dynamodb::keys;
use aex_content_dynamodb::wire_pending::InlineBody;
use aex_session_dynamodb::attr::CodecError;

use support::{DEFINITION, descriptor, digest, grant, now, other_workspace, sealed, workspace};

#[test]
fn a_row_belonging_to_another_tenant_is_refused_after_read() {
    let encoded = encode_descriptor(&descriptor()).expect("encodes");
    let error = decode_descriptor(&encoded, other_workspace()).expect_err("another tenant");
    assert!(matches!(error, CodecError::WrongTenant { .. }), "{error}");

    let body = encode_inline_body(workspace(), &digest(1), &sealed(32)).expect("encodes");
    let error = decode_inline_body(&body, other_workspace()).expect_err("another tenant");
    assert!(matches!(error, CodecError::WrongTenant { .. }), "{error}");
}

#[test]
fn identical_bytes_in_two_workspaces_are_two_physically_separate_bodies() {
    let shared = digest(0x42);
    assert_ne!(
        keys::descriptor(workspace(), &shared),
        keys::descriptor(other_workspace(), &shared),
        "physical reuse across tenants is an equality side channel and a shared \
         deletion fate; the workspace prefix is what removes both"
    );
}

#[test]
fn no_indexed_attribute_can_ever_carry_a_ciphertext() {
    let projection: Vec<&str> = keys::GC_PROJECTION.to_vec();
    for forbidden in ["ciphertext", "encContextDigest", "mediaType"] {
        assert!(
            !projection.contains(&forbidden),
            "`{forbidden}` must never reach a scan result"
        );
    }
    let definition: serde_json::Value =
        serde_json::from_str(DEFINITION).expect("the generation definition is JSON");
    let attributes = definition["globalSecondaryIndexes"][0]["projection"]["attributes"]
        .as_array()
        .expect("an attribute list");
    assert!(
        attributes
            .iter()
            .all(|value| value.as_str() != Some("ciphertext")),
        "the index definition itself must not project a body"
    );
}

#[test]
fn a_download_grant_row_has_nowhere_to_put_a_body() {
    let encoded = encode_grant(&grant()).expect("encodes");
    let names: Vec<&String> = encoded.keys().collect();
    for name in &names {
        assert!(
            !name.to_lowercase().contains("cipher") && !name.to_lowercase().contains("body"),
            "a grant carried `{name}`"
        );
    }
    assert!(encoded.contains_key("contentDigest"), "{names:?}");
}

#[test]
fn the_body_row_and_the_descriptor_row_are_separate_so_a_scan_never_reads_a_body() {
    let body = digest(7);
    assert_ne!(
        keys::descriptor(workspace(), &body).sk,
        keys::inline_body(workspace(), &body).sk
    );
    let encoded = encode_inline_body(workspace(), &body, &sealed(64)).expect("encodes");
    assert!(!encoded.contains_key(keys::GC_PK));
}

#[test]
fn a_candidate_carries_the_index_but_never_a_readable_body() {
    let candidate = codec::GcCandidate {
        workspace: workspace(),
        digest: digest(3),
        epoch: 1,
        staged_at: now(),
        object_key: Some("ws/aa/bb/cc".to_owned()),
        object_etag: Some("\"etag\"".to_owned()),
        size_bytes: 128,
        attempt_count: 0,
    };
    let encoded = encode_gc_candidate(&candidate).expect("encodes");
    assert!(encoded.contains_key(keys::GC_PK));
    assert!(!encoded.contains_key("ciphertext"));
}

#[test]
fn an_inline_content_body_never_prints_its_ciphertext() {
    let sealed = InlineBody {
        ciphertext: b"super secret plaintext-shaped bytes".to_vec(),
        enc_context_digest: "e".repeat(64),
    };
    let printed = format!("{sealed:?}");
    assert!(!printed.contains("super secret"), "{printed}");
    assert!(printed.contains("35 bytes"), "{printed}");
}
