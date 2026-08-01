//! The canonical observation value model and the batch intent digest.
//!
//! There is exactly one canonicalization rule workspace-wide: RFC 8785 JCS over
//! UTF-8 byte ordering. This module models values in a shape that has no
//! ambiguity to canonicalize away (maps are ordered, floats are finite, no
//! duplicate keys are representable) and then defers to
//! [`aex_wire::canonical::to_jcs_bytes`]. No second canonicalizer is introduced.

use std::collections::BTreeMap;

use aex_wire::canonical::{CanonicalError, to_jcs_bytes};
use aex_wire::idempotency::IntentDigest;
use serde::{Serialize, Serializer};
use sha2::{Digest as _, Sha256};

/// A value that can appear inside a normalized observation.
///
/// Deliberately not `serde_json::Value`: a `Map` here is a `BTreeMap`, so two
/// values that differ only in insertion order are the *same* value rather than
/// two values that happen to canonicalize alike, and a `Num` is checked finite
/// at construction rather than at serialization.
#[derive(Clone, Debug, PartialEq)]
pub enum CanonicalValue {
    /// The absent value.
    Null,
    /// A boolean.
    Bool(bool),
    /// A signed integer.
    Int(i64),
    /// A finite double.
    Num(f64),
    /// A UTF-8 string.
    Str(Box<str>),
    /// An ordered sequence.
    Array(Vec<CanonicalValue>),
    /// A key-ordered map.
    Map(BTreeMap<String, CanonicalValue>),
}

impl CanonicalValue {
    /// Builds a number, refusing a non-finite double.
    ///
    /// # Errors
    ///
    /// Returns [`CanonicalError::NotJson`] for `NaN` and both infinities, which
    /// have no JSON spelling and would otherwise become `null` silently.
    pub fn number(value: f64) -> Result<Self, CanonicalError> {
        if value.is_finite() {
            Ok(Self::Num(value))
        } else {
            Err(CanonicalError::NotJson {
                reason: "a non-finite number has no canonical JSON spelling".to_owned(),
            })
        }
    }

    /// A shallow type name, used in error messages and field-policy diagnostics.
    #[must_use]
    pub const fn kind(&self) -> &'static str {
        match self {
            Self::Null => "null",
            Self::Bool(_) => "boolean",
            Self::Int(_) => "integer",
            Self::Num(_) => "number",
            Self::Str(_) => "string",
            Self::Array(_) => "array",
            Self::Map(_) => "map",
        }
    }

    /// Whether this value is a scalar that a public filter may compare against.
    #[must_use]
    pub const fn is_external_scalar(&self) -> bool {
        matches!(
            self,
            Self::Null | Self::Bool(_) | Self::Int(_) | Self::Num(_) | Self::Str(_)
        )
    }

    /// The string content, when the value is a string.
    #[must_use]
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Self::Str(text) => Some(text),
            _ => None,
        }
    }

    /// The canonical byte length of this value, used for the per-observation
    /// size ceiling and for the logical-byte usage fact.
    ///
    /// # Errors
    ///
    /// Returns [`CanonicalError`] when the value cannot be canonicalized.
    pub fn canonical_len(&self) -> Result<usize, CanonicalError> {
        canonical_bytes(self).map(|bytes| bytes.len())
    }
}

impl Serialize for CanonicalValue {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::Null => serializer.serialize_unit(),
            Self::Bool(value) => serializer.serialize_bool(*value),
            Self::Int(value) => serializer.serialize_i64(*value),
            Self::Num(value) => serializer.serialize_f64(*value),
            Self::Str(value) => serializer.serialize_str(value),
            Self::Array(values) => values.serialize(serializer),
            Self::Map(values) => values.serialize(serializer),
        }
    }
}

/// Canonicalizes a value to RFC 8785 JCS bytes.
///
/// # Errors
///
/// Returns [`CanonicalError`] when the value is not representable as canonical
/// JSON.
pub fn canonical_bytes(value: &CanonicalValue) -> Result<Vec<u8>, CanonicalError> {
    to_jcs_bytes(value)
}

/// The non-payload half of a batch's identity.
///
/// Carried forward verbatim from the implementation being replaced, because it
/// is a working oracle: an admitted batch is bound to who asked, how, where and
/// for which scope, not merely to its bytes.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BatchBinding<'a> {
    /// The authenticated principal.
    pub principal_id: &'a str,
    /// The HTTP method.
    pub method: &'a str,
    /// The canonical (un-substituted) route template.
    pub canonical_route: &'a str,
    /// The owning workspace.
    pub workspace_id: &'a str,
    /// The rendered scope key.
    pub scope: &'a str,
}

/// The digest that binds an admitted batch to exactly what was asked for.
///
/// The observation list is ordered: reordering the same observations is a
/// different batch, because the accepted sequence it claims would differ.
///
/// # Errors
///
/// Returns [`CanonicalError`] when any observation cannot be canonicalized.
pub fn batch_intent_digest(
    binding: &BatchBinding<'_>,
    observations: &[CanonicalValue],
) -> Result<IntentDigest, CanonicalError> {
    let mut hasher = Sha256::new();
    for field in [
        binding.principal_id,
        binding.method,
        binding.canonical_route,
        binding.workspace_id,
        binding.scope,
    ] {
        // Length-prefix every field so no concatenation of two fields can equal
        // another pair.
        hasher.update(u64::try_from(field.len()).unwrap_or(u64::MAX).to_be_bytes());
        hasher.update(field.as_bytes());
    }
    hasher.update(
        u64::try_from(observations.len())
            .unwrap_or(u64::MAX)
            .to_be_bytes(),
    );
    for observation in observations {
        let bytes = canonical_bytes(observation)?;
        hasher.update(u64::try_from(bytes.len()).unwrap_or(u64::MAX).to_be_bytes());
        hasher.update(&bytes);
    }
    Ok(IntentDigest::from_bytes(hasher.finalize().into()))
}

/// The SHA-256 of a canonical body, rendered lowercase hex.
///
/// # Errors
///
/// Returns [`CanonicalError`] when the value cannot be canonicalized.
pub fn body_sha256_hex(value: &CanonicalValue) -> Result<String, CanonicalError> {
    let bytes = canonical_bytes(value)?;
    Ok(sha256_hex(&bytes))
}

/// The SHA-256 of raw bytes, rendered lowercase hex.
#[must_use]
pub fn sha256_hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    let digest = Sha256::digest(bytes);
    let mut out = String::with_capacity(64);
    for byte in digest {
        let _ = write!(out, "{byte:02x}");
    }
    out
}

/// A stable digest over a normalized attribute map.
///
/// Used as `attrDigest` on every observation item and as one input to the series
/// hash, so two observations with identical attributes share one digest without
/// the index having to project the attributes themselves.
///
/// # Errors
///
/// Returns [`CanonicalError`] when an attribute value cannot be canonicalized.
pub fn attribute_digest(
    attributes: &BTreeMap<String, CanonicalValue>,
) -> Result<String, CanonicalError> {
    let value = CanonicalValue::Map(attributes.clone());
    body_sha256_hex(&value)
}

#[cfg(test)]
mod tests {
    use super::{CanonicalValue, attribute_digest, canonical_bytes, sha256_hex};
    use std::collections::BTreeMap;

    #[test]
    fn a_non_finite_number_is_refused_rather_than_silently_nulled() {
        assert!(CanonicalValue::number(f64::NAN).is_err());
        assert!(CanonicalValue::number(f64::INFINITY).is_err());
        assert!(CanonicalValue::number(1.5).is_ok());
    }

    #[test]
    fn scalars_and_containers_are_distinguished_for_the_field_policy() {
        assert!(CanonicalValue::Str("x".into()).is_external_scalar());
        assert!(CanonicalValue::Null.is_external_scalar());
        assert!(!CanonicalValue::Array(vec![]).is_external_scalar());
        assert!(!CanonicalValue::Map(BTreeMap::new()).is_external_scalar());
        assert_eq!(CanonicalValue::Int(1).kind(), "integer");
    }

    #[test]
    fn the_empty_digest_is_stable() {
        assert_eq!(
            sha256_hex(&[]),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }

    #[test]
    fn an_attribute_digest_ignores_insertion_order() {
        let mut forward = BTreeMap::new();
        forward.insert("b".to_owned(), CanonicalValue::Int(1));
        forward.insert("a".to_owned(), CanonicalValue::Int(2));
        let mut reverse = BTreeMap::new();
        reverse.insert("a".to_owned(), CanonicalValue::Int(2));
        reverse.insert("b".to_owned(), CanonicalValue::Int(1));
        assert_eq!(
            attribute_digest(&forward).expect("digests"),
            attribute_digest(&reverse).expect("digests")
        );
    }

    #[test]
    fn canonical_length_matches_the_encoded_bytes() {
        let value = CanonicalValue::Str("hello".into());
        assert_eq!(
            value.canonical_len().expect("length"),
            canonical_bytes(&value).expect("bytes").len()
        );
    }
}
