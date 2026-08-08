//! Plaintext non-persistence: plan 04 items 99 and 101.
//!
//! These checks exercise the public behavior: rendering never emits plaintext,
//! and fingerprints are salted, one-way values. The plaintext module's unit
//! suite directly exercises the zeroizing buffer wrapper.

use aex_secret_domain::SecretPlaintext;
use aex_wire::ids::{PrefixedId as _, ProviderCredentialId, Uuid7, WorkspaceId};

/// 99 `plaintext_never_serialized`, behavioural check.
#[test]
fn no_rendering_emits_a_plaintext_byte() {
    let secret = b"correct-horse-battery-staple";
    let value = SecretPlaintext::new(secret.to_vec()).expect("accepted");
    let rendered = format!("{value}|{value:?}");
    assert!(!rendered.contains("correct-horse"));
    assert!(!rendered.contains("battery"));
    for window in secret.windows(4) {
        let fragment = std::str::from_utf8(window).expect("ascii fixture");
        assert!(
            !rendered.contains(fragment),
            "rendering leaked the fragment `{fragment}`"
        );
    }
    assert_eq!(value.expose_for_encryption(), secret);
}

// --- provider-credential fingerprint -----------------------------------------

/// The persisted fingerprint is one-way and is salted by the binding it
/// describes, so it can be shown to a customer without becoming an oracle.
#[test]
fn a_credential_fingerprint_is_salted_by_its_binding() {
    let workspace = WorkspaceId::from_uuid7(Uuid7::compose(1_754_051_696_789, [1; 10]));
    let other_workspace = WorkspaceId::from_uuid7(Uuid7::compose(1_754_051_696_789, [2; 10]));
    let credential = ProviderCredentialId::from_uuid7(Uuid7::compose(1_754_051_696_789, [3; 10]));
    let other_credential =
        ProviderCredentialId::from_uuid7(Uuid7::compose(1_754_051_696_789, [4; 10]));
    let value = || SecretPlaintext::new(b"sk-live-correct-horse".to_vec()).expect("accepted");

    let base = value().credential_fingerprint(workspace, credential);
    assert_eq!(
        base,
        value().credential_fingerprint(workspace, credential),
        "the same key under the same binding fingerprints identically"
    );
    assert_ne!(
        base,
        value().credential_fingerprint(other_workspace, credential),
        "the same key in another workspace must not fingerprint identically"
    );
    assert_ne!(
        base,
        value().credential_fingerprint(workspace, other_credential),
        "the same key under another binding must not fingerprint identically"
    );
    assert_ne!(
        base,
        SecretPlaintext::new(b"sk-live-correct-horsf".to_vec())
            .expect("accepted")
            .credential_fingerprint(workspace, credential),
        "a different key must not fingerprint identically"
    );
}

/// A fingerprint is a digest, never a rendering: no byte of the plaintext, and
/// no plain SHA-256 of it, survives into the published value.
#[test]
fn a_credential_fingerprint_carries_no_plaintext_and_is_not_a_bare_digest() {
    use sha2::Digest as _;

    let workspace = WorkspaceId::from_uuid7(Uuid7::compose(1_754_051_696_789, [1; 10]));
    let credential = ProviderCredentialId::from_uuid7(Uuid7::compose(1_754_051_696_789, [3; 10]));
    let secret = b"sk-live-correct-horse-battery-staple";
    let fingerprint = SecretPlaintext::new(secret.to_vec())
        .expect("accepted")
        .credential_fingerprint(workspace, credential);

    let rendered = fingerprint.to_wire();
    for window in secret.windows(4) {
        let fragment = std::str::from_utf8(window).expect("ascii fixture");
        assert!(
            !rendered.contains(fragment),
            "the fingerprint leaked the fragment `{fragment}`"
        );
    }

    let bare: [u8; 32] = sha2::Sha256::digest(secret).into();
    assert_ne!(
        *fingerprint.as_bytes(),
        bare,
        "an unsalted digest would let one precomputed table cover every workspace"
    );
}
