//! Tenancy and leakage cases for `regional-secret-custody`.

mod support;

use aex_secret_custody_dynamodb::codec::{
    decode_generation, decode_secret, encode_generation, encode_manifest,
    encode_provider_credential, encode_secret,
};
use aex_secret_custody_dynamodb::keys;
use aex_session_dynamodb::attr::CodecError;

use support::{
    REDACTED_SECRETS, generation, manifest, metadata, other_workspace, provider_credential,
    session, workspace,
};

#[test]
fn a_row_from_another_tenant_is_refused_after_read() {
    let encoded = encode_secret(&metadata()).expect("encodes");
    let error = decode_secret(&encoded, other_workspace()).expect_err("another tenant");
    assert!(matches!(error, CodecError::WrongTenant { .. }), "{error}");

    let sealed = encode_generation(&generation()).expect("encodes");
    let error = decode_generation(&sealed, other_workspace()).expect_err("another tenant");
    assert!(matches!(error, CodecError::WrongTenant { .. }), "{error}");
}

#[test]
fn the_list_path_and_the_ciphertext_path_are_different_partitions() {
    let listed = keys::secret_partition(workspace());
    let sealed = keys::generation_partition(workspace(), "openai-key").expect("a partition");
    assert_ne!(listed, sealed);
    assert!(listed.starts_with("SEC#"));
    assert!(sealed.starts_with("SECGEN#"));
}

#[test]
fn a_metadata_row_has_no_attribute_that_could_hold_a_secret() {
    let encoded = encode_secret(&metadata()).expect("encodes");
    for name in encoded.keys() {
        let lowered = name.to_lowercase();
        assert!(
            !lowered.contains("cipher")
                && !lowered.contains("wrapped")
                && !lowered.contains("nonce"),
            "the metadata row carried `{name}`"
        );
    }
}

#[test]
fn the_redaction_manifest_carries_digests_and_nothing_a_collector_could_reverse() {
    let encoded = encode_manifest(&manifest());
    for name in encoded.keys() {
        let lowered = name.to_lowercase();
        assert!(
            !lowered.contains("cipher")
                && !lowered.contains("value")
                && !lowered.contains("plaintext"),
            "the manifest carried `{name}`"
        );
    }
    let entries = encoded["entries"].as_l().expect("a list");
    assert_eq!(entries.len(), REDACTED_SECRETS.len());
    for (entry, secret) in entries.iter().zip(REDACTED_SECRETS) {
        let map = entry.as_m().expect("a map");
        assert_eq!(
            map["hmac"].as_b().expect("a blob").as_ref().len(),
            32,
            "every entry is a fixed-width digest, so none of them can be a value"
        );
        assert_ne!(
            map["hmac"].as_b().expect("a blob").as_ref(),
            secret.as_bytes(),
            "the digest is not the secret"
        );
        // The length is published on purpose — the collector cannot slide a
        // window without it — and it is the only thing the row says about the
        // value. A length is not a value, and nothing here inverts a digest.
        assert_eq!(
            map["len"].as_n().expect("a number"),
            &secret.len().to_string()
        );
    }
}

#[test]
fn a_provider_credential_binding_references_a_secret_and_holds_no_key_material() {
    let encoded = encode_provider_credential(&provider_credential()).expect("encodes");
    assert!(encoded.contains_key("secretName"));
    assert!(encoded.contains_key("sourceGeneration"));
    for name in encoded.keys() {
        let lowered = name.to_lowercase();
        assert!(
            !lowered.contains("cipher") && !lowered.contains("wrapped"),
            "the credential binding carried `{name}`"
        );
    }
}

#[test]
fn the_manifest_sits_in_a_partition_that_holds_no_custody_row() {
    let manifest = keys::redaction_manifest(session());
    let head = keys::custody_head(session());
    assert_ne!(manifest.pk, head.pk);
    assert!(manifest.pk.starts_with("REDACT#"));
}
