//! The `BLAKE3` intent hash over the one workspace canonicalization rule.
//!
//! Canonicalization itself is **not** implemented here. RFC 8785 JCS over UTF-8
//! byte ordering lives in [`aex_wire::canonical`] and that is the only
//! implementation in the workspace: two canonicalizers mean two answers to "is
//! this the same measurement", which is exactly the ambiguity the admission
//! fence exists to remove.
//!
//! The intent hash answers one question: two producers offered the same
//! deterministic fact identity — did they mean the same measurement? A matching
//! hash is an idempotent replay. A differing hash is an identity conflict and is
//! never silently resolved in either direction.

use std::fmt;

use aex_wire::canonical::{CanonicalError as WireCanonicalError, to_jcs_bytes};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Why a value could not be reduced to an intent hash.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CanonicalError {
    /// A number was not an exact integer.
    ///
    /// No usage value is ever fractional. The typed model makes a fraction
    /// unconstructable, so this is a defence-in-depth check over the canonical
    /// document rather than an expected path.
    #[error("number `{value}` is not an exact integer; the usage authority carries no fractions")]
    NonIntegerNumber {
        /// The offending literal.
        value: String,
    },
    /// The workspace canonicalizer refused the value.
    #[error(transparent)]
    Wire(#[from] WireCanonicalError),
    /// The value could not be represented as JSON at all.
    #[error("value could not be represented as JSON: {reason}")]
    NotRepresentable {
        /// Why serialization failed.
        reason: String,
    },
}

/// Refuses any non-integer number anywhere in a canonical document.
///
/// [`aex_wire::canonical`] normalizes an integral float to an integer and
/// rejects a magnitude at or above `1e21`; it deliberately does not reject a
/// genuine fraction, because other planes legitimately carry one. The usage
/// authority never does, so the fence is applied here.
fn reject_fractions(value: &Value) -> Result<(), CanonicalError> {
    match value {
        Value::Number(number) => {
            if number.is_f64() {
                return Err(CanonicalError::NonIntegerNumber {
                    value: number.to_string(),
                });
            }
            Ok(())
        }
        Value::Array(items) => items.iter().try_for_each(reject_fractions),
        Value::Object(members) => members.values().try_for_each(reject_fractions),
        Value::Null | Value::Bool(_) | Value::String(_) => Ok(()),
    }
}

/// Why a digest could not be parsed.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("`{value}` is not a 64-character lowercase hex BLAKE3 digest")]
pub struct DigestError {
    /// The value that was refused.
    pub value: String,
}

/// A `BLAKE3`-256 digest in its canonical lowercase hex form.
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

/// A `BLAKE3` digest over one canonical JSON document.
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
    ///
    /// The caller is asserting that `canonical` came out of
    /// [`aex_wire::canonical`]; nothing else produces a comparable byte string.
    #[must_use]
    pub fn of_canonical(canonical: &[u8]) -> Self {
        Self(Blake3Digest::of_bytes(canonical))
    }

    /// Canonicalizes any serializable value through [`aex_wire::canonical`] and
    /// hashes the result.
    ///
    /// # Errors
    ///
    /// Returns [`CanonicalError::Wire`] when the workspace canonicalizer refuses
    /// the value, and [`CanonicalError::NonIntegerNumber`] when the document
    /// contains a fraction.
    pub fn of<T: Serialize>(value: &T) -> Result<Self, CanonicalError> {
        let json =
            serde_json::to_value(value).map_err(|error| CanonicalError::NotRepresentable {
                reason: error.to_string(),
            })?;
        reject_fractions(&json)?;
        Ok(Self::of_canonical(&to_jcs_bytes(&json)?))
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
    use super::{CanonicalError, IntentHash, reject_fractions};
    use aex_wire::canonical::to_jcs_string;
    use serde_json::json;

    #[test]
    fn canonicalization_is_delegated_to_the_one_workspace_rule() {
        // Ordering, escaping and number form are `aex_wire`'s answers, not this
        // crate's. Asserting the delegated output here is what keeps a second
        // canonicalizer from reappearing unnoticed.
        let value = json!({ "b": 1, "a": 2, "A": 3, "text": "x\ny" });
        assert_eq!(
            to_jcs_string(&value).expect("canonical"),
            "{\"A\":3,\"a\":2,\"b\":1,\"text\":\"x\\ny\"}"
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
    fn fractional_numbers_are_refused_before_hashing() {
        assert!(matches!(
            IntentHash::of(&json!({ "value": 1.5 })),
            Err(CanonicalError::NonIntegerNumber { .. })
        ));
        // `-0.0` survives `serde_json` as a float and must not silently become `0`.
        assert!(IntentHash::of(&json!({ "value": -0.0 })).is_err());
        assert!(reject_fractions(&json!({ "value": 0 })).is_ok());
        assert!(IntentHash::of(&json!({ "value": 0 })).is_ok());
    }

    #[test]
    fn a_fraction_nested_anywhere_is_still_refused() {
        assert!(IntentHash::of(&json!({ "a": [{ "b": [1, 2.5] }] })).is_err());
        assert!(IntentHash::of(&json!({ "a": [{ "b": [1, 2] }] })).is_ok());
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
