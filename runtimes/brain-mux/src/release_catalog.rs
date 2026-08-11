//! Release-bound signed model-catalog authority.
//!
//! The bounded publisher trust-root set and collection bytes are build inputs
//! copied into the binary by `build.rs`. Runtime environment variables are never consulted.
//! Missing real release inputs are a start-up refusal: `main` maps the error
//! into a non-zero process exit, rather than an invented key, an empty
//! authority, or a task that idles alive but unready.

use aex_brain_provider_gateway::catalog_port::{CatalogArtifactError, VerifiedCatalogPort};
use aex_model_catalog::signature::TrustedKeys;

include!(concat!(env!("OUT_DIR"), "/model_catalog_release.rs"));

/// Build-time variable carrying the canonical bounded publisher trust-root set.
pub const TRUST_ROOTS_JSON_BUILD_VAR: &str = "AEX_MODEL_CATALOG_TRUST_ROOTS_JSON";
/// Build-time variable binding the exact trust-root-set bytes.
pub const TRUST_ROOTS_SHA256_BUILD_VAR: &str = "AEX_MODEL_CATALOG_TRUST_ROOTS_SHA256";
/// Build-time variable naming the exact release collection copied into the binary.
pub const COLLECTION_FILE_BUILD_VAR: &str = "AEX_MODEL_CATALOG_COLLECTION_FILE";
/// Build-time variable binding the exact collection bytes recorded by the release plan.
pub const COLLECTION_SHA256_BUILD_VAR: &str = "AEX_MODEL_CATALOG_COLLECTION_SHA256";

/// Why this binary cannot install model-catalog authority.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum BrainMuxReleaseCatalogError {
    /// The build did not carry the real paired release inputs.
    #[error("model catalog release binding unavailable: {0}")]
    MissingReleaseInputs(&'static str),
    /// The compiled collection did not verify completely.
    #[error(transparent)]
    InvalidCollection(#[from] CatalogArtifactError),
    /// Signatures and coverage verified, but no wake could route to an Active model.
    #[error("model catalog collection verifies but contains no Active serviceable model")]
    NoActiveModels,
}

/// Loads the immutable authority compiled into this exact binary.
///
/// # Errors
///
/// Missing real release inputs, any collection verification failure, or a
/// cryptographically valid but service-incapable zero-Active collection
/// refuses start-up: the caller exits non-zero instead of serving without
/// catalog authority.
pub fn load(
    now: aex_wire::types::Timestamp,
) -> Result<VerifiedCatalogPort, BrainMuxReleaseCatalogError> {
    if let Some(reason) = RELEASE_INPUT_BLOCKER {
        return Err(BrainMuxReleaseCatalogError::MissingReleaseInputs(reason));
    }
    let keys = TrustedKeys::new(RELEASE_TRUSTED_KEYS);
    let catalog = VerifiedCatalogPort::load_collection(RELEASE_CATALOG_COLLECTION, &keys, now)?;
    if !catalog.is_service_capable() {
        return Err(BrainMuxReleaseCatalogError::NoActiveModels);
    }
    Ok(catalog)
}

#[cfg(test)]
mod tests {
    use super::{BrainMuxReleaseCatalogError, RELEASE_INPUT_BLOCKER, load};

    #[test]
    fn an_ordinary_build_names_the_real_release_inputs_it_lacks() {
        if let Some(reason) = RELEASE_INPUT_BLOCKER {
            let error = load(aex_model_catalog::fixture::at(1_800_000_000_000))
                .expect_err("no production release inputs are present");
            assert_eq!(
                error,
                BrainMuxReleaseCatalogError::MissingReleaseInputs(reason)
            );
            assert!(reason.contains("real publisher P-256 trust-root set"));
            assert!(reason.contains("signed model-catalog collection"));
        }
    }
}
