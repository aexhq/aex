//! Authorization scopes.
//!
//! [`ScopeId`] is generated from the registry; [`ScopeSet`] is the ordered,
//! duplicate-free set a credential actually carries. The set is a `Vec` rather
//! than a `HashSet` on purpose: it is small, it must render deterministically,
//! and a linear scan over at most a few dozen entries beats a hash.

use std::fmt;

pub use crate::generated::scopes::ScopeId;

/// The scopes one credential carries, sorted and duplicate-free.
#[derive(
    Debug,
    Clone,
    Default,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    serde::Serialize,
    serde::Deserialize,
)]
#[serde(transparent)]
pub struct ScopeSet(Vec<ScopeId>);

impl ScopeSet {
    /// An empty set.
    #[must_use]
    pub const fn empty() -> Self {
        Self(Vec::new())
    }

    /// Builds a set, sorting and deduplicating.
    #[must_use]
    pub fn new(scopes: impl IntoIterator<Item = ScopeId>) -> Self {
        let mut all: Vec<ScopeId> = scopes.into_iter().collect();
        all.sort_unstable();
        all.dedup();
        Self(all)
    }

    /// Every scope in the credential, ascending.
    #[must_use]
    pub fn as_slice(&self) -> &[ScopeId] {
        &self.0
    }

    /// Whether the credential carries `scope`.
    #[must_use]
    pub fn contains(&self, scope: ScopeId) -> bool {
        self.0.binary_search(&scope).is_ok()
    }

    /// Whether the credential carries every scope in `required`.
    #[must_use]
    pub fn contains_all(&self, required: &Self) -> bool {
        required.0.iter().all(|scope| self.contains(*scope))
    }

    /// The first scope in `required` the credential lacks.
    ///
    /// Returning the offending scope rather than a boolean is what lets the
    /// `insufficient_scope` envelope name exactly what to add.
    #[must_use]
    pub fn missing(&self, required: ScopeId) -> Option<ScopeId> {
        (!self.contains(required)).then_some(required)
    }

    /// How many scopes the credential carries.
    #[must_use]
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// Whether the credential carries no scopes at all.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl FromIterator<ScopeId> for ScopeSet {
    fn from_iter<I: IntoIterator<Item = ScopeId>>(iter: I) -> Self {
        Self::new(iter)
    }
}

impl fmt::Display for ScopeId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}
