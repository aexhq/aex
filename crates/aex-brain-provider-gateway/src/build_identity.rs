//! Build-stamped identity of the complete six-adapter source tree.
//!
//! The crate build script hashes every Rust source below this crate's `src/`
//! with path and length framing, then injects the result into the compiled
//! library. The value cannot be supplied by the runtime environment and cannot
//! drift from the bytes Cargo compiled.

use aex_model_catalog::document::AdapterSourceDigest;
use aex_model_catalog::primitives::Blake3Digest;

/// The compile-time variable this crate's build script stamps.
pub const ADAPTER_DIGEST_BUILD_VAR: &str = "AEX_PROVIDER_ADAPTER_SOURCE_DIGEST";

/// Why the running binary cannot prove which adapter tree it contains.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum AdapterBuildIdentityError {
    /// The crate build script did not inject a digest.
    #[error(
        "provider adapter source digest is not build-stamped; the aex-brain-provider-gateway build script must derive AEX_PROVIDER_ADAPTER_SOURCE_DIGEST"
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
    use super::adapter_source_digest;

    #[test]
    fn every_build_is_stamped_from_the_source_tree_it_compiled() {
        assert!(option_env!("AEX_PROVIDER_ADAPTER_SOURCE_DIGEST").is_some());
        assert!(adapter_source_digest().is_ok());
    }
}
