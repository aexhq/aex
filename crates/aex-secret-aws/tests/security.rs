//! Redaction, leakage and least-exposure cases for the secret cryptography
//! adapter.

mod support;

use aex_secret_aws::context::{NAME_DIGEST_KEY, NAME_KEY, kms_pairs};
use aex_secret_aws::crypto::{EnvelopeCrypto, SecretCrypto};
use aex_secret_domain::plaintext::REDACTED;

use support::{FakeKeys, Pinned, WRAPPED, context, now, plaintext};

const VALUE: &str = "sk-live-0123456789abcdef";

fn crypto(keys: FakeKeys) -> EnvelopeCrypto {
    EnvelopeCrypto::new(Box::new(keys), "role-a").with_entropy(Box::new(Pinned(0x33)))
}

fn run<T>(future: impl std::future::Future<Output = T>) -> T {
    tokio::runtime::Builder::new_current_thread()
        .build()
        .expect("a current-thread runtime")
        .block_on(future)
}

#[test]
fn no_rendering_of_anything_this_adapter_returns_prints_a_secret() {
    let crypto = crypto(FakeKeys::new());
    let bound = context("openai-key", 1);
    let sealed = run(crypto.seal(&bound, WRAPPED, &plaintext(VALUE), now())).expect("seals");
    let revealed = run(crypto.reveal(&sealed, &bound, now())).expect("reveals");
    let rewrapped = run(crypto.rewrap(
        &sealed,
        &bound,
        &support::session_context("openai-key", 9),
        now(),
    ))
    .expect("rewraps");

    for rendering in [
        format!("{sealed:?}"),
        format!("{revealed:?}"),
        format!("{rewrapped:?}"),
        format!("{revealed}"),
        format!("{crypto:?}"),
    ] {
        assert!(!rendering.contains(VALUE), "{rendering}");
        assert!(!rendering.contains("sk-live"), "{rendering}");
    }
    assert!(format!("{revealed}").contains(REDACTED));
}

#[test]
fn the_context_that_reaches_kms_carries_identifiers_and_a_name_digest_only() {
    let pairs = kms_pairs(&context("acme-production-stripe", 1));
    assert!(!pairs.contains_key(NAME_KEY));
    assert_eq!(pairs[NAME_DIGEST_KEY].len(), 64);
    for value in pairs.values() {
        assert!(
            !value.contains("acme"),
            "an encryption context is recorded in CloudTrail: {value}"
        );
    }
}

#[test]
fn the_cache_never_prints_the_material_it_holds() {
    let crypto = crypto(FakeKeys::new());
    let bound = context("openai-key", 1);
    run(crypto.seal(&bound, WRAPPED, &plaintext(VALUE), now())).expect("seals");
    let printed = format!("{:?}", crypto.cache());
    assert!(printed.contains("<redacted>"), "{printed}");
    assert!(!printed.contains("2a2a"), "{printed}");
}

#[test]
fn clearing_the_cache_forces_the_next_operation_back_through_the_key_provider() {
    let keys = FakeKeys::new();
    let crypto = crypto(keys.clone());
    let bound = context("openai-key", 1);
    run(crypto.seal(&bound, WRAPPED, &plaintext(VALUE), now())).expect("seals");
    assert_eq!(keys.calls(), 1);
    crypto.cache().clear();
    run(crypto.seal(&bound, WRAPPED, &plaintext(VALUE), now())).expect("seals");
    assert_eq!(
        keys.calls(),
        2,
        "a cleared cache must not keep serving material it no longer holds"
    );
}

#[test]
fn a_sealed_value_carries_the_wrapped_key_and_never_an_unwrapped_one() {
    let crypto = crypto(FakeKeys::new());
    let bound = context("openai-key", 1);
    let sealed = run(crypto.seal(&bound, WRAPPED, &plaintext(VALUE), now())).expect("seals");
    assert_eq!(sealed.wrapped_branch_key, WRAPPED);
    let stored = sealed.to_ciphertext_ref(1);
    assert_eq!(stored.wrapped_key, WRAPPED);
    assert_eq!(stored.ciphertext, sealed.frame);
    assert!(
        !stored
            .ciphertext
            .windows(VALUE.len())
            .any(|window| window == VALUE.as_bytes())
    );
}
