//! Release-bound compiled model catalog for session admission.
//!
//! The checked-in models.dev admit table is the whole authority: admission
//! consults the compiled table directly, and `aex-model-catalog` proves the
//! table non-empty with a compile-time assertion. There is no runtime JSON
//! catalog and no empty fallback.

use aex_model_catalog::CatalogError;
use aex_session_app::{ModelQualifier, QualificationRefusal, QualifiedModel};
use aex_wire::provider::ProviderId;

/// Immutable, completely compiled model qualification authority.
#[derive(Debug, Clone, Copy, Default)]
pub struct ReleaseCatalog;

impl ModelQualifier for ReleaseCatalog {
    fn admit(
        &self,
        provider: ProviderId,
        model: &str,
    ) -> Result<QualifiedModel, QualificationRefusal> {
        let qualified = aex_model_catalog::admit(provider, model).map_err(|error| match error {
            CatalogError::UnknownProvider { .. } => QualificationRefusal::UnknownProvider,
            CatalogError::UnknownModel { .. } => QualificationRefusal::UnknownModel,
        })?;
        Ok(QualifiedModel {
            provider: qualified.provider(),
            model: qualified.model().as_str().to_owned(),
            catalog_revision: qualified.catalog().to_wire(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_compiled_table_admits_the_launch_models() {
        let catalog = ReleaseCatalog;
        for (provider, model) in [
            (ProviderId::Openai, "gpt-5.2"),
            (ProviderId::Anthropic, "claude-opus-4-5"),
            (ProviderId::Deepseek, "deepseek-v4-pro"),
            (ProviderId::Xai, "grok-4.3"),
            (ProviderId::Meta, "muse-spark-1.2"),
            (ProviderId::Moonshotai, "kimi-k2.5"),
            (ProviderId::Alibaba, "qwen3.7-plus"),
        ] {
            let qualified = catalog.admit(provider, model).expect("admitted");
            assert!(qualified.catalog_revision.starts_with("mc1_"));
        }
    }

    #[test]
    fn an_unknown_pair_is_typed_refused() {
        assert!(matches!(
            ReleaseCatalog.admit(ProviderId::Openai, "deepseek-v4-pro"),
            Err(QualificationRefusal::UnknownModel)
        ));
    }
}
