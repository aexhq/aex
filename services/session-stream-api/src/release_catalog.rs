//! Release-bound signed model-catalog authority for session admission.
//!
//! The release builder copies one exact verified collection into this binary.
//! There is no runtime JSON catalog and no empty fallback: an ordinary build is
//! useful for tests, but it cannot become a serving process.

use aex_brain_provider_gateway::VerifiedCatalogCollection;
use aex_brain_provider_gateway::signature::TrustedKeys;
use aex_model_catalog::CatalogError;
use aex_session_app::{ModelQualifier, QualificationRefusal, QualifiedModel};
use aex_wire::provider::ModelSelection;

include!(concat!(env!("OUT_DIR"), "/model_catalog_release.rs"));

/// Why the release catalog cannot become session admission authority.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SessionStreamReleaseCatalogError {
    /// This binary was not built by the catalog-bound release recipe.
    #[error("model catalog release binding unavailable: {0}")]
    MissingReleaseInputs(&'static str),
    /// The compiled collection failed complete signature/content verification.
    #[error(transparent)]
    InvalidCollection(#[from] aex_brain_provider_gateway::CatalogCollectionError),
    /// The collection verified but its admission head can serve no model.
    #[error("model catalog collection verifies but contains no Active serviceable model")]
    NoActiveModels,
}

/// Immutable, completely verified model qualification authority.
#[derive(Debug)]
pub struct ReleaseCatalog {
    inner: VerifiedCatalogCollection,
}

/// Loads the exact collection compiled into this binary.
///
/// # Errors
///
/// Missing build bindings, invalid collection authority, or a zero-serviceable
/// admission head refuses startup before the listener binds.
pub fn load(
    now: aex_wire::types::Timestamp,
) -> Result<ReleaseCatalog, SessionStreamReleaseCatalogError> {
    if let Some(reason) = RELEASE_INPUT_BLOCKER {
        return Err(SessionStreamReleaseCatalogError::MissingReleaseInputs(
            reason,
        ));
    }
    let keys = TrustedKeys::new(RELEASE_TRUSTED_KEYS);
    let inner = VerifiedCatalogCollection::load(RELEASE_CATALOG_COLLECTION, &keys, now)?;
    if !inner.is_service_capable() {
        return Err(SessionStreamReleaseCatalogError::NoActiveModels);
    }
    Ok(ReleaseCatalog { inner })
}

impl ModelQualifier for ReleaseCatalog {
    fn admit(
        &self,
        provider: aex_wire::provider::ProviderId,
        model: &str,
    ) -> Result<QualifiedModel, QualificationRefusal> {
        let pin = self.inner.admission_pin();
        let catalog = self
            .inner
            .catalog(pin)
            .expect("verified admission pin remains retained");
        let qualified = catalog
            .admit(&ModelSelection {
                credential_id: None,
                provider,
                model: model.to_owned(),
            })
            .map_err(|error| match error {
                CatalogError::UnknownProvider { .. } => QualificationRefusal::UnknownProvider,
                CatalogError::UnknownModel { .. } => QualificationRefusal::UnknownModel,
                CatalogError::UnqualifiedPair { .. } | CatalogError::EmergencyDisabled { .. } => {
                    QualificationRefusal::Unqualified
                }
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
    use super::{RELEASE_INPUT_BLOCKER, SessionStreamReleaseCatalogError, load};

    #[test]
    fn an_ordinary_build_cannot_become_session_admission_authority() {
        if let Some(reason) = RELEASE_INPUT_BLOCKER {
            let error = load(aex_model_catalog::fixture::at(1_800_000_000_000))
                .expect_err("ordinary builds carry no release authority");
            assert_eq!(
                error,
                SessionStreamReleaseCatalogError::MissingReleaseInputs(reason)
            );
            assert!(reason.contains("real publisher P-256 trust-root set"));
            assert!(reason.contains("signed model-catalog collection"));
        }
    }
}
