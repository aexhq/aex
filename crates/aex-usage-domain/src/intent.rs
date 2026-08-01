//! RFC 8785 canonical JSON and the BLAKE3 intent hash.
//!
//! One canonicalization rule holds workspace-wide: RFC 8785 JCS with object
//! members ordered by UTF-16 code unit. This module is the usage authority's
//! single implementation of it; nothing else in these crates may introduce a
//! second canonicalizer or comparator.
//!
//! The intent hash answers exactly one question: two producers offered the same
//! deterministic fact identity — did they mean the same measurement? A matching
//! hash is an idempotent replay. A differing hash is an identity conflict and is
//! never silently resolved in either direction.

use std::fmt;

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Why a value could not be canonicalized.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CanonicalError {
    /// A number was not an exact integer.
    ///
    /// No usage value is ever fractional, so admitting one here would let a
    /// float reach the hash the authority fences on.
    #[error("number `{value}` is not an exact integer; the usage authority carries no fractions")]
    NonIntegerNumber {
        /// The offending literal.
        value: String,
    },
    /// A number was not finite.
    #[error("number `{value}` is not finite")]
    NonFinite {
        /// The offending literal.
        value: String,
    },
    /// The value could not be represented as JSON at all.
    #[error("value could not be represented as JSON: {reason}")]
    NotRepresentable {
        /// Why serialization failed.
        reason: String,
    },
}

/// Renders `value` as RFC 8785 canonical JSON.
///
/// Object members are ordered by the UTF-16 code units of their names, string
/// escaping follows the JCS minimal-escape rule, `-0` normalizes to `0`, and a
/// non-integer or non-finite number is refused rather than rounded.
///
/// # Errors
///
/// Returns [`CanonicalError`] when the value contains a fractional or
/// non-finite number.
pub fn canonical_json(value: &Value) -> Result<String, CanonicalError> {
    let mut out = String::new();
    write_value(value, &mut out)?;
    Ok(out)
}

fn write_value(value: &Value, out: &mut String) -> Result<(), CanonicalError> {
    match value {
        Value::Null => out.push_str("null"),
        Value::Bool(true) => out.push_str("true"),
        Value::Bool(false) => out.push_str("false"),
        Value::Number(number) => write_number(number, out)?,
        Value::String(text) => write_string(text, out),
        Value::Array(items) => {
            out.push('[');
            for (index, item) in items.iter().enumerate() {
                if index > 0 {
                    out.push(',');
                }
                write_value(item, out)?;
            }
            out.push(']');
        }
        Value::Object(members) => {
            let mut keys: Vec<&String> = members.keys().collect();
            keys.sort_by(|left, right| utf16_order(left, right));
            out.push('{');
            for (index, key) in keys.into_iter().enumerate() {
                if index > 0 {
                    out.push(',');
                }
                write_string(key, out);
                out.push(':');
                write_value(&members[key], out)?;
            }
            out.push('}');
        }
    }
    Ok(())
}

fn write_number(number: &serde_json::Number, out: &mut String) -> Result<(), CanonicalError> {
    if let Some(value) = number.as_u64() {
        out.push_str(&value.to_string());
        return Ok(());
    }
    if let Some(value) = number.as_i64() {
        // `-0` is only reachable through the float arm; an i64 zero is `0`.
        out.push_str(&value.to_string());
        return Ok(());
    }
    let literal = number.to_string();
    let Some(value) = number.as_f64() else {
        return Err(CanonicalError::NonFinite { value: literal });
    };
    if !value.is_finite() {
        return Err(CanonicalError::NonFinite { value: literal });
    }
    Err(CanonicalError::NonIntegerNumber { value: literal })
}

/// Orders two object member names by their UTF-16 code units, as RFC 8785 requires.
fn utf16_order(left: &str, right: &str) -> std::cmp::Ordering {
    left.encode_utf16().cmp(right.encode_utf16())
}

fn write_string(value: &str, out: &mut String) {
    out.push('"');
    for character in value.chars() {
        match character {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            other if (other as u32) < 0x20 => {
                out.push_str(&format!("\\u{:04x}", other as u32));
            }
            other => out.push(other),
        }
    }
    out.push('"');
}

/// Why a digest could not be parsed.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("`{value}` is not a 64-character lowercase hex BLAKE3 digest")]
pub struct DigestError {
    /// The value that was refused.
    pub value: String,
}

/// A BLAKE3-256 digest in its canonical lowercase hex form.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct Blake3Digest([u8; 32]);

impl Blake3Digest {
    /// Hashes an arbitrary byte string.
    #[must_use]
    pub fn of_bytes(bytes: &[u8]) -> Self {
        Self(*blake3::hash(bytes).as_bytes())
    }

    /// The raw digest bytes.
    #[must_use]
    pub const fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    /// Parses the lowercase hex form written to a row.
    ///
    /// # Errors
    ///
    /// Returns [`DigestError`] for anything that is not 64 lowercase hex
    /// characters.
    pub fn parse(value: &str) -> Result<Self, DigestError> {
        let refuse = || DigestError {
            value: value.to_owned(),
        };
        if value.len() != 64 || value.bytes().any(|byte| byte.is_ascii_uppercase()) {
            return Err(refuse());
        }
        let bytes = hex::decode(value).map_err(|_| refuse())?;
        let digest: [u8; 32] = bytes.try_into().map_err(|_| refuse())?;
        Ok(Self(digest))
    }
}

impl fmt::Display for Blake3Digest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&hex::encode(self.0))
    }
}

impl TryFrom<String> for Blake3Digest {
    type Error = DigestError;

    fn try_from(value: String) -> Result<Self, DigestError> {
        Self::parse(&value)
    }
}

impl From<Blake3Digest> for String {
    fn from(value: Blake3Digest) -> Self {
        hex::encode(value.0)
    }
}

/// A BLAKE3 digest over one canonical JSON document.
///
/// Distinct from [`Blake3Digest`] on purpose: an intent hash answers "did two
/// producers mean the same measurement?" and is compared inside the admission
/// fence, while a receipt digest is provenance carried alongside the row.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct IntentHash(Blake3Digest);

/// Why an intent hash could not be parsed.
pub type IntentHashError = DigestError;

impl IntentHash {
    /// Hashes an already-canonical document.
    #[must_use]
    pub fn of_canonical(canonical: &str) -> Self {
        Self(Blake3Digest::of_bytes(canonical.as_bytes()))
    }

    /// Canonicalizes and hashes any serializable value.
    ///
    /// # Errors
    ///
    /// Returns [`CanonicalError`] when the value cannot be serialized or
    /// contains a fractional or non-finite number.
    pub fn of<T: Serialize>(value: &T) -> Result<Self, CanonicalError> {
        let json = serde_json::to_value(value).map_err(|error| CanonicalError::NotRepresentable {
            reason: error.to_string(),
        })?;
        Ok(Self::of_canonical(&canonical_json(&json)?))
    }

    /// The raw digest bytes.
    #[must_use]
    pub const fn as_bytes(&self) -> &[u8; 32] {
        self.0.as_bytes()
    }

    /// Parses the lowercase hex form written to a row.
    ///
    /// # Errors
    ///
    /// Returns [`IntentHashError`] for anything that is not 64 lowercase hex
    /// characters.
    pub fn parse(value: &str) -> Result<Self, IntentHashError> {
        Blake3Digest::parse(value).map(Self)
    }
}

impl fmt::Display for IntentHash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&self.0, f)
    }
}

impl TryFrom<String> for IntentHash {
    type Error = IntentHashError;

    fn try_from(value: String) -> Result<Self, IntentHashError> {
        Self::parse(&value)
    }
}

impl From<IntentHash> for String {
    fn from(value: IntentHash) -> Self {
        value.0.into()
    }
}

#[cfg(test)]
mod tests {
    use super::{CanonicalError, IntentHash, canonical_json};
    use serde_json::json;

    #[test]
    fn object_members_are_ordered_by_utf16_code_unit() {
        let value = json!({ "b": 1, "a": 2, "A": 3, "\u{00e9}": 4, "\u{1f600}": 5, "\u{ff3a}": 6 });
        assert_eq!(
            canonical_json(&value).expect("canonical"),
            "{\"A\":3,\"a\":2,\"b\":1,\"\u{00e9}\":4,\"\u{ff3a}\":6,\"\u{1f600}\":5}"
        );
    }

    #[test]
    fn key_order_does_not_change_the_hash() {
        let left = json!({ "alpha": 1, "beta": [1, 2, 3] });
        let right = json!({ "beta": [1, 2, 3], "alpha": 1 });
        assert_eq!(
            IntentHash::of(&left).expect("hash"),
            IntentHash::of(&right).expect("hash")
        );
    }

    #[test]
    fn a_changed_value_changes_the_hash() {
        let left = json!({ "quantity": "10" });
        let right = json!({ "quantity": "11" });
        assert_ne!(
            IntentHash::of(&left).expect("hash"),
            IntentHash::of(&right).expect("hash")
        );
    }

    #[test]
    fn fractional_and_non_finite_numbers_are_refused() {
        assert!(matches!(
            canonical_json(&json!({ "value": 1.5 })),
            Err(CanonicalError::NonIntegerNumber { .. })
        ));
        // `-0.0` survives serde_json as a float and must not silently become `0`.
        assert!(canonical_json(&json!({ "value": -0.0 })).is_err());
        assert_eq!(
            canonical_json(&json!({ "value": 0 })).expect("integer zero"),
            "{\"value\":0}"
        );
    }

    #[test]
    fn control_characters_use_the_minimal_escape() {
        let value = json!({ "text": "a\nb\u{1}c\"d\\e" });
        assert_eq!(
            canonical_json(&value).expect("canonical"),
            "{\"text\":\"a\\nb\\u0001c\\\"d\\\\e\"}"
        );
    }

    #[test]
    fn digests_round_trip_through_lowercase_hex() {
        let hash = IntentHash::of(&json!({ "a": 1 })).expect("hash");
        assert_eq!(
            IntentHash::parse(&hash.to_string()).expect("round trip"),
            hash
        );
        assert!(IntentHash::parse(&hash.to_string().to_uppercase()).is_err());
        assert!(IntentHash::parse("deadbeef").is_err());
    }
}
