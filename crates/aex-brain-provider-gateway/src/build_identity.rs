//! Build-stamped identity of the complete six-adapter source tree.

use aex_model_catalog::document::AdapterSourceDigest;
use aex_model_catalog::primitives::Blake3Digest;

/// The compile-time variable the release builder must stamp.
pub const ADAPTER_DIGEST_BUILD_VAR: &str = "AEX_PROVIDER_ADAPTER_SOURCE_DIGEST";

/// Why the running binary cannot prove which adapter tree it contains.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum AdapterBuildIdentityError {
    /// The release build did not inject a digest.
    #[error(
        "provider adapter source digest is not build-stamped; the release builder must set AEX_PROVIDER_ADAPTER_SOURCE_DIGEST at compile time"
    )]
    Missing,
    /// The injected value is not one lowercase blake3-256 digest.
    #[error(
        "the build-stamped AEX_PROVIDER_ADAPTER_SOURCE_DIGEST is not 64 lowercase hexadecimal characters"
    )]
    Invalid,
}

/// Returns the immutable digest injected into this binary by the release build.
///
/// Runtime environment variables are deliberately ignored: an operator must
/// not be able to relabel arbitrary bytes as the adapter tree that earned a
/// conformance receipt.
///
/// # Errors
///
/// Returns [`AdapterBuildIdentityError`] when this was not a stamped release
/// build or the stamp was malformed.
pub fn adapter_source_digest() -> Result<AdapterSourceDigest, AdapterBuildIdentityError> {
    let raw = option_env!("AEX_PROVIDER_ADAPTER_SOURCE_DIGEST")
        .ok_or(AdapterBuildIdentityError::Missing)?;
    Blake3Digest::from_hex(raw)
        .map(AdapterSourceDigest)
        .map_err(|_| AdapterBuildIdentityError::Invalid)
}

#[cfg(test)]
mod tests {
    use super::{AdapterBuildIdentityError, adapter_source_digest};

    #[test]
    fn an_ordinary_developer_build_cannot_claim_a_release_identity() {
        if option_env!("AEX_PROVIDER_ADAPTER_SOURCE_DIGEST").is_none() {
            assert_eq!(
                adapter_source_digest(),
                Err(AdapterBuildIdentityError::Missing)
            );
        }
    }
}
