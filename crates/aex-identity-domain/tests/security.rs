//! Security properties: redaction, constant-time comparison, and the rule that
//! a structural parse leaks nothing about database state.

use aex_control_domain::CursorSecret;
use aex_identity_domain::assertion::{KeyId, LocalSigner};
use aex_identity_domain::credential::{
    CredentialKind, PresentedDigest, RegionCode, SecretRng, WorkspacePin, mint, parse, verifier,
    verify,
};
use aex_identity_domain::{NormalizedEmail, Pepper};
use std::time::Instant;
use uuid::Uuid;
use zeroize::Zeroizing;

/// A generator whose bytes are all the same, so a token is reproducible.
struct Fixed(u8);

impl SecretRng for Fixed {
    fn fill(&self, out: &mut [u8]) {
        out.fill(self.0);
    }
}

fn id() -> Uuid {
    Uuid::from_u128(0x0192_3f2a_1c00_7000_8000_0000_0000_0001)
}

fn pin() -> WorkspacePin {
    WorkspacePin {
        region: RegionCode::ALL[4],
        workspace: Uuid::from_u128(0x0192_3f2a_1c00_7000_8000_0000_0000_0002),
    }
}

#[test]
fn no_secret_carrying_type_renders_its_contents() {
    let (secret, digest) = mint(
        CredentialKind::WorkspaceKey,
        Some(pin()),
        id(),
        &Fixed(0x11),
    );
    let pepper = Pepper::new([0x22_u8; 32]);
    let stored = verifier(&pepper, &digest);
    let cursor = CursorSecret::new([0x33_u8; 32]);
    let signer = LocalSigner::new(KeyId::new(id()), &Zeroizing::new([0x44_u8; 32]));
    let email = NormalizedEmail::parse("alice@example.com").expect("valid");

    let rendered = [
        format!("{secret:?}"),
        format!("{digest:?}"),
        format!("{pepper:?}"),
        format!("{stored:?}"),
        format!("{cursor:?}"),
        format!("{signer:?}"),
        format!("{email:?}"),
    ];
    for text in &rendered {
        assert!(
            text.contains("<redacted:"),
            "a secret-carrying type rendered as `{text}`"
        );
    }

    // The plaintext, the pepper bytes and the address must appear nowhere.
    let plaintext = secret.expose().to_owned();
    let corpus = rendered.join(" ");
    for needle in [plaintext.as_str(), "alice", "example.com", "\\x22"] {
        assert!(
            !corpus.contains(needle),
            "`{needle}` leaked into a diagnostic"
        );
    }
}

#[test]
fn the_plaintext_is_only_reachable_through_an_explicit_call() {
    let (secret, _) = mint(CredentialKind::DashboardSession, None, id(), &Fixed(0x55));
    // `expose` is the only accessor, and it is named so a review notices it.
    assert!(secret.expose().starts_with("aex_ds_"));
    assert!(!format!("{secret:?}").contains("aex_ds_"));
}

#[test]
fn a_structural_parse_never_compares_a_secret() {
    // Two tokens differing only in their secret parse identically apart from the
    // digest, so a parse failure cannot distinguish "no such credential" from
    // "wrong secret" — that distinction only exists after the database lookup.
    let (first, _) = mint(CredentialKind::DashboardSession, None, id(), &Fixed(0x01));
    let (second, _) = mint(CredentialKind::DashboardSession, None, id(), &Fixed(0x02));
    let a = parse(CredentialKind::DashboardSession, first.expose()).expect("parses");
    let b = parse(CredentialKind::DashboardSession, second.expose()).expect("parses");
    assert_eq!(a.id, b.id);
    assert_eq!(a.kind, b.kind);
    assert_ne!(a.digest, b.digest);
}

/// How many verifications each timing sample runs.
const ROUNDS: usize = 100_000;

#[test]
fn verification_is_constant_time_within_measurement_noise() {
    // A statistical check rather than a proof: `subtle::ConstantTimeEq` is the
    // guarantee, and this asserts the call site actually uses it by showing that
    // a verifier differing in its first byte costs the same as one differing in
    // its last.
    let pepper = Pepper::new([0x66_u8; 32]);
    let (_, digest) = mint(CredentialKind::EmailChallenge, None, id(), &Fixed(0x77));
    let stored = verifier(&pepper, &digest);

    let mut early = *stored.as_bytes();
    early[0] ^= 0xff;
    let mut late = *stored.as_bytes();
    late[31] ^= 0xff;
    let early = aex_identity_domain::credential::Verifier::from_bytes(early);
    let late = aex_identity_domain::credential::Verifier::from_bytes(late);

    let measure = |target: &aex_identity_domain::credential::Verifier| {
        let start = Instant::now();
        let mut accepted = 0_usize;
        for _ in 0..ROUNDS {
            if verify(&pepper, &digest, target) {
                accepted += 1;
            }
        }
        assert_eq!(accepted, 0, "a mutated verifier must never match");
        start.elapsed().as_nanos()
    };

    let first = measure(&early);
    let last = measure(&late);
    let (low, high) = if first <= last {
        (first, last)
    } else {
        (last, first)
    };
    // A short-circuiting comparison would make the early-difference case
    // roughly 32 times cheaper. A 4x envelope is loose enough to survive a busy
    // machine and tight enough to catch a `==` on byte arrays.
    assert!(
        high <= low.saturating_mul(4).max(1),
        "early-difference {first} ns and late-difference {last} ns diverge too far"
    );
}

#[test]
fn a_wrong_kind_prefix_is_refused_before_anything_else_is_examined() {
    let (secret, _) = mint(CredentialKind::WorkspaceKey, Some(pin()), id(), &Fixed(1));
    // Truncated to just the prefix: the parser must refuse on the prefix rather
    // than reach a length or encoding check that could reveal the shape.
    assert!(parse(CredentialKind::DashboardSession, secret.expose()).is_err());
    assert!(parse(CredentialKind::DashboardSession, "aex_wk_").is_err());
}

#[test]
fn the_transmitted_digest_carries_no_recoverable_secret() {
    let (secret, digest) = mint(CredentialKind::WorkspaceKey, Some(pin()), id(), &Fixed(2));
    let transmitted = digest.to_base64url();
    assert!(!transmitted.contains(secret.expose()));
    assert_eq!(
        PresentedDigest::from_base64url(&transmitted),
        Ok(digest),
        "the digest survives the region boundary unchanged"
    );
    // The digest alone cannot authenticate: verification still needs the pepper.
    let stored = verifier(&Pepper::new([9_u8; 32]), &digest);
    assert!(!verify(&Pepper::new([8_u8; 32]), &digest, &stored));
}
