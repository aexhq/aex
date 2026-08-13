//! Release-bound compiled model catalog: the checked-in models.dev admit
//! table is the whole authority.
//!
//! There is no signed collection, no trust roots, no runtime JSON and no
//! readiness gate: the table is compiled into the binary, `admit()` is the
//! only lookup, and a non-empty table is a compile-time assertion
//! (`aex-model-catalog` proves it with `const _: () = assert!(!MODELS.is_empty())`).

use aex_brain_app::ports::{CatalogError, CatalogPort};
use aex_brain_domain::ids::{CatalogPin, ModelSlug};
use aex_model_catalog::{QualifiedModel, admit};
use aex_wire::provider::ProviderId;

/// The compiled models.dev snapshot revision, as the single catalog pin.
#[must_use]
pub const fn admission_pin() -> CatalogPin {
    CatalogPin(aex_model_catalog::SNAPSHOT_REVISION.0)
}

/// The immutable compiled catalog authority.
///
/// There is deliberately no insertion or replacement method. A change is a
/// new snapshot riding the normal release train.
#[derive(Debug, Clone, Copy, Default)]
pub struct CompiledCatalogPort;

impl CatalogPort for CompiledCatalogPort {
    fn model(
        &self,
        pin: &CatalogPin,
        provider: ProviderId,
        model: &ModelSlug,
    ) -> Result<QualifiedModel, CatalogError> {
        if *pin != admission_pin() {
            return Err(CatalogError::UnknownPin { pin: *pin });
        }
        admit(provider, model.as_str()).map_err(CatalogError::Lookup)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_compiled_pin_admits_every_launch_model() {
        let catalog = CompiledCatalogPort;
        for (provider, model) in [
            (ProviderId::Openai, "gpt-5.2"),
            (ProviderId::Anthropic, "claude-opus-4-5"),
            (ProviderId::Google, "gemini-2.5-flash"),
            (ProviderId::Deepseek, "deepseek-v4-pro"),
            (ProviderId::Zai, "glm-4.6"),
            (ProviderId::Moonshotai, "kimi-k2.5"),
            (ProviderId::Openrouter, "deepseek/deepseek-v4-pro"),
            (ProviderId::VercelAiGateway, "anthropic/claude-opus-4.5"),
        ] {
            catalog
                .model(
                    &admission_pin(),
                    provider,
                    &ModelSlug::new(model).expect("slug"),
                )
                .expect("the compiled table admits the launch model");
        }
    }

    #[test]
    fn a_foreign_pin_never_admits() {
        let foreign = CatalogPin(aex_model_catalog::Blake3Digest::of(b"other"));
        assert!(matches!(
            CompiledCatalogPort.model(
                &foreign,
                ProviderId::Openai,
                &ModelSlug::new("gpt-5.2").expect("slug"),
            ),
            Err(CatalogError::UnknownPin { .. })
        ));
    }

    #[test]
    fn an_absent_model_is_typed_unknown() {
        assert!(matches!(
            CompiledCatalogPort.model(
                &admission_pin(),
                ProviderId::Openai,
                &ModelSlug::new("gpt-archive-9999").expect("slug"),
            ),
            Err(CatalogError::Lookup(_))
        ));
    }
}
