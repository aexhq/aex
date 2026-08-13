//! Pre-dispatch qualification: the proof that a `(provider, model)` pair exists
//! in the compiled admit table, and the typed reasons it does not.
//!
//! A [`QualifiedModel`] is the only way to reach an [`AdmittedModel`] from
//! outside this crate, so an adapter cannot be handed a pair the compiled
//! table never resolved.

use aex_wire::ErrorCode;
use aex_wire::provider::ProviderId;

use crate::document::{CapabilitySet, ModelLimits, StructuredOutputLevel};
use crate::generated::{self, AdmittedModel, MODELS};
use crate::primitives::{Blake3Digest, ModelSlug};
use crate::wire_pending::CatalogRevision;

/// A `(provider, model)` pair resolved against the compiled admit table.
///
/// Cheap to clone: the entry is a `'static` reference and the slug is shared.
/// Equality is by *value* — the same pair resolved twice compares equal.
#[derive(Debug, Clone)]
pub struct QualifiedModel {
    entry: &'static AdmittedModel,
    model: ModelSlug,
    catalog: CatalogRevision,
}

impl PartialEq for QualifiedModel {
    fn eq(&self, other: &Self) -> bool {
        self.catalog == other.catalog && self.model == other.model
    }
}

impl Eq for QualifiedModel {}

impl QualifiedModel {
    /// Builds a qualified pair from an admitted row. Only [`admit`] and the
    /// fixture surface may mint one.
    #[doc(hidden)]
    #[must_use]
    pub const fn new(
        entry: &'static AdmittedModel,
        model: ModelSlug,
        catalog: CatalogRevision,
    ) -> Self {
        Self {
            entry,
            model,
            catalog,
        }
    }

    /// The provider half of the pair.
    #[must_use]
    pub const fn provider(&self) -> ProviderId {
        self.entry.provider
    }

    /// The exact provider-native model id.
    #[must_use]
    pub const fn model(&self) -> &ModelSlug {
        &self.model
    }

    /// The catalog revision that resolved the pair.
    #[must_use]
    pub const fn catalog(&self) -> CatalogRevision {
        self.catalog
    }

    /// The compiled admit-table row.
    #[must_use]
    pub const fn entry(&self) -> &'static AdmittedModel {
        self.entry
    }

    /// Which dialect class the provider adapter must speak.
    #[must_use]
    pub const fn dialect(&self) -> aex_model_vocabulary::DialectClass {
        self.entry.dialect
    }

    /// What the pair can do.
    #[must_use]
    pub fn capabilities(&self) -> CapabilitySet {
        let mut set = CapabilitySet::EMPTY;
        if self.entry.tools {
            set = set.with(crate::document::Capability::Tools);
        }
        if self.entry.parallel_tools {
            set = set.with(crate::document::Capability::ParallelTools);
        }
        if self.entry.structured_output != StructuredOutputLevel::None {
            set = set.with(crate::document::Capability::StructuredOutput);
        }
        set
    }

    /// The strongest native structured-output contract this exact route has
    /// passed.
    #[must_use]
    pub const fn structured_output(&self) -> StructuredOutputLevel {
        self.entry.structured_output
    }

    /// The numeric bounds.
    #[must_use]
    pub const fn limits(&self) -> ModelLimits {
        ModelLimits {
            context_window_tokens: self.entry.context_window_tokens,
            max_output_tokens: self.entry.max_output_tokens,
        }
    }

    /// Whether a sealed assistant turn must carry reasoning round-trip material.
    #[must_use]
    pub const fn requires_reasoning_token(&self, has_tool_use: bool) -> bool {
        self.entry.dialect.requires_reasoning_token(has_tool_use)
    }

    /// The compiled origin for this provider family.
    ///
    /// # Panics
    ///
    /// Never: every admitted row names a provider in the compiled
    /// `PROVIDERS` table, and the generator proves the two agree.
    #[must_use]
    pub fn base_url(&self) -> &'static str {
        generated::PROVIDERS
            .iter()
            .find(|meta| meta.provider == self.entry.provider)
            .expect("every admitted row names a compiled provider")
            .base_url
    }
}

/// Why a `(provider, model)` pair did not qualify.
///
/// Every arm fails **before** dispatch, and carries the public wire code it
/// renders as. With a compiled table there is no staged or emergency-disabled
/// state: every row is admissible and everything else is unknown
/// (model-provider simplification 2026-08-13).
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
}

impl CatalogError {
    /// The public wire code this failure renders as.
    #[must_use]
    pub const fn error_code(&self) -> ErrorCode {
        match self {
            Self::UnknownProvider { .. } => ErrorCode::UnknownProvider,
            Self::UnknownModel { .. } => ErrorCode::UnknownModel,
        }
    }
}

/// The compiled revision identity: the vendored snapshot digest.
pub const SNAPSHOT_REVISION: CatalogRevision =
    CatalogRevision(Blake3Digest::from_bytes(generated::SNAPSHOT_DIGEST));

// The compiled table is never empty; a generator regression fails the build.
const _: () = assert!(!MODELS.is_empty());

/// Admits a `(provider, model)` pair against the compiled table.
///
/// # Errors
///
/// Returns [`CatalogError::UnknownProvider`] or [`CatalogError::UnknownModel`]
/// when the compiled table does not carry the pair.
pub fn admit(provider: ProviderId, model: &str) -> Result<QualifiedModel, CatalogError> {
    let slug = ModelSlug::new(model).map_err(|_| CatalogError::UnknownModel {
        provider,
        model: ModelSlug::truncating(model),
    })?;
    let start = MODELS.partition_point(|row| row.provider < provider);
    let end = MODELS.partition_point(|row| row.provider <= provider);
    let entry = MODELS[start..end]
        .binary_search_by(|row| row.model.cmp(model))
        .ok()
        .map(|index| &MODELS[start + index])
        .ok_or(CatalogError::UnknownModel {
            provider,
            model: slug.clone(),
        })?;
    Ok(QualifiedModel::new(entry, slug, SNAPSHOT_REVISION))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_compiled_provider_has_at_least_one_row() {
        for meta in generated::PROVIDERS {
            assert!(
                MODELS.iter().any(|row| row.provider == meta.provider),
                "{} has no rows",
                meta.base_url
            );
        }
    }

    #[test]
    fn the_known_launch_models_admit() {
        for (provider, model) in [
            (ProviderId::Deepseek, "deepseek-v4-flash"),
            (ProviderId::Deepseek, "deepseek-v4-pro"),
            (ProviderId::Anthropic, "claude-opus-4-5"),
            (ProviderId::Xai, "grok-4.3"),
            (ProviderId::Meta, "muse-spark-1.2"),
            (ProviderId::Moonshotai, "kimi-k2.5"),
            (ProviderId::Alibaba, "qwen3.7-plus"),
        ] {
            let qualified = admit(provider, model).expect("admitted");
            assert_eq!(qualified.provider(), provider);
            assert_eq!(qualified.model().as_str(), model);
            assert!(
                qualified
                    .capabilities()
                    .has(crate::document::Capability::Tools)
            );
        }
    }

    #[test]
    fn an_unknown_provider_and_model_fail_typed() {
        assert!(matches!(
            admit(ProviderId::Openai, "deepseek-v4-pro"),
            Err(CatalogError::UnknownModel { .. })
        ));
        assert!(matches!(
            admit(ProviderId::Openai, "no-such-model-anywhere"),
            Err(CatalogError::UnknownModel { .. })
        ));
    }

    #[test]
    fn only_the_seven_official_candidate_families_are_compiled() {
        let providers = generated::PROVIDERS
            .iter()
            .map(|meta| meta.provider)
            .collect::<Vec<_>>();
        assert_eq!(
            providers,
            vec![
                ProviderId::Openai,
                ProviderId::Anthropic,
                ProviderId::Deepseek,
                ProviderId::Xai,
                ProviderId::Meta,
                ProviderId::Moonshotai,
                ProviderId::Alibaba,
            ]
        );
    }
}
