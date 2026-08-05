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

#[cfg(test)]
mod tests {
    use aex_model_catalog::document::{CatalogDocument, EntryState};
    use aex_model_catalog::fixture;
    use aex_wire::provider::ProviderId;
    use aex_wire::{to_jcs_bytes, types::Timestamp};
    use serde::Serialize;

    use super::{
        STATIC_CATALOG_SOURCE_SCHEMA, STATIC_DEEPSEEK_MODEL, STATIC_NOT_BEFORE, STATIC_PUBLISHER,
        STATIC_SEQUENCE, StaticCatalogSourceError, generate_static_catalog,
        validate_static_catalog,
    };

    const SOURCE: &[u8] =
        include_bytes!("../../../../release/model-catalog/deepseek-v4-flash-genesis.source.json");

    #[derive(Serialize)]
    struct SourceEnvelope {
        schema: &'static str,
        document: CatalogDocument,
    }

    fn now() -> Timestamp {
        Timestamp::parse(STATIC_NOT_BEFORE).expect("reviewed initial not-before")
    }

    #[test]
    fn reviewed_genesis_source_is_exact_and_deterministic() {
        let first = generate_static_catalog(SOURCE, now()).expect("reviewed source preflights");
        let second = generate_static_catalog(SOURCE, now()).expect("same source preflights");
        assert_eq!(first, second);
        assert_eq!(
            validate_static_catalog(SOURCE, &first.canonical_document, now())
                .expect("independent check"),
            first.head
        );

        let document: CatalogDocument =
            serde_json::from_slice(&first.canonical_document).expect("generated document");
        assert_eq!(document.publisher.0.as_str(), STATIC_PUBLISHER);
        assert_eq!(document.sequence, STATIC_SEQUENCE);
        assert_eq!(document.predecessor, None);
        assert_eq!(document.issued_at, now());
        assert_eq!(document.not_before, now());
        assert!(document.emergency_disable.is_empty());
        let [entry] = document.entries.as_slice() else {
            panic!("initial source must contain exactly one reviewed entry");
        };
        assert_eq!(entry.provider, ProviderId::Deepseek);
        assert_eq!(entry.model.as_str(), STATIC_DEEPSEEK_MODEL);
        assert_eq!(entry.state, EntryState::Active);
    }

    #[test]
    fn generic_generator_accepts_an_explicit_later_revision_and_multiple_entries() {
        let genesis = generate_static_catalog(SOURCE, now()).expect("genesis");
        let mut document: CatalogDocument =
            serde_json::from_slice(&genesis.canonical_document).expect("document");
        document.sequence.0 = 2;
        document.predecessor = Some(genesis.head.digest);
        document.entries.push(fixture::entry(
            ProviderId::Openai,
            "gpt-reviewed",
            aex_model_catalog::document::CapabilitySet::EMPTY,
        ));
        document.entries.sort_by(|left, right| {
            (left.provider, left.model.as_str()).cmp(&(right.provider, right.model.as_str()))
        });
        let source = to_jcs_bytes(&SourceEnvelope {
            schema: STATIC_CATALOG_SOURCE_SCHEMA,
            document,
        })
        .expect("canonical later source");
        let generated = generate_static_catalog(&source, now()).expect("generic later source");
        assert_eq!(generated.head.sequence.0, 2);
    }

    #[test]
    fn source_and_generated_bytes_are_both_closed_and_canonical() {
        let generated = generate_static_catalog(SOURCE, now()).expect("genesis");
        let mut noncanonical = SOURCE.to_vec();
        noncanonical.push(b'\n');
        assert!(matches!(
            generate_static_catalog(&noncanonical, now()),
            Err(StaticCatalogSourceError::NonCanonicalSource)
        ));
        let mut different = generated.canonical_document.clone();
        different.push(b'\n');
        assert!(matches!(
            validate_static_catalog(SOURCE, &different, now()),
            Err(StaticCatalogSourceError::DocumentMismatch)
        ));
    }
}
