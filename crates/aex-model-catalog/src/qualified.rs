//! Pre-dispatch qualification: the proof that a `(provider, model)` pair exists
//! in a loaded catalog, and the typed reasons it does not.
//!
//! A [`QualifiedModel`] is the only way to reach a [`ModelEntry`] from outside
//! this crate, so an adapter cannot be handed a pair the catalog never resolved.

use std::sync::Arc;

use aex_wire::ErrorCode;
use aex_wire::provider::ProviderId;

use crate::document::{
    CapabilitySet, Dialect, DialectRevision, DisableReason, DurableOperationSupport, EndpointPin,
    EntryState, ModelEntry, ModelLimits, ReasoningReplay,
};
use crate::primitives::ModelSlug;
use crate::wire_pending::CatalogRevision;

/// A `(provider, model)` pair resolved against a loaded catalog revision.
///
/// Cheap to clone: one reference-count bump, no allocation. The entry it points
/// at is immutable for the life of the revision.
///
/// Equality is by *value* — the same pair resolved from two loads of the same
/// revision compares equal — not by pointer, so a test cannot pass merely
/// because two handles happen to share an allocation.
#[derive(Debug, Clone)]
pub struct QualifiedModel {
    entry: Arc<ModelEntry>,
    catalog: CatalogRevision,
}

impl PartialEq for QualifiedModel {
    fn eq(&self, other: &Self) -> bool {
        self.catalog == other.catalog && self.entry == other.entry
    }
}

impl Eq for QualifiedModel {}

impl QualifiedModel {
    /// Builds a qualified pair. Crate-private: only a loaded [`crate::Catalog`]
    /// can mint one.
    pub(crate) fn new(entry: Arc<ModelEntry>, catalog: CatalogRevision) -> Self {
        Self { entry, catalog }
    }

    /// The provider half of the pair.
    #[must_use]
    pub fn provider(&self) -> ProviderId {
        self.entry.provider
    }

    /// The exact provider-native model id.
    #[must_use]
    pub fn model(&self) -> &ModelSlug {
        &self.entry.model
    }

    /// The catalog revision that resolved the pair.
    #[must_use]
    pub fn catalog(&self) -> CatalogRevision {
        self.catalog
    }

    /// The full catalog entry.
    #[must_use]
    pub fn entry(&self) -> &ModelEntry {
        &self.entry
    }

    /// Which wire dialect the adapter must speak.
    #[must_use]
    pub fn dialect(&self) -> Dialect {
        self.entry.dialect
    }

    /// The exact dialect revision implemented by the running binary.
    #[must_use]
    pub fn dialect_revision(&self) -> DialectRevision {
        self.entry.dialect_revision
    }

    /// The compiled origin.
    #[must_use]
    pub fn endpoint(&self) -> EndpointPin {
        self.entry.endpoint
    }

    /// What the pair can do.
    #[must_use]
    pub fn capabilities(&self) -> CapabilitySet {
        self.entry.capabilities
    }

    /// The numeric bounds.
    #[must_use]
    pub fn limits(&self) -> &ModelLimits {
        &self.entry.limits
    }

    /// Whether the pair is admissible in principle.
    #[must_use]
    pub fn state(&self) -> EntryState {
        self.entry.state
    }

    /// Whether the provider offers a durable result lookup. `None` for all eight
    /// at launch.
    #[must_use]
    pub fn durable_operation(&self) -> DurableOperationSupport {
        self.entry.durable_operation
    }

    /// Whether a sealed assistant turn must carry reasoning round-trip material.
    #[must_use]
    pub fn requires_reasoning_token(&self, has_tool_use: bool) -> bool {
        match self.entry.reasoning.replay {
            ReasoningReplay::NotRequired | ReasoningReplay::RecommendedEcho => false,
            ReasoningReplay::RequiredWithToolCalls => has_tool_use,
            ReasoningReplay::RequiredAlways => true,
        }
    }
}

/// Why a `(provider, model)` pair did not qualify.
///
/// Every arm fails **before** dispatch, before any reservation, and carries the
/// public wire code it renders as.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CatalogError {
    /// The catalog carries no entry for this provider at all.
    #[error("the catalog carries no entry for provider `{provider}`")]
    UnknownProvider {
        /// The provider that was asked for.
        provider: ProviderId,
    },
    /// The provider is known; this model is not.
    #[error("provider `{provider}` has no model `{model}` in this catalog")]
    UnknownModel {
        /// The provider half of the pair.
        provider: ProviderId,
        /// The model half of the pair.
        model: ModelSlug,
    },
    /// The pair exists but is not admissible.
    #[error("the pair is `{state:?}`, not `Active`")]
    UnqualifiedPair {
        /// The state it is actually in.
        state: EntryState,
    },
    /// The pair is emergency-disabled in this revision.
    #[error("the pair is emergency-disabled: {reason:?}")]
    EmergencyDisabled {
        /// Why it was disabled.
        reason: DisableReason,
    },
}

impl CatalogError {
    /// The public wire code this failure renders as.
    #[must_use]
    pub const fn error_code(&self) -> ErrorCode {
        match self {
            Self::UnknownProvider { .. } => ErrorCode::UnknownProvider,
            Self::UnknownModel { .. } => ErrorCode::UnknownModel,
            Self::UnqualifiedPair { .. } | Self::EmergencyDisabled { .. } => {
                ErrorCode::UnqualifiedProviderModel
            }
        }
    }
}
