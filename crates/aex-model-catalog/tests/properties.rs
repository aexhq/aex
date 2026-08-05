//! Catalog and canonical-vocabulary properties (plan 08 §2.5 MC-1..MC-10 and
//! test-order slices S-0.1, S-0.2, S-0.4, S-1.1..S-1.8).

use aex_wire::ErrorCode;
use aex_wire::provider::{ModelSelection, ProviderId};
use aex_wire::types::Timestamp;
use aex_wire::{CanonicalJson, ContentHash, ResourceName, to_jcs_bytes};
use aws_lc_rs::rand::SystemRandom;
use aws_lc_rs::signature::{ECDSA_P256_SHA256_ASN1_SIGNING, EcdsaKeyPair, KeyPair};
use proptest::prelude::*;

use aex_model_catalog::canonical::{
    CanonicalBlock, CanonicalMessage, NormalizedUsage, ReasoningBlock, ReasoningBody,
    ReasoningToken, Role, StopReason, TextAnnotation, ToolResultPart, UsageCompleteness, seal,
};
use aex_model_catalog::catalog::{Catalog, CatalogHead, CatalogLoadError, catalog_entry_digest};
use aex_model_catalog::document::{
    CapabilitySet, CatalogDigest, CatalogDocument, Dialect, DialectRevision, DisableReason,
    EmergencyDisable, EndpointPin, EntryState, ModelEntry, ReasoningReplay,
};
use aex_model_catalog::fixture;
use aex_model_catalog::primitives::{Blake3Digest, BoundedString, ModelSlug, ToolCallId};
use aex_model_catalog::qualified::{CatalogError, QualifiedModel};
use aex_model_catalog::signature::{
    CatalogEnvelope, CatalogSignature, P256_PUBLIC_KEY_BYTES, SIGNING_PREFIX, SigAlg,
    SignatureError, SigningKeyId, TrustedKey, TrustedKeys, verify,
};
use aex_model_catalog::wire_pending::CatalogRevision;

// ---------------------------------------------------------------------------
// harness
// ---------------------------------------------------------------------------

const NOW_MS: i64 = 1_800_000_000_000;
const PUBLISHER: &str = "aex-catalog-test";

fn now() -> Timestamp {
    fixture::at(NOW_MS)
}

struct Publisher {
    pair: EcdsaKeyPair,
    random: SystemRandom,
    keys: TrustedKeys,
}

impl Publisher {
    fn new() -> Self {
        let random = SystemRandom::new();
        let pair = EcdsaKeyPair::generate(&ECDSA_P256_SHA256_ASN1_SIGNING).expect("keygen");
        let mut public = [0u8; P256_PUBLIC_KEY_BYTES];
        public.copy_from_slice(pair.public_key().as_ref());
        let compiled: &'static [TrustedKey] = Box::leak(Box::new([(PUBLISHER, public)]));
        Self {
            pair,
            random,
            keys: TrustedKeys::new(compiled),
        }
    }

    fn seal(&self, document: &CatalogDocument) -> CatalogEnvelope {
        self.seal_bytes(fixture::canonical_bytes(document))
    }

    fn seal_bytes(&self, document: bytes::Bytes) -> CatalogEnvelope {
        let mut signed = Vec::from(SIGNING_PREFIX);
        signed.extend_from_slice(&document);
        let signature = self.pair.sign(&self.random, &signed).expect("sign");
        CatalogEnvelope {
            document,
            signatures: vec![CatalogSignature {
                key_id: SigningKeyId(fixture::bounded(PUBLISHER)),
                algorithm: SigAlg::EcdsaP256Sha256Asn1,
                bytes: bytes::Bytes::copy_from_slice(signature.as_ref()),
            }],
        }
    }
}

/// A document whose entries are all explicitly staged.
fn launch_document() -> CatalogDocument {
    let entries = vec![
        fixture::entry(ProviderId::Openai, "gpt-5.2", CapabilitySet::EMPTY),
        fixture::entry(ProviderId::Anthropic, "claude-opus-5", CapabilitySet::EMPTY),
        fixture::entry(
            ProviderId::Deepseek,
            "deepseek-v4-pro",
            CapabilitySet::EMPTY,
        ),
        fixture::entry(ProviderId::Zai, "glm-5.2", CapabilitySet::EMPTY),
        fixture::entry(ProviderId::Moonshotai, "kimi-k3", CapabilitySet::EMPTY),
        fixture::entry(ProviderId::Google, "gemini-3-pro", CapabilitySet::EMPTY),
        fixture::entry(
            ProviderId::Openrouter,
            "fixture/openrouter-model",
            CapabilitySet::EMPTY,
        ),
        fixture::entry(
            ProviderId::VercelAiGateway,
            "fixture/vercel-model",
            CapabilitySet::EMPTY,
        ),
    ];
    fixture::document(PUBLISHER, 1, entries, now())
}

fn load(publisher: &Publisher, document: &CatalogDocument) -> Result<Catalog, CatalogLoadError> {
    Catalog::load(&publisher.seal(document), &publisher.keys, now(), None)
}

fn selection(provider: ProviderId, model: &str) -> ModelSelection {
    ModelSelection {
        credential_id: None,
        model: model.to_owned(),
        provider,
    }
}

// ---------------------------------------------------------------------------
// MC-1 signature
// ---------------------------------------------------------------------------

#[test]
fn mc1_a_launch_document_loads_and_admits_nothing() {
    let publisher = Publisher::new();
    let catalog = load(&publisher, &launch_document()).expect("the launch document loads");
    assert_eq!(catalog.len(), 8);
    assert_eq!(catalog.active_len(), 0, "staged metadata admits no models");
    for provider in ProviderId::ALL {
        assert!(
            catalog
                .document()
                .entries
                .iter()
                .any(|entry| entry.provider == *provider),
            "{provider} is absent from the launch catalog"
        );
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(48))]

    /// MC-1: any single-bit mutation of the signed bytes fails `BadSignature`.
    #[test]
    fn mc1_any_single_bit_mutation_breaks_the_signature(byte in 0usize..512, bit in 0u8..8) {
        let publisher = Publisher::new();
        let document = launch_document();
        let envelope = publisher.seal(&document);
        let mut mutated = envelope.document.to_vec();
        prop_assume!(byte < mutated.len());
        mutated[byte] ^= 1u8 << bit;
        let attacked = CatalogEnvelope {
            document: bytes::Bytes::from(mutated),
            signatures: envelope.signatures.clone(),
        };
        prop_assert_eq!(
            verify(&attacked, &publisher.keys),
            Err(SignatureError::BadSignature(SigningKeyId(fixture::bounded(PUBLISHER))))
        );
    }
}

#[test]
fn mc1_an_untrusted_key_cannot_publish() {
    let publisher = Publisher::new();
    let attacker = Publisher::new();
    let document = launch_document();
    let envelope = attacker.seal(&document);
    let error = Catalog::load(&envelope, &publisher.keys, now(), None)
        .expect_err("a foreign key must not admit a catalog");
    // The attacker signed under the same id, so the failure is a bad signature
    // against the compiled key rather than an unknown id: either way, closed.
    assert!(matches!(
        error,
        CatalogLoadError::Signature(SignatureError::BadSignature(_))
    ));
}

// ---------------------------------------------------------------------------
// MC-2 canonicality
// ---------------------------------------------------------------------------

#[test]
fn mc2_a_semantically_equal_non_canonical_encoding_is_rejected() {
    let publisher = Publisher::new();
    let document = launch_document();
    let canonical = fixture::canonical_bytes(&document);

    // Same value, different bytes: two spaces of indentation.
    let value: serde_json::Value = serde_json::from_slice(&canonical).expect("parse");
    let pretty = serde_json::to_vec_pretty(&value).expect("pretty");
    assert_ne!(pretty, canonical.to_vec());

    let envelope = publisher.seal_bytes(bytes::Bytes::from(pretty));
    let error = Catalog::load(&envelope, &publisher.keys, now(), None)
        .expect_err("a non-canonical encoding must not load");
    assert!(matches!(error, CatalogLoadError::NotCanonical { .. }));
}

#[test]
fn mc2_reserializing_a_loaded_document_reproduces_the_exact_bytes() {
    let publisher = Publisher::new();
    let document = launch_document();
    let canonical = fixture::canonical_bytes(&document);
    let catalog = load(&publisher, &document).expect("load");
    let round_tripped = to_jcs_bytes(catalog.document()).expect("canonicalize");
    assert_eq!(round_tripped, canonical.to_vec());
    assert_eq!(
        catalog.digest(),
        CatalogDigest(Blake3Digest::of(&canonical))
    );
    assert_eq!(catalog.revision(), CatalogRevision(catalog.digest().0));
}

// ---------------------------------------------------------------------------
// MC-3 activation
// ---------------------------------------------------------------------------

fn head(catalog: &Catalog) -> CatalogHead {
    catalog.head()
}

#[test]
fn mc3_a_lower_sequence_is_a_downgrade() {
    let publisher = Publisher::new();
    let first = launch_document();
    let active = head(&load(&publisher, &first).expect("load"));

    let mut older = launch_document();
    older.sequence = aex_model_catalog::document::CatalogSequence(0);
    let error = Catalog::load(
        &publisher.seal(&older),
        &publisher.keys,
        now(),
        Some(&active),
    )
    .expect_err("a downgrade must be refused");
    assert!(matches!(error, CatalogLoadError::Downgrade { .. }));
}

#[test]
fn mc3_a_broken_predecessor_chain_is_refused() {
    let publisher = Publisher::new();
    let first = launch_document();
    let active = head(&load(&publisher, &first).expect("load"));

    let mut next = launch_document();
    next.sequence = aex_model_catalog::document::CatalogSequence(2);
    next.predecessor = Some(CatalogDigest(Blake3Digest::of(b"not the active digest")));
    let error = Catalog::load(
        &publisher.seal(&next),
        &publisher.keys,
        now(),
        Some(&active),
    )
    .expect_err("a broken chain must be refused");
    assert!(matches!(error, CatalogLoadError::BrokenChain { .. }));

    next.predecessor = Some(active.digest);
    Catalog::load(
        &publisher.seal(&next),
        &publisher.keys,
        now(),
        Some(&active),
    )
    .expect("an intact chain advances");
}

#[test]
fn mc3_a_publisher_change_is_never_automatic() {
    let publisher = Publisher::new();
    let active = head(&load(&publisher, &launch_document()).expect("load"));
    let mut renamed = launch_document();
    renamed.publisher = aex_model_catalog::document::PublisherId(fixture::bounded("someone-else"));
    renamed.sequence = aex_model_catalog::document::CatalogSequence(9);
    let error = Catalog::load(
        &publisher.seal(&renamed),
        &publisher.keys,
        now(),
        Some(&active),
    )
    .expect_err("a publisher change must be refused");
    assert!(matches!(error, CatalogLoadError::PublisherChanged { .. }));
}

#[test]
fn mc3_re_offering_the_active_revision_is_idempotent() {
    let publisher = Publisher::new();
    let document = launch_document();
    let active = head(&load(&publisher, &document).expect("load"));
    Catalog::load(
        &publisher.seal(&document),
        &publisher.keys,
        now(),
        Some(&active),
    )
    .expect("the same revision reloads");
}

// ---------------------------------------------------------------------------
// MC-4 unknown capability
// ---------------------------------------------------------------------------

#[test]
fn mc4_an_unknown_field_fails_load() {
    let publisher = Publisher::new();
    let document = launch_document();
    let mut value: serde_json::Value =
        serde_json::from_slice(&fixture::canonical_bytes(&document)).expect("parse");
    value
        .as_object_mut()
        .expect("object")
        .insert("futureField".to_owned(), serde_json::Value::Bool(true));
    let bytes = to_jcs_bytes(&value).expect("canonicalize");
    let error = Catalog::load(
        &publisher.seal_bytes(bytes::Bytes::from(bytes)),
        &publisher.keys,
        now(),
        None,
    )
    .expect_err("an unknown field must fail load");
    assert!(matches!(error, CatalogLoadError::UnknownField { .. }));
}

#[test]
fn mc4_an_unknown_enum_member_fails_load() {
    let publisher = Publisher::new();
    let document = launch_document();
    let mut value: serde_json::Value =
        serde_json::from_slice(&fixture::canonical_bytes(&document)).expect("parse");
    value["entries"][0]["state"] = serde_json::Value::String("provisional".to_owned());
    let bytes = to_jcs_bytes(&value).expect("canonicalize");
    let error = Catalog::load(
        &publisher.seal_bytes(bytes::Bytes::from(bytes)),
        &publisher.keys,
        now(),
        None,
    )
    .expect_err("an unknown enum member must fail load");
    match error {
        CatalogLoadError::UnknownEnumMember { value, .. } => {
            assert_eq!(value.as_str(), "provisional");
        }
        other => panic!("expected UnknownEnumMember, got {other:?}"),
    }
}

#[test]
fn mc4_an_unknown_capability_bit_fails_load() {
    let publisher = Publisher::new();
    let mut document = launch_document();
    document.entries[0].capabilities = CapabilitySet(1 << 31);
    let error = load(&publisher, &document).expect_err("an unknown bit must fail load");
    assert!(matches!(
        error,
        CatalogLoadError::UnknownCapabilityBit { .. }
    ));
}

#[test]
fn mc4_a_wrong_schema_version_fails_load() {
    let publisher = Publisher::new();
    let mut document = launch_document();
    document.schema_version = 2;
    let error = load(&publisher, &document).expect_err("a future schema must fail load");
    assert!(matches!(
        error,
        CatalogLoadError::SchemaVersion { found: 2 }
    ));
}

// ---------------------------------------------------------------------------
// MC-5 time gates
// ---------------------------------------------------------------------------

#[test]
fn mc5_not_before_is_exact_to_the_millisecond_and_has_no_expiry_horizon() {
    let publisher = Publisher::new();
    let document = launch_document();
    let envelope = publisher.seal(&document);
    let not_before = document.not_before.unix_millis();

    let at_load =
        |millis: i64| Catalog::load(&envelope, &publisher.keys, fixture::at(millis), None);

    assert!(matches!(
        at_load(not_before - 1),
        Err(CatalogLoadError::NotYetValid { .. })
    ));
    at_load(not_before).expect("valid at exactly not_before");
    let catalog = at_load(not_before + 10 * 365 * 24 * 60 * 60 * 1000)
        .expect("signed compatibility metadata has no magic expiry horizon");
    let pin = selection(ProviderId::Openai, "gpt-5.2");
    catalog
        .qualified(&pin)
        .expect("committed history stays readable");
    assert!(matches!(
        catalog.admit(&pin),
        Err(CatalogError::UnqualifiedPair {
            state: EntryState::Staged
        }),
    ));
}

#[test]
fn mc5_unordered_time_gates_fail_load() {
    let publisher = Publisher::new();
    let mut document = launch_document();
    document.issued_at = fixture::at(NOW_MS + 1);
    let error = load(&publisher, &document).expect_err("unordered gates must fail");
    assert_eq!(error, CatalogLoadError::TimeGatesUnordered);
}

// ---------------------------------------------------------------------------
// MC-6 emergency disable
// ---------------------------------------------------------------------------

fn active_document() -> CatalogDocument {
    let mut document = launch_document();
    for entry in &mut document.entries {
        fixture::promote(entry);
    }
    document
}

#[test]
fn mc6_a_disabled_pair_blocks_a_new_run_but_not_committed_history() {
    let publisher = Publisher::new();
    let mut document = active_document();
    document.emergency_disable = vec![EmergencyDisable {
        provider: ProviderId::Openai,
        model: ModelSlug::new("gpt-5.2").expect("slug"),
        since: now(),
        reason: DisableReason::ProviderIncident,
    }];
    let catalog = load(&publisher, &document).expect("load");

    let disabled = selection(ProviderId::Openai, "gpt-5.2");
    assert_eq!(
        catalog.admit(&disabled),
        Err(CatalogError::EmergencyDisabled {
            reason: DisableReason::ProviderIncident
        })
    );
    catalog
        .qualified(&disabled)
        .expect("history written under this revision stays resolvable");

    let healthy = selection(ProviderId::Anthropic, "claude-opus-5");
    catalog
        .admit(&healthy)
        .expect("an unaffected pair still admits");
}

#[test]
fn mc6_an_unsorted_disable_list_fails_load() {
    let publisher = Publisher::new();
    let mut document = active_document();
    document.emergency_disable = vec![
        EmergencyDisable {
            provider: ProviderId::Anthropic,
            model: ModelSlug::new("claude-opus-5").expect("slug"),
            since: now(),
            reason: DisableReason::Withdrawn,
        },
        EmergencyDisable {
            provider: ProviderId::Openai,
            model: ModelSlug::new("gpt-5.2").expect("slug"),
            since: now(),
            reason: DisableReason::Withdrawn,
        },
    ];
    // `Openai` sorts before `Anthropic` in the declared ProviderId order, so the
    // list above is out of order.
    let error = load(&publisher, &document).expect_err("unsorted disables must fail");
    assert!(matches!(
        error,
        CatalogLoadError::DisableListUnsorted { .. }
    ));
}

// ---------------------------------------------------------------------------
// MC-7 / MC-8 signed state and compiled compatibility metadata
// ---------------------------------------------------------------------------

#[test]
fn mc7_signed_active_metadata_admits_without_live_qualification_evidence() {
    let publisher = Publisher::new();
    let document = active_document();
    let catalog = load(&publisher, &document).expect("signed Active metadata loads");
    catalog
        .admit(&selection(ProviderId::Openai, "gpt-5.2"))
        .expect("provider availability is not an admission authority");
}

#[test]
fn mc7_a_staged_entry_never_admits() {
    let publisher = Publisher::new();
    let mut document = active_document();
    document.entries[0].state = EntryState::Staged;
    let catalog = load(&publisher, &document).expect("load");
    assert_eq!(
        catalog.admit(&selection(ProviderId::Openai, "gpt-5.2")),
        Err(CatalogError::UnqualifiedPair {
            state: EntryState::Staged
        })
    );
}

#[test]
fn mc7_a_deprecated_entry_never_admits() {
    let publisher = Publisher::new();
    let mut document = active_document();
    document.entries[0].state = EntryState::Deprecated;
    let catalog = load(&publisher, &document).expect("load");
    assert_eq!(
        catalog.admit(&selection(ProviderId::Openai, "gpt-5.2")),
        Err(CatalogError::UnqualifiedPair {
            state: EntryState::Deprecated
        })
    );
}

#[test]
fn mc8_an_unimplemented_dialect_fails_load() {
    let publisher = Publisher::new();
    let mut document = active_document();
    document.entries[0].dialect = Dialect::GeminiInteractions;
    let error = load(&publisher, &document).expect_err("reserved dialect must fail");
    assert!(matches!(
        error,
        CatalogLoadError::DialectNotImplemented { .. }
    ));
}

#[test]
fn mc8_a_dialect_for_another_provider_fails_load() {
    let publisher = Publisher::new();
    let mut document = active_document();
    document.entries[0].dialect = Dialect::AnthropicMessages;
    let error = load(&publisher, &document).expect_err("mismatched dialect must fail");
    assert!(matches!(
        error,
        CatalogLoadError::DialectProviderMismatch { .. }
    ));
}

#[test]
fn mc8_an_endpoint_for_another_provider_fails_load() {
    let publisher = Publisher::new();
    let mut document = active_document();
    document.entries[0].endpoint = EndpointPin::AnthropicApi;
    let error = load(&publisher, &document).expect_err("mismatched endpoint must fail");
    assert!(matches!(
        error,
        CatalogLoadError::EndpointProviderMismatch { .. }
    ));
}

#[test]
fn mc8_an_unsupported_dialect_revision_fails_load() {
    let publisher = Publisher::new();
    let mut document = active_document();
    document.entries[0].dialect_revision = DialectRevision(2);
    let error = load(&publisher, &document).expect_err("unsupported revision must fail");
    assert!(matches!(
        error,
        CatalogLoadError::DialectRevisionUnsupported {
            found: DialectRevision(2),
            ..
        }
    ));
}

#[test]
fn mc8_external_assurance_identity_changes_with_compatibility_metadata() {
    let mut entry = fixture::entry(ProviderId::Openai, "gpt-5.2", CapabilitySet::EMPTY);
    let before = catalog_entry_digest(&entry).expect("entry digest");
    entry.limits.max_output_tokens -= 1;
    let after = catalog_entry_digest(&entry).expect("entry digest");
    assert_ne!(before, after);
}

#[test]
fn a_forbidden_in_call_retry_status_fails_load() {
    let publisher = Publisher::new();
    let mut document = launch_document();
    document.entries[0].retry_policy.in_call_status_retry = vec![429, 500];
    let error = load(&publisher, &document).expect_err("500 is not an in-call retry status");
    assert!(matches!(
        error,
        CatalogLoadError::RetryStatusForbidden { status: 500, .. }
    ));
}

#[test]
fn too_many_in_call_attempts_fail_load() {
    let publisher = Publisher::new();
    let mut document = launch_document();
    document.entries[0].retry_policy.max_attempts = 9;
    let error = load(&publisher, &document).expect_err("D-20 caps attempts at three");
    assert!(matches!(
        error,
        CatalogLoadError::RetryAttemptsOutOfRange { attempts: 9, .. }
    ));
}

// ---------------------------------------------------------------------------
// MC-9 ordering
// ---------------------------------------------------------------------------

#[test]
fn mc9_unsorted_entries_fail_load() {
    let publisher = Publisher::new();
    let mut document = launch_document();
    document.entries.swap(0, 1);
    let error = load(&publisher, &document).expect_err("unsorted entries must fail");
    assert!(matches!(error, CatalogLoadError::EntriesUnsorted { .. }));
}

#[test]
fn mc9_a_duplicate_pair_fails_load() {
    let publisher = Publisher::new();
    let mut document = launch_document();
    let duplicate = document.entries[0].clone();
    document.entries.insert(1, duplicate);
    let error = load(&publisher, &document).expect_err("a duplicate pair must fail");
    assert!(matches!(error, CatalogLoadError::DuplicateEntry { .. }));
}

// ---------------------------------------------------------------------------
// MC-10 scale
// ---------------------------------------------------------------------------

#[test]
fn mc10_a_three_hundred_entry_document_loads_and_looks_up_by_binary_search() {
    let publisher = Publisher::new();
    let mut entries: Vec<ModelEntry> = Vec::with_capacity(300);
    for index in 0..300u32 {
        let provider = ProviderId::ALL[(index as usize) % ProviderId::ALL.len()];
        entries.push(fixture::entry(
            provider,
            &format!("model-{index:04}"),
            CapabilitySet::EMPTY,
        ));
    }
    let document = fixture::document(PUBLISHER, 1, entries, now());
    let catalog = load(&publisher, &document).expect("a 300-entry document loads");
    assert_eq!(catalog.len(), 300);

    for index in 0..300u32 {
        let provider = ProviderId::ALL[(index as usize) % ProviderId::ALL.len()];
        let pair = selection(provider, &format!("model-{index:04}"));
        let resolved = catalog.qualified(&pair).expect("every entry resolves");
        assert_eq!(resolved.provider(), provider);
    }
    assert!(
        catalog
            .qualified(&selection(ProviderId::Openai, "model-9999"))
            .is_err()
    );
}

// ---------------------------------------------------------------------------
// S-1.8 the CatalogError -> ErrorCode mapping
// ---------------------------------------------------------------------------

#[test]
fn s18_every_catalog_failure_carries_its_wire_code() {
    let publisher = Publisher::new();
    let mut document = active_document();
    // Drop every google entry so the provider itself is unknown.
    document
        .entries
        .retain(|entry| entry.provider != ProviderId::Google);
    let catalog = load(&publisher, &document).expect("load");

    let unknown_provider = catalog
        .qualified(&selection(ProviderId::Google, "gemini-3-pro"))
        .expect_err("google is absent");
    assert_eq!(
        unknown_provider,
        CatalogError::UnknownProvider {
            provider: ProviderId::Google
        }
    );
    assert_eq!(unknown_provider.error_code(), ErrorCode::UnknownProvider);

    let unknown_model = catalog
        .qualified(&selection(ProviderId::Openai, "gpt-4o"))
        .expect_err("the model is absent");
    assert_eq!(unknown_model.error_code(), ErrorCode::UnknownModel);

    let mut staged = active_document();
    staged.entries[0].state = EntryState::Staged;
    let catalog = load(&publisher, &staged).expect("load");
    let unqualified = catalog
        .admit(&selection(ProviderId::Openai, "gpt-5.2"))
        .expect_err("a staged pair never admits");
    assert_eq!(
        unqualified.error_code(),
        ErrorCode::UnqualifiedProviderModel
    );
}

#[test]
fn a_model_slug_longer_than_the_bound_is_an_unknown_model_not_a_panic() {
    let publisher = Publisher::new();
    let catalog = load(&publisher, &launch_document()).expect("load");
    let error = catalog
        .qualified(&selection(ProviderId::Openai, &"m".repeat(4096)))
        .expect_err("an over-long slug cannot name an entry");
    assert_eq!(error.error_code(), ErrorCode::UnknownModel);
}

#[test]
fn the_anthropic_spelling_has_no_alias() {
    assert_eq!(ProviderId::parse("anthropic"), Some(ProviderId::Anthropic));
    assert_eq!(ProviderId::parse("anthrophic"), None);
}

// ---------------------------------------------------------------------------
// S-0.1 / S-0.2 the canonical vocabulary
// ---------------------------------------------------------------------------

fn qualified(publisher: &Publisher, replay: ReasoningReplay) -> QualifiedModel {
    let mut document = active_document();
    document.entries[0].reasoning.replay = replay;
    let catalog = load(publisher, &document).expect("load");
    let entry = &catalog.document().entries[0];
    catalog
        .qualified(&selection(entry.provider, entry.model.as_str()))
        .expect("the entry resolves")
}

fn text(body: &str) -> CanonicalBlock {
    CanonicalBlock::Text {
        text: fixture::bounded(body),
        annotations: Vec::new(),
    }
}

fn tool_use(id: &str) -> CanonicalBlock {
    CanonicalBlock::ToolUse {
        id: ToolCallId::new(id).expect("id"),
        name: ResourceName::parse("read_file").expect("name"),
        input: CanonicalJson::parse("{\"path\":\"a\"}").expect("json"),
    }
}

#[test]
fn s01_the_canonical_corpus_round_trips_byte_identically() {
    let corpus = vec![
        text("hello"),
        CanonicalBlock::Text {
            text: fixture::bounded("cited"),
            annotations: vec![TextAnnotation {
                start: 0,
                end: 5,
                kind: aex_model_catalog::canonical::AnnotationKind::Citation,
            }],
        },
        CanonicalBlock::Reasoning(ReasoningBlock {
            body: ReasoningBody::Text {
                text: fixture::bounded("step one"),
            },
            token: Some(ReasoningToken {
                provenance: ProviderId::Anthropic,
                bytes: bytes::Bytes::from_static(b"signature-material"),
            }),
        }),
        CanonicalBlock::Reasoning(ReasoningBlock {
            body: ReasoningBody::Redacted,
            token: None,
        }),
        tool_use("call_1"),
        CanonicalBlock::ToolResult {
            call: ToolCallId::new("call_1").expect("id"),
            content: vec![
                ToolResultPart::Text {
                    text: fixture::bounded("ok"),
                },
                ToolResultPart::Json {
                    value: CanonicalJson::parse("{\"b\":2,\"a\":1}").expect("json"),
                },
            ],
            is_error: false,
        },
        CanonicalBlock::Refusal {
            text: fixture::bounded("I cannot help with that"),
        },
    ];
    let message = CanonicalMessage {
        role: Role::Assistant,
        blocks: corpus,
    };
    let first = to_jcs_bytes(&message).expect("canonicalize");
    let parsed: CanonicalMessage = serde_json::from_slice(&first).expect("parse");
    let second = to_jcs_bytes(&parsed).expect("canonicalize");
    assert_eq!(first, second);
    assert_eq!(parsed, message);
}

#[test]
fn s01_an_unknown_block_kind_is_rejected() {
    let error = serde_json::from_str::<CanonicalBlock>("{\"kind\":\"video\",\"url\":\"x\"}")
        .expect_err("an unknown block kind must not decode");
    assert!(error.to_string().contains("unknown variant"));
}

#[test]
fn s02_seal_rejects_an_empty_block_set() {
    let publisher = Publisher::new();
    let model = qualified(&publisher, ReasoningReplay::NotRequired);
    assert_eq!(
        seal(
            Vec::new(),
            StopReason::EndTurn,
            &NormalizedUsage::default(),
            &model
        ),
        Err(aex_model_catalog::canonical::SealError::EmptyBlocks)
    );
}

#[test]
fn s02_seal_rejects_inconsistent_usage() {
    let publisher = Publisher::new();
    let model = qualified(&publisher, ReasoningReplay::NotRequired);
    let usage = NormalizedUsage {
        output_tokens: 1,
        reasoning_tokens: 2,
        ..NormalizedUsage::default()
    };
    assert_eq!(
        seal(vec![text("answer")], StopReason::EndTurn, &usage, &model),
        Err(aex_model_catalog::canonical::SealError::InconsistentUsage)
    );
}

#[test]
fn s02_seal_rejects_a_duplicate_tool_call_id() {
    let publisher = Publisher::new();
    let model = qualified(&publisher, ReasoningReplay::NotRequired);
    let error = seal(
        vec![tool_use("call_1"), tool_use("call_1")],
        StopReason::ToolUse,
        &NormalizedUsage::default(),
        &model,
    )
    .expect_err("duplicate ids must be refused");
    assert!(matches!(
        error,
        aex_model_catalog::canonical::SealError::DuplicateToolCallId(_)
    ));
}

#[test]
fn s02_seal_rejects_an_unbalanced_tool_use() {
    let publisher = Publisher::new();
    let model = qualified(&publisher, ReasoningReplay::NotRequired);
    assert_eq!(
        seal(
            vec![tool_use("call_1")],
            StopReason::EndTurn,
            &NormalizedUsage::default(),
            &model
        ),
        Err(aex_model_catalog::canonical::SealError::UnbalancedToolUse)
    );
    assert_eq!(
        seal(
            vec![text("no tools here")],
            StopReason::ToolUse,
            &NormalizedUsage::default(),
            &model
        ),
        Err(aex_model_catalog::canonical::SealError::UnbalancedToolUse)
    );
}

#[test]
fn s02_seal_rejects_a_refusal_with_no_content() {
    let publisher = Publisher::new();
    let model = qualified(&publisher, ReasoningReplay::NotRequired);
    assert_eq!(
        seal(
            vec![CanonicalBlock::Refusal {
                text: fixture::bounded("no")
            }],
            StopReason::Refusal,
            &NormalizedUsage::default(),
            &model
        ),
        Err(aex_model_catalog::canonical::SealError::RefusalWithoutContent)
    );
}

#[test]
fn s02_seal_requires_reasoning_material_where_the_entry_declares_it() {
    let publisher = Publisher::new();
    let model = qualified(&publisher, ReasoningReplay::RequiredAlways);
    let bare = ReasoningBlock {
        body: ReasoningBody::Text {
            text: fixture::bounded("thinking"),
        },
        token: None,
    };
    assert_eq!(
        seal(
            vec![CanonicalBlock::Reasoning(bare.clone()), text("answer")],
            StopReason::EndTurn,
            &NormalizedUsage::default(),
            &model
        ),
        Err(aex_model_catalog::canonical::SealError::ReasoningTokenMissing)
    );

    let carried = ReasoningBlock {
        token: Some(ReasoningToken {
            provenance: model.provider(),
            bytes: bytes::Bytes::from_static(b"sig"),
        }),
        ..bare
    };
    seal(
        vec![CanonicalBlock::Reasoning(carried), text("answer")],
        StopReason::EndTurn,
        &NormalizedUsage::default(),
        &model,
    )
    .expect("carried material seals");
}

#[test]
fn s02_seal_rejects_reasoning_material_from_another_provider() {
    let publisher = Publisher::new();
    let model = qualified(&publisher, ReasoningReplay::NotRequired);
    let foreign = ProviderId::ALL
        .iter()
        .copied()
        .find(|provider| *provider != model.provider())
        .expect("another provider exists");
    let error = seal(
        vec![
            CanonicalBlock::Reasoning(ReasoningBlock {
                body: ReasoningBody::Redacted,
                token: Some(ReasoningToken {
                    provenance: foreign,
                    bytes: bytes::Bytes::from_static(b"sig"),
                }),
            }),
            text("answer"),
        ],
        StopReason::EndTurn,
        &NormalizedUsage::default(),
        &model,
    )
    .expect_err("a foreign token must never be replayed");
    assert!(matches!(
        error,
        aex_model_catalog::canonical::SealError::ReasoningProvenanceMismatch { .. }
    ));
}

#[test]
fn s02_the_proof_changes_with_every_input_it_covers() {
    let publisher = Publisher::new();
    let model = qualified(&publisher, ReasoningReplay::NotRequired);
    let base = seal(
        vec![text("answer")],
        StopReason::EndTurn,
        &NormalizedUsage::default(),
        &model,
    )
    .expect("seals");

    let other_stop = seal(
        vec![text("answer")],
        StopReason::MaxOutputTokens,
        &NormalizedUsage::default(),
        &model,
    )
    .expect("seals");
    assert_ne!(base.proof, other_stop.proof);

    let other_text = seal(
        vec![text("answer!")],
        StopReason::EndTurn,
        &NormalizedUsage::default(),
        &model,
    )
    .expect("seals");
    assert_ne!(base.proof, other_text.proof);

    let other_usage = seal(
        vec![text("answer")],
        StopReason::EndTurn,
        &NormalizedUsage {
            output_tokens: 1,
            ..NormalizedUsage::default()
        },
        &model,
    )
    .expect("seals");
    assert_ne!(base.proof, other_usage.proof);

    let same = seal(
        vec![text("answer")],
        StopReason::EndTurn,
        &NormalizedUsage::default(),
        &model,
    )
    .expect("seals");
    assert_eq!(base.proof, same.proof, "sealing is deterministic");
}

#[test]
fn s02_a_sealed_message_carries_its_provider_model_and_revision() {
    let publisher = Publisher::new();
    let model = qualified(&publisher, ReasoningReplay::NotRequired);
    let sealed = seal(
        vec![text("answer")],
        StopReason::EndTurn,
        &NormalizedUsage::default(),
        &model,
    )
    .expect("seals");
    assert_eq!(sealed.provider, model.provider());
    assert_eq!(&sealed.model, model.model());
    assert_eq!(sealed.catalog, model.catalog());
}

// ---------------------------------------------------------------------------
// S-0.4 usage arithmetic
// ---------------------------------------------------------------------------

#[test]
fn s04_reasoning_tokens_are_always_a_subset_of_output_tokens() {
    // The Gemini case: the provider reports thoughts outside candidates, so the
    // adapter must add before the value reaches this type (D-11).
    let candidates = 120u64;
    let thoughts = 45u64;
    let google = NormalizedUsage {
        input_tokens: 10,
        output_tokens: candidates + thoughts,
        reasoning_tokens: thoughts,
        provider_total_tokens: Some(175),
        completeness: UsageCompleteness::Exact,
        ..NormalizedUsage::default()
    };
    assert!(google.reasoning_tokens <= google.output_tokens);
    assert_eq!(google.output_tokens, 165);

    // The Anthropic/OpenAI case: output already includes reasoning, so the
    // adapter adds nothing.
    let anthropic = NormalizedUsage {
        input_tokens: 10,
        output_tokens: 165,
        reasoning_tokens: 45,
        completeness: UsageCompleteness::Exact,
        ..NormalizedUsage::default()
    };
    assert_eq!(anthropic.output_tokens, google.output_tokens);
    assert_eq!(anthropic.reasoning_tokens, google.reasoning_tokens);
}

#[test]
fn s04_absent_usage_is_recorded_not_invented() {
    let absent = NormalizedUsage {
        completeness: UsageCompleteness::Absent,
        ..NormalizedUsage::default()
    };
    assert_eq!(absent.input_tokens, 0);
    assert_eq!(absent.output_tokens, 0);
    assert_eq!(absent.completeness, UsageCompleteness::Absent);
}

#[test]
fn the_content_hash_of_a_sealed_message_is_the_workspace_content_hash() {
    // A regression guard on D-30's split: the catalog *digest* is blake3, the
    // completeness proof is the workspace `ContentHash`.
    let publisher = Publisher::new();
    let model = qualified(&publisher, ReasoningReplay::NotRequired);
    let sealed = seal(
        vec![text("answer")],
        StopReason::EndTurn,
        &NormalizedUsage::default(),
        &model,
    )
    .expect("seals");
    assert!(sealed.proof.0.to_wire().starts_with("sha256:"));
    let _: ContentHash = sealed.proof.0;
    assert!(model.catalog().to_wire().starts_with("mc1_"));
}

#[test]
fn a_bounded_string_refuses_growth_past_its_bound() {
    let too_long = "x".repeat(1025);
    assert!(BoundedString::<1024>::new(too_long.clone()).is_err());
    assert_eq!(BoundedString::<1024>::truncating(&too_long).len(), 1024);
}
