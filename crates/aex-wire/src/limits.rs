//! Effective workspace safety limits.
//!
//! This crate owns the identity and the public shape. Enforcement, defaults and
//! the audited override path belong to the regional capacity authority, and
//! there is deliberately no public mutation route.

use std::fmt;

pub use crate::generated::limits::{LimitId, LimitShape};
pub use crate::generated::models::{
    EffectiveWorkspaceLimit, EffectiveWorkspaceLimitPage, LimitMapValue, LimitScalarValue,
    LimitSource, LimitValue,
};

impl fmt::Display for LimitId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl LimitValue {
    /// The scalar value, when the limit has one.
    #[must_use]
    pub const fn scalar(&self) -> Option<&crate::types::DecimalU128> {
        match self {
            Self::Scalar(value) => Some(&value.value),
            Self::Map(_) => None,
        }
    }

    /// The value of one named dimension, when the limit is a map.
    #[must_use]
    pub fn dimension(&self, name: &str) -> Option<crate::types::DecimalU128> {
        match self {
            Self::Scalar(_) => None,
            Self::Map(value) => value.values.get(name).copied(),
        }
    }

    /// The shape this value actually has.
    #[must_use]
    pub const fn shape(&self) -> LimitShape {
        match self {
            Self::Scalar(_) => LimitShape::Scalar,
            Self::Map(_) => LimitShape::Map,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use serde_json::Value;

    use super::{LimitId, LimitShape};

    #[test]
    fn the_canonical_default_document_is_complete_and_shape_correct() {
        let document: Value = serde_json::from_str(include_str!(
            "../../../api/schemas/registries/limit-defaults.v1.json"
        ))
        .expect("the capacity default document is JSON");
        assert_eq!(
            document["schema"].as_str(),
            Some("aex.capacity-defaults.v1")
        );
        assert_eq!(document["revision"].as_u64(), Some(1));
        let rows = document["limits"]
            .as_array()
            .expect("the capacity default document carries limits");
        assert_eq!(rows.len(), LimitId::ALL.len());

        let mut seen = BTreeSet::new();
        for (expected, row) in LimitId::ALL.iter().copied().zip(rows) {
            let id = row["id"].as_str().expect("a default row carries an id");
            assert_eq!(id, expected.as_str(), "default order drifted");
            assert!(seen.insert(id), "duplicate default `{id}`");
            match expected.shape() {
                LimitShape::Scalar => {
                    assert!(
                        row["value"].as_u64().is_some_and(|value| value > 0),
                        "scalar default `{id}` is not a positive integer"
                    );
                }
                LimitShape::Map => {
                    let dimensions = row["value"]
                        .as_object()
                        .unwrap_or_else(|| panic!("map default `{id}` is not an object"));
                    assert!(!dimensions.is_empty(), "map default `{id}` is empty");
                    for (dimension, value) in dimensions {
                        assert!(!dimension.is_empty(), "`{id}` has an empty dimension");
                        assert!(
                            value.as_u64().is_some_and(|number| number > 0),
                            "`{id}.{dimension}` is not a positive integer"
                        );
                    }
                }
            }
        }
    }
}
