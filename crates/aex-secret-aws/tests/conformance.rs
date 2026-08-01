//! Request conformance for the secret cryptography adapter.

mod support;

use std::collections::BTreeMap;

use aex_secret_aws::context::{NAME_DIGEST_KEY, NAME_KEY};
use aex_secret_aws::crypto::{EnvelopeCrypto, IMPLEMENTATION, SecretCrypto, key_version};
use aex_secret_aws::keystore::{BranchKeyProvider, KmsBranchKeys};
use aws_sdk_kms::Client;
use aws_sdk_kms::config::{BehaviorVersion, Credentials, Region};
use aws_smithy_http_client::test_util::{CaptureRequestReceiver, capture_request};

use support::{FakeKeys, Pinned, WRAPPED, context, now, plaintext};

fn capturing_kms() -> (Client, CaptureRequestReceiver) {
    let (http_client, receiver) = capture_request(None);
    let config = aws_sdk_kms::Config::builder()
        .behavior_version(BehaviorVersion::latest())
        .region(Region::new("eu-west-1"))
        .credentials_provider(Credentials::new(
            "AKIDTESTTESTTESTTEST",
            "test-secret",
            None,
            None,
            "aex-tests",
        ))
        .http_client(http_client)
        .build();
    (Client::from_conf(config), receiver)
}

#[tokio::test]
async fn the_serialized_decrypt_carries_the_context_and_never_the_secret_name() {
    let (client, receiver) = capturing_kms();
    let keys = KmsBranchKeys::new(client, "arn:aws:kms:eu-west-1:000000000000:key/secret");
    let bound = context("acme-production-stripe", 1);

    let _ignored = keys
        .material(
            &bound.workspace.to_string(),
            key_version(WRAPPED),
            WRAPPED,
            &aex_secret_aws::context::kms_pairs(&bound),
        )
        .await;

    let request = receiver.expect_request();
    let body = request.body().bytes().expect("the KMS body is in memory");
    let parsed: serde_json::Value = serde_json::from_slice(body).expect("the KMS body is JSON");
    let context_pairs = parsed["EncryptionContext"]
        .as_object()
        .expect("an encryption context");
    assert!(
        context_pairs.contains_key(NAME_DIGEST_KEY),
        "{context_pairs:?}"
    );
    assert!(
        !context_pairs.contains_key(NAME_KEY),
        "the context is recorded in CloudTrail: {context_pairs:?}"
    );
    assert!(
        !String::from_utf8_lossy(body).contains("acme-production-stripe"),
        "the customer-chosen name reached the wire"
    );
    assert_eq!(
        parsed["KeyId"].as_str(),
        Some("arn:aws:kms:eu-west-1:000000000000:key/secret")
    );
}

#[tokio::test]
async fn the_branch_key_is_selected_by_the_context_and_never_by_the_caller() {
    let keys = FakeKeys::new();
    let crypto =
        EnvelopeCrypto::new(Box::new(keys.clone()), "role-a").with_entropy(Box::new(Pinned(3)));
    let bound = context("openai-key", 1);

    crypto
        .seal(&bound, WRAPPED, &plaintext("hunter2"), now())
        .await
        .expect("seals");

    let seen: Vec<BTreeMap<String, String>> = keys.contexts();
    assert_eq!(seen.len(), 1);
    assert_eq!(
        seen[0]["aex:workspace"],
        bound.workspace.to_string(),
        "the branch key id is a pure function of the context"
    );
}

#[test]
fn the_selected_implementation_is_named_so_a_startup_log_can_state_it() {
    assert_eq!(
        IMPLEMENTATION, "envelope-aead-aws-lc-rs",
        "OD-33 requires the composition to select one arm at startup and log \
         which; a runtime fallback is not allowed"
    );
}

#[test]
fn a_key_version_is_derived_from_the_wrapped_bytes_rather_than_declared_beside_them() {
    assert_eq!(key_version(WRAPPED), key_version(WRAPPED));
    assert_ne!(key_version(WRAPPED), key_version(b"another wrapped key"));
}
