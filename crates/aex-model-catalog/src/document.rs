//! The compiled per-model facts admission exposes: numeric limits and the
//! two-bit tool capability set.
//!
//! Everything else the old catalog document carried — dialect revisions,
//! endpoints, reasoning policy, cache policy, error maps, staged/deprecated
//! states — died with the signed catalog authority (model-provider
//! simplification 2026-08-13). The generated admit table is the single source
//! of model truth; dialect-class policy lives on
//! [`aex_model_vocabulary::DialectClass`].

use serde::{Deserialize, Serialize};

/// One declared model capability. Two bits only.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Capability {
    /// The model may call tools.
    Tools,
    /// The model may emit parallel tool calls.
    ParallelTools,
}

impl Capability {
    /// Every capability, for exhaustive tests.
    pub const ALL: [Self; 2] = [Self::Tools, Self::ParallelTools];
}

/// A bit set over [`Capability`].
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default, Serialize, Deserialize,
)]
#[serde(transparent)]
pub struct CapabilitySet(pub u8);

impl CapabilitySet {
    /// The empty set.
    pub const EMPTY: Self = Self(0);

    /// Builds a set from a slice of capabilities.
    #[must_use]
    pub const fn from_slice(capabilities: &[Capability]) -> Self {
        let mut bits = 0u8;
        let mut index = 0;
        while index < capabilities.len() {
            bits |= capabilities[index].bit();
            index += 1;
        }
        Self(bits)
    }

    /// Whether the set contains a capability.
    #[must_use]
    pub const fn has(self, capability: Capability) -> bool {
        self.0 & capability.bit() != 0
    }

    /// The set with one capability added.
    #[must_use]
    pub const fn with(self, capability: Capability) -> Self {
        Self(self.0 | capability.bit())
    }
}

impl Capability {
    /// The bit this capability occupies.
    const fn bit(self) -> u8 {
        match self {
            Self::Tools => 1,
            Self::ParallelTools => 2,
        }
    }
}

/// The numeric bounds a model row declares.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModelLimits {
    /// The provider context window, in tokens.
    pub context_window_tokens: u32,
    /// The provider output ceiling, in tokens.
    pub max_output_tokens: u32,
}
