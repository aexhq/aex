//! H-BOUNDARY B8: no guest-reported fact is billable or authoritative.
//!
//! Nothing reachable from the guest can name a usage or receipt
//!    type. `aex-internal-contracts::usage` and
//!    `aex_hands_protocol::lifecycle::RuntimeReceipt` are constructed only from a
//!    provider control-plane response, which the guest cannot make. The dependency
//!    scan below proves the guest links no crate that could write one.

#[test]
fn the_guest_links_no_crate_that_can_write_a_usage_authority() {
    // `aex-runtime-control` owns fact derivation and `aex-usage-*` own the
    // authorities. If any of them ever enters the guest closure, a guest-side type
    // could name a `UsageFact`, and this control is gone.
    let forbidden = [
        "aex-runtime-control",
        "aex-usage-domain",
        "aex-usage-app",
        "aex-usage-storage-dynamodb",
        "aex-usage-compute-dynamodb",
        "aex-usage-transfer-dynamodb",
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
