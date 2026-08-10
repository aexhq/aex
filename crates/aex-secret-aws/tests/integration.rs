//! Engine-backed cases for the secret cryptography adapter, against `moto`'s
//! KMS.
//!
//! What this proves: that the adapter's `Decrypt` really round-trips a wrapped
//! branch key against a service, that the encryption context is carried on the
//! wire and enforced by that service, and that the whole seal → reveal path
//! works end to end with real key material rather than a fake. The enforcement
//! is not bookkeeping: `moto` encrypts under AES-256-GCM with the serialised
//! encryption context as additional authenticated data, so a foreign context
//! fails tag verification and is refused with `InvalidCiphertextException`.
//!
//! What it does not prove is stated rather than assumed: `moto` implements no
//! KMS key policy, so the `kms:EncryptionContext:aex:workspace` condition OD-18
//! requires — the thing that makes the binding enforceable at the key rather
//! than advisory — is a live concern (plan 05 section 8.3 item 11).

mod support;

use aex_secret_aws::crypto::{EnvelopeCrypto, SecretCrypto, SecretCryptoError, key_version};
use aex_secret_aws::keystore::{BranchKeyProvider, KeyMaterialError, KmsBranchKeys};
use aex_test_harness::MotoContainer;
use aws_sdk_kms::Client;
use aws_sdk_kms::config::{BehaviorVersion, Credentials, Region};
use aws_sdk_kms::types::DataKeySpec;

use support::{WRAPPED, context, now, plaintext, session_context};

fn client(engine: &MotoContainer) -> Client {
    let config = aws_sdk_kms::Config::builder()
        .behavior_version(BehaviorVersion::latest())
        .region(Region::new(engine.region()))
        .endpoint_url(engine.endpoint_url())
        .credentials_provider(Credentials::new(
            engine.access_key_id(),
            engine.secret_access_key(),
            None,
            None,
            "aex-integration",
        ))
        .build();
    Client::from_conf(config)
}

/// Creates a root key and wraps one branch key under it, exactly as
/// `aex_secret_keystore_dynamodb::provision` does in production.
///
/// The wrap context is the **branch key's** — plane, region, workspace — and not
/// the secret's. One branch key serves every name and every generation in a
/// workspace, so the context that wraps it has to be reproducible from the
/// workspace alone; the secret's full identity binds the value one layer down,
/// as AEAD additional data, which is what the cases below exercise.
async fn branch_key(
    client: &Client,
    context: &aex_secret_domain::context::EncryptionContext,
) -> (String, Vec<u8>) {
    let key = client
        .create_key()
        .send()
        .await
        .expect("the root key is created")
        .key_metadata
        .expect("key metadata");
    let key = key.arn.expect("the root key ARN");

    let mut request = client
        .generate_data_key_without_plaintext()
        .key_id(&key)
        .key_spec(DataKeySpec::Aes256);
    for (name, value) in aex_secret_aws::context::branch_key_pairs(context) {
        request = request.encryption_context(name, value);
    }
    let wrapped = request
        .send()
        .await
        .expect("the branch key is wrapped")
        .ciphertext_blob
        .expect("a wrapped key")
        .into_inner();
    (key, wrapped)
}

#[tokio::test]
async fn a_value_seals_and_reveals_against_a_real_key_service() {
    let engine = MotoContainer::start().await.expect("moto starts");
    let client = client(&engine);
    let bound = context("openai-key", 1);
    let (key, wrapped) = branch_key(&client, &bound).await;

    let crypto = EnvelopeCrypto::new(
        Box::new(KmsBranchKeys::new(client, key)),
        "aex-integration-role",
    );

    let sealed = crypto
        .seal(&bound, &wrapped, &plaintext("hunter2"), now())
        .await
        .expect("seals");
    assert!(
        !sealed.frame.windows(7).any(|window| window == b"hunter2"),
        "the frame leaked its plaintext"
    );

    let revealed = crypto
        .reveal(&sealed, &bound, now())
        .await
        .expect("reveals");
    assert_eq!(revealed.expose_for_encryption(), b"hunter2");
    assert_eq!(
        crypto.cache().len(),
        1,
        "the second operation came from the cache rather than from KMS"
    );
}

/// Tenant isolation at the **key** layer: `KMS` itself refuses to open one
/// workspace's branch key while another workspace is claimed.
#[tokio::test]
async fn the_service_refuses_a_wrapped_key_presented_under_another_context() {
    let engine = MotoContainer::start().await.expect("moto starts");
    let client = client(&engine);
    let bound = context("openai-key", 1);
    let (key, wrapped) = branch_key(&client, &bound).await;

    let keys = KmsBranchKeys::new(client, key);
    let mut foreign = aex_secret_aws::context::branch_key_pairs(&bound);
    foreign.insert(
        aex_secret_aws::context::WORKSPACE_KEY.to_owned(),
        "wsp_0000000000000000000000000".to_owned(),
    );
    let error = keys
        .material(
            &bound.workspace.to_string(),
            key_version(&wrapped),
            &wrapped,
            &foreign,
        )
        .await
        .expect_err("another workspace's context");
    assert_eq!(
        error,
        KeyMaterialError::ContextMismatch,
        "the encryption context is authenticated by the service, not only by us"
    );
}

/// Tenant and identity isolation at the **value** layer, which is where the
/// secret's own context is enforced.
///
/// This is the assertion the branch-key hierarchy makes load bearing: the
/// wrapped key opens for the whole workspace, so nothing but the AEAD additional
/// data distinguishes one name or one generation from another. `moto` seals with
/// AES-256-GCM over the serialised context, so a frame presented under a context
/// differing in exactly one field fails tag verification — real cryptography,
/// not bookkeeping. The digest comparison catches it **before** any `KMS` call is
/// spent, which is why the error is `ContextMismatch` rather than a key failure.
#[tokio::test]
async fn a_frame_moved_to_another_generation_or_name_never_opens() {
    let engine = MotoContainer::start().await.expect("moto starts");
    let client = client(&engine);
    let bound = context("openai-key", 1);
    let (key, wrapped) = branch_key(&client, &bound).await;

    let crypto = EnvelopeCrypto::new(
        Box::new(KmsBranchKeys::new(client, key)),
        "aex-integration-role",
    );
    let sealed = crypto
        .seal(&bound, &wrapped, &plaintext("hunter2"), now())
        .await
        .expect("seals");

    for foreign in [
        // The same name, the next generation.
        context("openai-key", 2),
        // The same generation, another name.
        context("anthropic-key", 1),
    ] {
        assert_eq!(
            crypto
                .reveal(&sealed, &foreign, now())
                .await
                .expect_err("a frame from another identity"),
            SecretCryptoError::ContextMismatch,
            "the value's own identity is authenticated one layer below the key"
        );
    }
    assert_eq!(
        crypto
            .reveal(&sealed, &bound, now())
            .await
            .expect("its own context")
            .expose_for_encryption(),
        b"hunter2"
    );
}

#[tokio::test]
async fn a_rewrap_between_contexts_survives_a_real_key_round_trip() {
    let engine = MotoContainer::start().await.expect("moto starts");
    let client = client(&engine);
    let source = context("openai-key", 1);
    let (key, wrapped) = branch_key(&client, &source).await;

    let crypto = EnvelopeCrypto::new(
        Box::new(KmsBranchKeys::new(client, key)),
        "aex-integration-role",
    );
    let sealed = crypto
        .seal(&source, &wrapped, &plaintext("hunter2"), now())
        .await
        .expect("seals");

    // The target context differs only in the custody revision, which is exactly
    // the case a session rebind produces.
    let target = session_context("openai-key", 9);
    let rewrapped = crypto
        .rewrap(&sealed, &source, &target, now())
        .await
        .expect("rewraps");
    assert_ne!(
        rewrapped.wrapped_branch_key, sealed.wrapped_branch_key,
        "KMS must emit ciphertext authenticated under the destination context"
    );
    assert_eq!(
        crypto
            .reveal(&rewrapped, &target, now())
            .await
            .expect("reveals")
            .expose_for_encryption(),
        b"hunter2"
    );
    assert_eq!(
        crypto.reveal(&rewrapped, &source, now()).await.unwrap_err(),
        SecretCryptoError::ContextMismatch
    );
    let _ = WRAPPED;
}
