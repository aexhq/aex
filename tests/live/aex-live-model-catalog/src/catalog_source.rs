//! Deterministic static model-catalog source generation.
//!
//! The checked-in source is compatibility and addressing metadata owned by
//! AEX. It carries no provider-availability assertion and consumes no live
//! qualification evidence. Publication signs only the canonical
//! [`CatalogDocument`] rendered from this closed, canonical source.

use aex_model_catalog::Catalog;
use aex_model_catalog::catalog::{CatalogHead, CatalogLoadError};
use aex_model_catalog::document::{CatalogDocument, CatalogSequence};
use aex_wire::canonical::CanonicalError;
use aex_wire::to_jcs_bytes;
use aex_wire::types::Timestamp;
use serde::{Deserialize, Serialize};

/// Schema of the checked-in static source envelope.
pub const STATIC_CATALOG_SOURCE_SCHEMA: &str = "aex.model-catalog-source.v1";
/// Only provider-native model currently admitted by the reviewed source.
pub const STATIC_DEEPSEEK_MODEL: &str = "deepseek-v4-flash";
/// Stable publisher identity for the first static catalog line.
pub const STATIC_PUBLISHER: &str = "aex-catalog-prd";
/// Explicit first sequence in the static catalog chain.
pub const STATIC_SEQUENCE: CatalogSequence = CatalogSequence(1);
/// AEX-owned beginning of the static catalog validity policy.
pub const STATIC_NOT_BEFORE: &str = "2026-08-05T00:00:00.000Z";

/// Canonical document plus its independently preflighted head.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GeneratedStaticCatalog {
    /// Exact JCS bytes presented to the signing boundary.
    pub canonical_document: Vec<u8>,
    /// Head derived by the core catalog preflight from those exact bytes.
    pub head: CatalogHead,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct StaticCatalogSource {
    schema: String,
    document: CatalogDocument,
}

/// Why static catalog source generation was refused.
#[derive(Debug, thiserror::Error)]
pub enum StaticCatalogSourceError {
    /// Source bytes are not parseable as the closed source envelope.
    #[error("static catalog source is malformed")]
    MalformedSource,
    /// Source value is valid JSON but not its exact JCS rendering.
    #[error("static catalog source must be exact canonical JSON with no trailing bytes")]
    NonCanonicalSource,
    /// Source schema is not the only supported version.
    #[error("static catalog source schema is unsupported")]
    UnsupportedSourceSchema,
    /// Generated document bytes differ from the caller-supplied validation target.
    #[error("catalog document bytes do not equal the deterministic static source rendering")]
    DocumentMismatch,
    /// Canonical serialization failed.
    #[error(transparent)]
    Canonical(#[from] CanonicalError),
    /// Runtime-equivalent unsigned document validation failed.
    #[error(transparent)]
    Catalog(#[from] CatalogLoadError),
}

/// Renders and independently preflights the checked-in static source.
///
/// # Errors
///
/// Refuses non-canonical/unknown source, serialization failure, or a document
/// rejected by the core catalog loader. Target-specific review belongs to the
/// checked-in source diff, not hardcoded generator policy.
pub fn generate_static_catalog(
    source_bytes: &[u8],
    now: Timestamp,
) -> Result<GeneratedStaticCatalog, StaticCatalogSourceError> {
    let source = parse_source(source_bytes)?;
    let canonical_document = to_jcs_bytes(&source.document)?;
    let head = Catalog::preflight_document(&canonical_document, now, None)?;
    Ok(GeneratedStaticCatalog {
        canonical_document,
        head,
    })
}

/// Proves a document is exactly the deterministic output of one static source.
///
/// # Errors
///
/// Returns [`StaticCatalogSourceError`] when source generation fails, bytes
/// differ, or independent core preflight rejects the supplied document.
pub fn validate_static_catalog(
    source_bytes: &[u8],
    document_bytes: &[u8],
    now: Timestamp,
) -> Result<CatalogHead, StaticCatalogSourceError> {
    let generated = generate_static_catalog(source_bytes, now)?;
    if generated.canonical_document != document_bytes {
        return Err(StaticCatalogSourceError::DocumentMismatch);
    }
    Catalog::preflight_document(document_bytes, now, None).map_err(Into::into)
}

fn parse_source(source_bytes: &[u8]) -> Result<StaticCatalogSource, StaticCatalogSourceError> {
    let value: serde_json::Value = serde_json::from_slice(source_bytes)
        .map_err(|_| StaticCatalogSourceError::MalformedSource)?;
    if to_jcs_bytes(&value)?.as_slice() != source_bytes {
        return Err(StaticCatalogSourceError::NonCanonicalSource);
    }
    let source: StaticCatalogSource = serde_json::from_slice(source_bytes)
        .map_err(|_| StaticCatalogSourceError::MalformedSource)?;
    if source.schema != STATIC_CATALOG_SOURCE_SCHEMA {
        return Err(StaticCatalogSourceError::UnsupportedSourceSchema);
    }
    Ok(source)
}
