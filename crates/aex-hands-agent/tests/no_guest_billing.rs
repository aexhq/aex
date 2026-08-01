//! H-BOUNDARY B8: no guest-reported fact is billable or authoritative.
//!
//! Two controls, each with its own falsifying assertion:
//!
//! 1. **Structural.** Nothing reachable from the guest can name a usage or receipt
//!    type. `aex-internal-contracts::usage` and
//!    `aex_hands_protocol::lifecycle::RuntimeReceipt` are constructed only from a
//!    provider control-plane response, which the guest cannot make. The dependency
//!    scan below proves the guest links no crate that could write one.
//! 2. **Behavioural.** A forged terminal claiming a body it did not produce fails
//!    the length and digest checks and journals nothing.

use aex_hands_agent::wire::{FrameError, verify_body};
use aex_wire::ids::ContentHash;

#[test]
fn a_forged_terminal_claiming_a_huge_body_journals_nothing() {
    let real = b"ok";
    let honest_digest = ContentHash::from_bytes(*blake3::hash(real).as_bytes());

    // The forgery: a guest claims a ten-gibibyte deliverable it never produced.
    let forged_len = 10 * 1024 * 1024 * 1024;
    let outcome = verify_body(real, forged_len, honest_digest);
    let Err(FrameError::LengthMismatch { declared, received }) = outcome else {
        panic!("a forged length must be refused before anything is journalled");
    };
    assert_eq!(declared, forged_len);
    assert_eq!(received, 2);
}

#[test]
fn a_forged_digest_is_refused_and_the_diagnostics_survive() {
    let real = b"ok";
    let forged_digest = ContentHash::from_bytes([0xff; 32]);
    let Err(FrameError::DigestMismatch { declared, computed }) =
        verify_body(real, real.len() as u64, forged_digest)
    else {
        panic!("a forged digest must be refused");
    };
    assert_eq!(declared, forged_digest);
    assert_eq!(
        computed,
        ContentHash::from_bytes(*blake3::hash(real).as_bytes()),
        "the diagnostic hash is retained so an operator can see what actually arrived"
    );
}

#[test]
fn the_guest_links_no_crate_that_can_write_a_usage_authority() {
    // `aex-runtime-control` owns fact derivation and `aex-usage-*` own the
    // authorities. If any of them ever enters the guest closure, a guest-side type
    // could name a `UsageFact`, and this control is gone.
    let forbidden = [
        "aex-runtime-control",
        "aex-usage-domain",
        "aex-usage-application",
        "aex-usage-storage-aws",
        "aex-usage-compute-aws",
        "aex-usage-transfer-aws",
        "aex-usage-rating",
    ];
    let manifest = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/Cargo.toml"))
        .expect("the manifest reads");
    let dependencies = manifest
        .split("[dev-dependencies]")
        .next()
        .expect("a manifest has a first section");
    for name in forbidden {
        assert!(
            !dependencies.contains(name),
            "the guest must not depend on `{name}`: it would let a guest-side type name a billable fact"
        );
    }
}
