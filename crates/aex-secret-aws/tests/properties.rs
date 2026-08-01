//! Round-trip and binding properties for the secret cryptography adapter.

mod support;

use aex_secret_aws::crypto::{EnvelopeCrypto, SecretCrypto};
use proptest::prelude::*;

use support::{FakeKeys, Pinned, WRAPPED, context, now, plaintext, session_context};

fn crypto(keys: FakeKeys) -> EnvelopeCrypto {
    EnvelopeCrypto::new(Box::new(keys), "role-a").with_entropy(Box::new(Pinned(0x11)))
}

fn run<T>(future: impl std::future::Future<Output = T>) -> T {
    tokio::runtime::Builder::new_current_thread()
        .build()
        .expect("a current-thread runtime")
        .block_on(future)
}

proptest! {
    #[test]
    fn any_value_seals_and_reveals_unchanged(value in "\\PC{1,512}") {
        let crypto = crypto(FakeKeys::new());
        let bound = context("openai-key", 1);
        let sealed = run(crypto.seal(&bound, WRAPPED, &plaintext(&value), now())).expect("seals");
        let revealed = run(crypto.reveal(&sealed, &bound, now())).expect("reveals");
        prop_assert_eq!(revealed.expose_for_encryption(), value.as_bytes());
    }

    #[test]
    fn a_sealed_frame_never_contains_its_own_plaintext(value in "[a-z]{8,64}") {
        let crypto = crypto(FakeKeys::new());
        let bound = context("openai-key", 1);
        let sealed = run(crypto.seal(&bound, WRAPPED, &plaintext(&value), now())).expect("seals");
        prop_assert!(
            !sealed
                .frame
                .windows(value.len())
                .any(|window| window == value.as_bytes())
        );
    }
}

#[test]
fn a_rewrap_moves_a_value_between_contexts_without_the_caller_ever_holding_it() {
    let crypto = crypto(FakeKeys::new());
    let source = context("openai-key", 1);
    let target = session_context("openai-key", 9);

    let sealed = run(crypto.seal(&source, WRAPPED, &plaintext("hunter2"), now())).expect("seals");
    let rewrapped = run(crypto.rewrap(&sealed, &source, &target, now())).expect("rewraps");

    assert_ne!(rewrapped.frame, sealed.frame);
    assert_ne!(rewrapped.context_digest, sealed.context_digest);
    let revealed = run(crypto.reveal(&rewrapped, &target, now())).expect("reveals");
    assert_eq!(revealed.expose_for_encryption(), b"hunter2");
    assert!(
        run(crypto.reveal(&rewrapped, &source, now())).is_err(),
        "the rewrapped value belongs to the target context alone"
    );
}

#[test]
fn the_branch_key_cache_absorbs_repeated_operations() {
    let keys = FakeKeys::new();
    let crypto = crypto(keys.clone());
    let bound = context("openai-key", 1);

    for _ in 0..8 {
        let sealed = run(crypto.seal(&bound, WRAPPED, &plaintext("v"), now())).expect("seals");
        run(crypto.reveal(&sealed, &bound, now())).expect("reveals");
    }
    assert_eq!(
        keys.calls(),
        1,
        "the cache is what keeps a KMS request off every operation"
    );
    assert_eq!(crypto.cache().len(), 1);
}

#[test]
fn a_stored_digest_is_the_digest_of_the_context_the_value_was_sealed_under() {
    let crypto = crypto(FakeKeys::new());
    let bound = context("openai-key", 1);
    let sealed = run(crypto.seal(&bound, WRAPPED, &plaintext("v"), now())).expect("seals");
    assert_eq!(
        sealed.context_digest,
        aex_secret_aws::context::context_digest(&bound)
    );
}
