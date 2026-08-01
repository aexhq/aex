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
