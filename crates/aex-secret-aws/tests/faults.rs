//! Failure-path cases for the secret cryptography adapter.

mod support;

use aex_secret_aws::crypto::{EnvelopeCrypto, SecretCrypto, SecretCryptoError};
use aex_secret_aws::keystore::KeyMaterialError;

use support::{FakeKeys, Pinned, WRAPPED, context, now, plaintext, session_context};

fn crypto(keys: FakeKeys) -> EnvelopeCrypto {
    EnvelopeCrypto::new(Box::new(keys), "role-a").with_entropy(Box::new(Pinned(0x22)))
}

fn run<T>(future: impl std::future::Future<Output = T>) -> T {
    tokio::runtime::Builder::new_current_thread()
        .build()
        .expect("a current-thread runtime")
        .block_on(future)
}

#[test]
fn a_value_moved_between_tenants_is_refused_before_a_kms_call_is_spent() {
    let keys = FakeKeys::new();
    let crypto = crypto(keys.clone());
    let mine = context("openai-key", 1);
    let sealed = run(crypto.seal(&mine, WRAPPED, &plaintext("hunter2"), now())).expect("seals");
    let calls_after_seal = keys.calls();

    let theirs = context("openai-key", 2);
    let error = run(crypto.reveal(&sealed, &theirs, now())).expect_err("another tenant");
    assert_eq!(
        error,
        SecretCryptoError::ContextMismatch,
        "a row that moved tenants is caught by the stored digest"
    );
    assert_eq!(
        keys.calls(),
        calls_after_seal,
        "the mismatch must be caught before KMS is asked anything"
    );
}

#[test]
fn a_value_moved_between_custody_revisions_is_refused() {
    let crypto = crypto(FakeKeys::new());
    let first = session_context("openai-key", 1);
    let sealed = run(crypto.seal(&first, WRAPPED, &plaintext("v"), now())).expect("seals");
    let error = run(crypto.reveal(&sealed, &session_context("openai-key", 2), now()))
        .expect_err("another revision");
    assert_eq!(error, SecretCryptoError::ContextMismatch);
}

#[test]
fn a_tampered_frame_is_refused_even_under_the_right_context() {
    let crypto = crypto(FakeKeys::new());
    let bound = context("openai-key", 1);
    let mut sealed = run(crypto.seal(&bound, WRAPPED, &plaintext("v"), now())).expect("seals");
    let last = sealed.frame.len() - 1;
    sealed.frame[last] ^= 0x01;
    let error = run(crypto.reveal(&sealed, &bound, now())).expect_err("a flipped tag bit");
    assert!(matches!(error, SecretCryptoError::Envelope(_)), "{error}");
}

#[test]
fn a_denied_role_gets_a_typed_denial_rather_than_a_weaker_key() {
    let crypto = crypto(FakeKeys::denied());
    let bound = context("openai-key", 1);
    let error = run(crypto.seal(&bound, WRAPPED, &plaintext("v"), now())).expect_err("denied");
    assert_eq!(
        error,
        SecretCryptoError::KeyMaterial(KeyMaterialError::Denied),
        "there is no fallback path to a weaker key, only a typed denial"
    );
}

#[test]
fn a_frame_that_is_not_this_format_is_refused_rather_than_parsed() {
    let crypto = crypto(FakeKeys::new());
    let bound = context("openai-key", 1);
    let mut sealed = run(crypto.seal(&bound, WRAPPED, &plaintext("v"), now())).expect("seals");
    sealed.frame = b"AEX1".to_vec();
    assert!(run(crypto.reveal(&sealed, &bound, now())).is_err());

    let mut empty = run(crypto.seal(&bound, WRAPPED, &plaintext("v"), now())).expect("seals");
    empty.frame.clear();
    assert!(run(crypto.reveal(&empty, &bound, now())).is_err());
}

#[test]
fn a_rewrap_whose_source_context_is_wrong_never_produces_a_target_value() {
    let crypto = crypto(FakeKeys::new());
    let source = context("openai-key", 1);
    let sealed = run(crypto.seal(&source, WRAPPED, &plaintext("v"), now())).expect("seals");
    let error = run(crypto.rewrap(
        &sealed,
        &context("openai-key", 2),
        &session_context("openai-key", 5),
        now(),
    ))
    .expect_err("the source context is wrong");
    assert_eq!(error, SecretCryptoError::ContextMismatch);
}
