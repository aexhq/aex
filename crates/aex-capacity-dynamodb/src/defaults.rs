//! The one canonical shared-safety default document.

use std::collections::{BTreeMap, BTreeSet};

use aex_wire::limits::{LimitId, LimitShape};
use aex_wire::models::{LimitMapValue, LimitScalarValue, LimitValue};
use aex_wire::types::DecimalU128;
use serde::Deserialize;
use sha2::{Digest as _, Sha256};

const DOCUMENT: &str = include_str!("../../../api/schemas/registries/limit-defaults.v1.json");
const SCHEMA: &str = "aex.capacity-defaults.v1";

/// The validated default set embedded in the controller artifact.
#[derive(Clone, Debug, PartialEq)]
pub struct CapacityDefaults {
    /// Monotonic authored document revision.
    pub revision: u64,
    /// Digest of the exact authored bytes.
    pub digest: String,
    /// One complete value for every registered shared-safety limit.
    pub values: BTreeMap<LimitId, LimitValue>,
}

impl CapacityDefaults {
    /// Returns one default, which is always present after validation.
    #[must_use]
    pub fn value(&self, id: LimitId) -> &LimitValue {
        self.values
            .get(&id)
            .expect("validated defaults contain every registered limit")
    }
}

/// Why the authored default document was refused.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum DefaultsError {
    /// The document is not valid JSON.
    #[error("capacity defaults are not valid JSON: {0}")]
    Json(String),
    /// The document uses another schema.
    #[error("capacity defaults use unsupported schema `{0}`")]
    Schema(String),
    /// The authored revision must begin above zero.
    #[error("capacity defaults revision must be positive")]
    Revision,
    /// An identifier is unknown, duplicated, absent or out of registry order.
    #[error("capacity defaults registry drift: {0}")]
    Registry(String),
    /// A value disagrees with its registered shape.
    #[error("capacity default `{id}` is invalid: {reason}")]
    Value {
        /// Registered limit spelling.
        id: String,
        /// Exact validation failure.
        reason: String,
    },
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Document {
    schema: String,
    revision: u64,
    limits: Vec<Row>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Row {
    id: String,
    value: serde_json::Value,
}

/// Parses and validates the one embedded document.
///
/// # Errors
///
/// Returns [`DefaultsError`] for any schema, registry, shape, zero-value or
/// duplicate-dimension drift. A controller with an invalid artifact must not
/// start and must never infer a replacement value.
pub fn canonical_defaults() -> Result<CapacityDefaults, DefaultsError> {
    parse(DOCUMENT)
}

fn parse(input: &str) -> Result<CapacityDefaults, DefaultsError> {
    let document: Document =
        serde_json::from_str(input).map_err(|error| DefaultsError::Json(error.to_string()))?;
    if document.schema != SCHEMA {
        return Err(DefaultsError::Schema(document.schema));
    }
    if document.revision == 0 {
        return Err(DefaultsError::Revision);
    }
    if document.limits.len() != LimitId::ALL.len() {
        return Err(DefaultsError::Registry(format!(
            "expected {} rows, found {}",
            LimitId::ALL.len(),
            document.limits.len()
        )));
    }

    let mut values = BTreeMap::new();
    let mut seen = BTreeSet::new();
    for (expected, row) in LimitId::ALL.iter().copied().zip(document.limits) {
        if row.id != expected.as_str() {
            return Err(DefaultsError::Registry(format!(
                "expected `{}`, found `{}`",
                expected.as_str(),
                row.id
            )));
        }
        if !seen.insert(expected) {
            return Err(DefaultsError::Registry(format!(
                "duplicate `{}`",
                expected.as_str()
            )));
        }
        values.insert(expected, value(expected, row.value)?);
    }

    Ok(CapacityDefaults {
        revision: document.revision,
        digest: format!("sha256:{}", hex::encode(Sha256::digest(input.as_bytes()))),
        values,
    })
}

fn value(id: LimitId, raw: serde_json::Value) -> Result<LimitValue, DefaultsError> {
    match id.shape() {
        LimitShape::Scalar => {
            let number = positive(id, &raw)?;
            Ok(LimitValue::Scalar(LimitScalarValue {
                value: DecimalU128::new(u128::from(number)),
            }))
        }
        LimitShape::Map => {
            let object = raw
                .as_object()
                .ok_or_else(|| invalid(id, "expected an object"))?;
            if object.is_empty() {
                return Err(invalid(id, "the dimension map is empty"));
            }
            let mut dimensions = BTreeMap::new();
            for (name, raw_value) in object {
                if name.is_empty() {
                    return Err(invalid(id, "a dimension name is empty"));
                }
                dimensions.insert(
                    name.clone(),
                    DecimalU128::new(u128::from(positive(id, raw_value)?)),
                );
            }
            Ok(LimitValue::Map(LimitMapValue { values: dimensions }))
        }
    }
}

fn positive(id: LimitId, raw: &serde_json::Value) -> Result<u64, DefaultsError> {
    raw.as_u64()
        .filter(|value| *value > 0)
        .ok_or_else(|| invalid(id, "expected a positive integer"))
}

fn invalid(id: LimitId, reason: &str) -> DefaultsError {
    DefaultsError::Value {
        id: id.as_str().to_owned(),
        reason: reason.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use aex_wire::limits::LimitId;

    use super::{DefaultsError, canonical_defaults, parse};

    #[test]
    fn canonical_defaults_are_complete_and_digest_bound() {
        let defaults = canonical_defaults().expect("canonical defaults validate");
        assert_eq!(defaults.revision, 1);
        assert_eq!(defaults.values.len(), LimitId::ALL.len());
        assert_eq!(defaults.digest.len(), "sha256:".len() + 64);
    }

    #[test]
    fn missing_unknown_reordered_and_zero_values_fail_closed() {
        let canonical = include_str!("../../../api/schemas/registries/limit-defaults.v1.json");
        for broken in [
            canonical.replacen("\"revision\": 1", "\"revision\": 0", 1),
            canonical.replacen("context.tool_result_bytes", "unknown.tool_result_bytes", 1),
            canonical.replacen("\"value\": 65536", "\"value\": 0", 1),
        ] {
            assert!(parse(&broken).is_err(), "broken defaults were accepted");
        }
        assert!(matches!(parse("{}"), Err(DefaultsError::Json(_))));
    }
}
