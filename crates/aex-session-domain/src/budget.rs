//! Effective limits and the spend grant a run carries.
//!
//! No ceiling is compiled in (D-21). Every value arrives as an effective limit
//! resolved for the workspace, and a missing row is a **typed failure**, not a
//! default: silently substituting a built-in number is how a durable override
//! stops being durable.

use std::collections::BTreeMap;
use std::num::NonZeroU64;

use aex_wire::limits::LimitId;

/// The limits in force for one workspace.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EffectiveLimits(BTreeMap<LimitId, u64>);

impl EffectiveLimits {
    /// An empty set. Every lookup against it fails, which is the correct
    /// behaviour for a workspace whose limits have not been resolved.
    #[must_use]
    pub fn new() -> Self {
        Self(BTreeMap::new())
    }

    /// Records one resolved limit.
    pub fn insert(&mut self, id: LimitId, value: u64) -> Option<u64> {
        self.0.insert(id, value)
    }

    /// The resolved value of one limit.
    ///
    /// # Errors
    ///
    /// Returns [`LimitUnresolved`] when the row is absent.
    pub fn require(&self, id: LimitId) -> Result<u64, LimitUnresolved> {
        self.0.copied_value(id)
    }

    /// Whether any limit has been resolved.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// How many limits are resolved.
    #[must_use]
    pub fn len(&self) -> usize {
        self.0.len()
    }
}

impl FromIterator<(LimitId, u64)> for EffectiveLimits {
    fn from_iter<I: IntoIterator<Item = (LimitId, u64)>>(iter: I) -> Self {
        Self(iter.into_iter().collect())
    }
}

trait CopiedValue {
    fn copied_value(&self, id: LimitId) -> Result<u64, LimitUnresolved>;
}

impl CopiedValue for BTreeMap<LimitId, u64> {
    fn copied_value(&self, id: LimitId) -> Result<u64, LimitUnresolved> {
        self.get(&id).copied().ok_or(LimitUnresolved { limit: id })
    }
}

/// A limit the workspace has no resolved row for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("limit `{limit}` is not resolved for this workspace")]
pub struct LimitUnresolved {
    /// Which limit.
    pub limit: LimitId,
}

/// The spend a run is allowed to consume.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BudgetGrant {
    /// The ceiling, in cents.
    pub max_spend_cents: NonZeroU64,
}

#[cfg(test)]
mod tests {
    use aex_wire::limits::LimitId;

    use super::{EffectiveLimits, LimitUnresolved};

    #[test]
    fn an_unresolved_limit_is_a_typed_failure_not_a_default() {
        let limits = EffectiveLimits::new();
        assert!(limits.is_empty());
        assert_eq!(
            limits.require(LimitId::SessionMaterializedAgents),
            Err(LimitUnresolved {
                limit: LimitId::SessionMaterializedAgents
            })
        );

        let resolved: EffectiveLimits = [(LimitId::SessionMaterializedAgents, 256)]
            .into_iter()
            .collect();
        assert_eq!(resolved.len(), 1);
        assert_eq!(
            resolved.require(LimitId::SessionMaterializedAgents),
            Ok(256)
        );
    }
}
