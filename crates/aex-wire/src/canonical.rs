//! The one canonicalization rule in the workspace: RFC 8785 JCS over UTF-8 byte
//! ordering.
//!
//! Every digest, idempotency intent, signing input and golden byte string in
//! AEX is taken over the output of [`to_jcs_bytes`]. No stream may introduce a
//! second canonicalizer or a second comparator, because two canonicalizers mean
//! two answers to "is this the same request".
//!
//! # Deliberate deviations from RFC 8785, each fail-fast
//!
//! - Object members are ordered by their **UTF-8** bytes rather than by UTF-16
//!   code units. The two orders differ only for astral-plane keys, and the
//!   workspace pinned UTF-8 ordering so that a byte comparison of the
//!   serialized form and a comparison of the parsed keys can never disagree.
//! - A number whose magnitude reaches `1e21` is **rejected**, because that is
//!   exactly where `ECMAScript`'s `Number::toString` switches to exponential
//!   notation and a Rust shortest-round-trip printer does not.
//! - A non-finite number cannot exist in `serde_json`, but an integral float is
//!   normalized to an integer so `1.0` and `1` canonicalize identically.

use serde::Serialize;
use serde_json::Value;

use crate::idempotency::IntentDigest;
use crate::routes::{PathBinding, RouteId};
use crate::types::JsonPointer;

/// A value that cannot be canonicalized.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CanonicalError {
    /// The value could not be represented as JSON at all.
    #[error("value is not representable as JSON: {reason}")]
    NotJson {
        /// The `serde_json` failure, rendered.
        reason: String,
    },
    /// A number was outside the range both `ECMAScript` and Rust print identically.
    #[error("number at `{pointer}` is outside the canonical range (|value| must be < 1e21)")]
    NumberOutOfRange {
        /// Where the offending number sits in the document.
        pointer: JsonPointer,
    },
    /// The input text was not valid JSON.
    #[error("input is not valid JSON: {reason}")]
    Malformed {
        /// The parser failure, rendered.
        reason: String,
    },
}

/// The magnitude at which `ECMAScript` switches to exponential notation.
const EXPONENTIAL_THRESHOLD: f64 = 1e21;

/// Serializes `value` to canonical JCS bytes.
///
/// # Errors
///
/// Returns [`CanonicalError`] when the value is not representable as JSON or
/// contains a number outside the canonical range.
pub fn to_jcs_bytes<T: Serialize>(value: &T) -> Result<Vec<u8>, CanonicalError> {
    let mut json = serde_json::to_value(value).map_err(|error| CanonicalError::NotJson {
        reason: error.to_string(),
    })?;
    normalize(&mut json, &JsonPointer::root())?;
    serde_json::to_vec(&json).map_err(|error| CanonicalError::NotJson {
        reason: error.to_string(),
    })
}

/// Serializes `value` to a canonical JCS string.
///
/// # Errors
///
/// As [`to_jcs_bytes`]; the bytes are always valid UTF-8.
pub fn to_jcs_string<T: Serialize>(value: &T) -> Result<String, CanonicalError> {
    let bytes = to_jcs_bytes(value)?;
    String::from_utf8(bytes).map_err(|error| CanonicalError::NotJson {
        reason: error.to_string(),
    })
}

/// Rewrites integral floats as integers, rejects out-of-range numbers, and puts
/// every object's members into UTF-8 byte order.
///
/// The ordering pass is **not** redundant. `serde_json::Map` is a `BTreeMap`
/// only while the `preserve_order` feature is off, and that feature is not ours
/// to control: `aws-smithy-http-client` enables it, so every binary linking any
/// AWS SDK crate gets an insertion-ordered `IndexMap` instead. Relying on the
/// map type made canonical bytes depend on which crates happened to share a
/// build, which silently changes every digest, idempotency key and signature.
/// Sorting here is correct under both map types.
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss,
    clippy::float_cmp,
    reason = "detecting an integral float is exactly an exact-bit-pattern question"
)]
fn normalize(value: &mut Value, pointer: &JsonPointer) -> Result<(), CanonicalError> {
    match value {
        Value::Number(number) => {
            let Some(as_f64) = number.as_f64() else {
                return Ok(());
            };
            if as_f64.abs() >= EXPONENTIAL_THRESHOLD {
                return Err(CanonicalError::NumberOutOfRange {
                    pointer: pointer.clone(),
                });
            }
            if number.is_f64() && as_f64.fract() == 0.0 {
                let rounded = as_f64 as i64;
                if (f64::from(i32::MIN)..=f64::from(i32::MAX)).contains(&as_f64)
                    || as_f64 == rounded as f64
                {
                    *value = Value::Number(rounded.into());
                }
            }
            Ok(())
        }
        Value::Array(items) => {
            for (index, item) in items.iter_mut().enumerate() {
                normalize(item, &pointer.index(index))?;
            }
            Ok(())
        }
        Value::Object(members) => {
            for (key, member) in members.iter_mut() {
                normalize(member, &pointer.child(key))?;
            }
            // Rebuild in UTF-8 byte order. Under `BTreeMap` this is already the
            // order and the rebuild is a no-op; under `IndexMap` the insertion
            // order becomes the emitted order, which is what makes the two
            // builds agree.
            let mut sorted: Vec<(String, Value)> = core::mem::take(members).into_iter().collect();
            sorted.sort_by(|(left, _), (right, _)| left.as_bytes().cmp(right.as_bytes()));
            for (key, member) in sorted {
                members.insert(key, member);
            }
            Ok(())
        }
        Value::Null | Value::Bool(_) | Value::String(_) => Ok(()),
    }
}

/// The canonical intent of one request.
///
/// `sha256(route_id ‖ 0x1f ‖ path bindings ‖ 0x1f ‖ jcs(body))`. The route id is
/// the `operationId`, not the method and path, so a template rename cannot
/// silently change an existing idempotency identity; the separator is a byte no
/// `operationId`, parameter name or JCS document can contain, so two different
/// requests cannot collide by concatenation.
#[must_use]
pub fn intent_digest(route: RouteId, path: &PathBinding<'_>, body: Option<&[u8]>) -> IntentDigest {
    use sha2::Digest as _;
    let mut hasher = sha2::Sha256::new();
    hasher.update(route.as_str().as_bytes());
    hasher.update([0x1f]);
    for (name, value) in path.as_slice() {
        hasher.update(name.as_bytes());
        hasher.update([0x1e]);
        hasher.update(value.as_bytes());
        hasher.update([0x1e]);
    }
    hasher.update([0x1f]);
    hasher.update(body.unwrap_or_default());
    IntentDigest::from_bytes(hasher.finalize().into())
}

/// A JSON document already in canonical form.
///
/// The contract uses it where a payload is opaque to AEX but must still hash
/// deterministically: registered tool arguments, a normalized export query, a
/// tool input schema. The value is canonical by construction, so a caller can
/// hash [`CanonicalJson::as_str`] without re-canonicalizing.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct CanonicalJson(Box<str>);

impl CanonicalJson {
    /// Parses arbitrary JSON text and canonicalizes it.
    ///
    /// # Errors
    ///
    /// Returns [`CanonicalError::Malformed`] when the text is not JSON, and
    /// [`CanonicalError::NumberOutOfRange`] when it contains an unprintable
    /// number.
    pub fn parse(text: &str) -> Result<Self, CanonicalError> {
        let value: Value =
            serde_json::from_str(text).map_err(|error| CanonicalError::Malformed {
                reason: error.to_string(),
            })?;
        Self::from_value(&value)
    }

    /// Canonicalizes an already-parsed value.
    ///
    /// # Errors
    ///
    /// As [`to_jcs_string`].
    pub fn from_value(value: &Value) -> Result<Self, CanonicalError> {
        Ok(Self(to_jcs_string(value)?.into_boxed_str()))
    }

    /// The canonical text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The canonical bytes.
    #[must_use]
    pub fn as_bytes(&self) -> &[u8] {
        self.0.as_bytes()
    }

    /// Re-parses the canonical text.
    ///
    /// # Panics
    ///
    /// Never: the stored text was produced by `serde_json` and is always valid.
    #[must_use]
    pub fn to_value(&self) -> Value {
        serde_json::from_str(&self.0).expect("canonical text is always valid JSON")
    }
}

impl std::fmt::Display for CanonicalJson {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl serde::Serialize for CanonicalJson {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.to_value().serialize(serializer)
    }
}

impl<'de> serde::Deserialize<'de> for CanonicalJson {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        use serde::de::Error as _;
        let value = Value::deserialize(deserializer)?;
        Self::from_value(&value).map_err(D::Error::custom)
    }
}
