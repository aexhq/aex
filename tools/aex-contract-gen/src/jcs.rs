//! RFC 8785 JCS over UTF-8 byte ordering, plus the fixed pretty printer used for
//! every checked-in JSON artifact.
//!
//! The generator carries its own copy rather than depending on `aex-wire`,
//! because `aex-wire`'s own source is one of this tool's outputs and a build
//! dependency in that direction would make a broken emission unrepairable.
//! `tests/canonical_agreement.rs` proves the two implementations agree byte for
//! byte, which is the property that actually matters.

use serde_json::Value;

/// Canonical JCS bytes for `value`.
///
/// `serde_json::Map` is a `BTreeMap` in this workspace, so member ordering is
/// already the UTF-8 byte order the workspace pinned.
///
/// # Panics
///
/// Never: a `Value` is always serializable.
#[must_use]
pub fn to_jcs_bytes(value: &Value) -> Vec<u8> {
    serde_json::to_vec(value).expect("a serde_json::Value always serializes")
}

/// The SHA-256 of the JCS bytes, rendered `sha256:<64 lowercase hex>`.
#[must_use]
pub fn digest(value: &Value) -> String {
    use sha2::Digest as _;
    let mut hasher = sha2::Sha256::new();
    hasher.update(to_jcs_bytes(value));
    let bytes: [u8; 32] = hasher.finalize().into();
    let mut out = String::with_capacity(71);
    out.push_str("sha256:");
    for byte in bytes {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

/// The SHA-256 of arbitrary bytes, rendered `sha256:<64 lowercase hex>`.
#[must_use]
pub fn digest_bytes(bytes: &[u8]) -> String {
    use sha2::Digest as _;
    let mut hasher = sha2::Sha256::new();
    hasher.update(bytes);
    let raw: [u8; 32] = hasher.finalize().into();
    let mut out = String::with_capacity(71);
    out.push_str("sha256:");
    for byte in raw {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

/// Renders a checked-in JSON artifact: two-space indent, `\n` only, trailing
/// newline, members in UTF-8 byte order.
///
/// # Panics
///
/// Never: a `Value` is always serializable.
#[must_use]
pub fn to_pretty(value: &Value) -> String {
    let mut text =
        serde_json::to_string_pretty(value).expect("a serde_json::Value always serializes");
    text.push('\n');
    text
}
