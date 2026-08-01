//! The direct BYOK provider vocabulary.
//!
//! Six providers, no gateway, no arbitrary base URL and no cross-provider
//! fallback. Qualification — whether a `(provider, model)` pair has a live
//! conformance receipt — is `aex-model-catalog`'s, not this crate's.

use std::fmt;

pub use crate::generated::models::{ModelSelection, ProviderId};

impl fmt::Display for ProviderId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl ProviderId {
    /// Resolves a wire spelling.
    ///
    /// There is no alias table: `anthrophic` is a decode error, not a synonym.
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        Self::ALL
            .iter()
            .copied()
            .find(|provider| provider.as_str() == text)
    }
}
