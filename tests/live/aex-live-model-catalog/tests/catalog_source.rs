//! Static model-catalog source and deterministic generator regression tests.

use aex_live_model_catalog::catalog_source::{
    STATIC_CATALOG_SOURCE_SCHEMA, STATIC_DEEPSEEK_MODEL, STATIC_NOT_BEFORE, STATIC_PUBLISHER,
    STATIC_SEQUENCE, StaticCatalogSourceError, generate_static_catalog, validate_static_catalog,
};
use aex_model_catalog::document::{CatalogDocument, EntryState};
use aex_model_catalog::fixture;
use aex_wire::provider::ProviderId;
use aex_wire::to_jcs_bytes;
use aex_wire::types::Timestamp;
use serde::Serialize;

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
