//! Catalog qualification properties: the compiled admit table, the thin
//! `QualifiedModel` handle, the `CatalogError` vocabulary, and the request
//! digest.

use aex_model_catalog::QualifiedModel;
use aex_model_catalog::canonical::{
    CanonicalMessage, CanonicalModelRequest, CorrelationId, ReasoningRequest, Role, SystemBlock,
    ToolChoice,
};
use aex_model_catalog::document::{Capability, CapabilitySet};
use aex_model_catalog::fixture;
use aex_model_catalog::primitives::{BoundedString, ModelSlug};
use aex_model_catalog::qualified::{CatalogError, admit};
use aex_wire::provider::ProviderId;
use aex_wire::{CanonicalJson, ContentHash, ResourceName};

fn model() -> QualifiedModel {
    fixture::qualified_entry(ProviderId::Openai, "gpt-5.2", CapabilitySet::EMPTY)
}

#[test]
fn a_qualified_model_exposes_the_entry_facts() {
    let model = model();
    assert_eq!(model.provider(), ProviderId::Openai);
    assert_eq!(model.model().as_str(), "gpt-5.2");
    assert_eq!(
        model.dialect(),
        aex_model_vocabulary::DialectClass::OpenAiResponses
    );
    assert_eq!(model.limits().context_window_tokens, 200_000);
    assert_eq!(model.limits().max_output_tokens, 8_192);
    assert!(model.catalog().to_wire().starts_with("mc1_"));
}

#[test]
fn the_two_bit_capability_set_behaves_as_declared() {
    assert!(!CapabilitySet::EMPTY.has(Capability::Tools));
    let tools = CapabilitySet::from_slice(&[Capability::Tools]);
    assert!(tools.has(Capability::Tools));
    assert!(!tools.has(Capability::ParallelTools));
    let both = tools.with(Capability::ParallelTools);
    assert!(both.has(Capability::Tools) && both.has(Capability::ParallelTools));
    assert_eq!(
        fixture::qualified_entry(ProviderId::Zai, "glm-4.6", both).capabilities(),
        both
    );
}

#[test]
fn the_catalog_error_vocabulary_carries_its_wire_code() {
    let unknown_provider = CatalogError::UnknownProvider {
        provider: ProviderId::Google,
    };
    assert_eq!(
        unknown_provider.error_code(),
        aex_wire::ErrorCode::UnknownProvider
    );

    let unknown_model = CatalogError::UnknownModel {
        provider: ProviderId::Openai,
        model: ModelSlug::new("gpt-4o").expect("slug"),
    };
    assert_eq!(
        unknown_model.error_code(),
        aex_wire::ErrorCode::UnknownModel
    );
}

#[test]
fn the_compiled_table_admits_every_provider_family() {
    for provider in ProviderId::ALL {
        let row = fixture::table()
            .iter()
            .find(|row| row.provider == *provider)
            .expect("every provider family has at least one row");
        let qualified = admit(row.provider, row.model).expect("the row admits");
        assert_eq!(qualified.provider(), row.provider);
        assert_eq!(qualified.model().as_str(), row.model);
    }
}

#[test]
fn admission_is_ordered_and_exact() {
    let qualified = admit(ProviderId::Deepseek, "deepseek-v4-pro").expect("admitted");
    assert_eq!(
        qualified.dialect(),
        aex_model_vocabulary::DialectClass::DeepSeekChat
    );
    assert!(matches!(
        admit(ProviderId::Deepseek, "deepseek"),
        Err(CatalogError::UnknownModel { .. })
    ));
    assert!(matches!(
        admit(ProviderId::Openai, "deepseek-v4-pro"),
        Err(CatalogError::UnknownModel { .. })
    ));
}

#[test]
fn the_snapshot_revision_is_the_vendored_digest() {
    let qualified = admit(ProviderId::Openai, "gpt-5.2").expect("admitted");
    assert_eq!(
        qualified.catalog(),
        aex_model_catalog::CatalogRevision(aex_model_catalog::Blake3Digest::from_bytes(
            fixture::snapshot_digest()
        ))
    );
}

fn request() -> CanonicalModelRequest {
    CanonicalModelRequest {
        selection: model(),
        system: vec![SystemBlock {
            text: BoundedString::truncating("be brief"),
            cacheable: true,
        }],
        messages: vec![CanonicalMessage {
            role: Role::User,
            blocks: Vec::new(),
        }],
        tools: Vec::new(),
        tool_choice: ToolChoice::Auto,
        parallel_tools: false,
        max_output_tokens: 4096,
        temperature_milli: None,
        top_p_milli: None,
        stop_sequences: Vec::new(),
        reasoning: ReasoningRequest::ProviderDefault,
        structured_output: None,
        cache_breakpoints: Vec::new(),
        correlation: CorrelationId(BoundedString::truncating("aex-correlation")),
        request_hash: ContentHash::of(b"placeholder"),
    }
}

#[test]
fn the_request_digest_covers_every_request_field() {
    let mut request = request();
    let base = request.digest().expect("digest");
    request.max_output_tokens += 1;
    assert_ne!(base, request.digest().expect("digest"));
    request.max_output_tokens -= 1;
    request.temperature_milli = Some(700);
    assert_ne!(base, request.digest().expect("digest"));
    request.temperature_milli = None;
    assert_eq!(base, request.digest().expect("the digest is deterministic"));
}

#[test]
fn hash_consistency_detects_a_stale_hash() {
    let mut request = request();
    request.request_hash = request.digest().expect("digest");
    assert!(request.hash_is_consistent().expect("consistent"));
    request.max_output_tokens = 1;
    assert!(!request.hash_is_consistent().expect("inconsistent"));
}

#[test]
fn the_fixture_mints_a_handle_without_touching_the_table() {
    let model = model();
    let _: &ModelSlug = model.model();
    let _: CanonicalJson = CanonicalJson::parse("{}").expect("json");
    let _: ResourceName = ResourceName::parse("read_file").expect("name");
    assert!(model.catalog().to_wire().starts_with("mc1_"));
}
