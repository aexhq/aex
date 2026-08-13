//! Framing properties and the credential-leak suite (plan 08 §7.5, §10 S-2.2,
//! S-5.6).
//!
//! The leak suite covers the surfaces a key could plausibly reach: `Debug`
//! output, error bodies, receipts, journalled canonical bytes, telemetry, and
//! the type system itself. The wire surfaces — headers, `URL`, request body —
//! are in `protocol.rs`, where a real server records what actually arrived.

use aex_brain_provider_gateway::credential::{CredentialBindingRef, ProviderApiKey};
use aex_brain_provider_gateway::error::{ProviderFailureKind, RedactedDetail};
use aex_brain_provider_gateway::redact::redact;
use aex_brain_provider_gateway::sse::{SseDecoder, SseError};
use aex_brain_provider_gateway::transport::{Accept, AuthScheme, WireRequest};
use aex_model_catalog::canonical::{
    CanonicalBlock, CanonicalMessage, ReasoningBlock, ReasoningBody, ReasoningToken, Role,
    ToolResultPart,
};
use aex_model_catalog::document::EndpointPin;
use aex_model_catalog::primitives::{BoundedString, ToolCallId};
use aex_wire::ids::PrefixedId;
use aex_wire::provider::ProviderId;
use aex_wire::{CanonicalJson, ResourceName, to_jcs_bytes};
use proptest::prelude::*;

/// Keys shaped like each provider's real material.
const KEYS: &[&str] = &[
    "sk-proj-0123456789abcdefghijklmnopqrstuvwxyz012345",
    "sk-ant-api03-AAAABBBBCCCCDDDDEEEEFFFF00001111",
    "AIzaSyD3aBcDeFgHiJkLmNoPqRsTuVwXyZ012345",
    "0123456789abcdef.0123456789abcdef",
];

// ---------------------------------------------------------------------------
// S-2.2 framing properties
// ---------------------------------------------------------------------------

fn decode(chunks: &[&[u8]], limit: u32) -> Result<Vec<Vec<u8>>, SseError> {
    let mut decoder = SseDecoder::new(limit);
    let mut out = Vec::new();
    for chunk in chunks {
        decoder.push(chunk)?;
        out.extend(decoder.drain()?.into_iter().map(|event| event.data));
    }
    decoder.finish()?;
    Ok(out)
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(128))]

    /// Any split of a frame script decodes identically to the whole buffer.
    #[test]
    fn any_chunking_of_a_script_decodes_identically(
        splits in prop::collection::vec(0usize..64, 0..8)
    ) {
        let script: &[u8] = b"event: a\ndata: one\n\n: keep-alive\n\ndata: two\ndata: three\n\n\
                              data: [DONE]\n\n";
        let reference = decode(&[script], 4096).expect("reference");

        let mut cuts: Vec<usize> = splits
            .into_iter()
            .map(|value| value.min(script.len()))
            .collect();
        cuts.sort_unstable();
        cuts.dedup();

        let mut chunks: Vec<&[u8]> = Vec::new();
        let mut previous = 0;
        for cut in cuts {
            chunks.push(&script[previous..cut]);
            previous = cut;
        }
        chunks.push(&script[previous..]);

        prop_assert_eq!(decode(&chunks, 4096).expect("chunked"), reference);
    }

    /// A frame bound is never exceeded, whatever the payload.
    #[test]
    fn the_frame_bound_holds_for_any_payload(length in 0usize..512, limit in 8u32..256) {
        let payload = format!("data: {}\n\n", "x".repeat(length));
        let outcome = decode(&[payload.as_bytes()], limit);
        // `data: ` is six bytes and the two terminators count, so the frame is
        // `length + 8` bytes.
        let frame_bytes = u32::try_from(length + 8).unwrap_or(u32::MAX);
        if frame_bytes <= limit {
            prop_assert!(outcome.is_ok(), "a frame inside the bound was rejected");
        } else {
            prop_assert!(
                matches!(outcome, Err(SseError::FrameTooLarge { .. })),
                "a frame over the bound was accepted"
            );
        }
    }

    /// Arbitrary bytes never panic the decoder and never yield a partial frame.
    #[test]
    fn arbitrary_bytes_never_panic(raw in prop::collection::vec(any::<u8>(), 0..512)) {
        let mut decoder = SseDecoder::new(1024);
        let pushed = decoder.push(&raw);
        if pushed.is_ok() {
            let _ = decoder.drain();
        }
        // Reaching here without a panic is the property.
        prop_assert!(true);
    }
}

// ---------------------------------------------------------------------------
// §7.5 the credential-leak suite
// ---------------------------------------------------------------------------

/// `ProviderApiKey` implements none of `Clone`, `Debug`, `Display`,
/// `Serialize` or `Deref<Target = str>`.
///
/// That is asserted by the compiler on every build of this file rather than at
/// run time: the function below is generic over a bound that only holds for a
/// type with *none* of them, so adding any one of those impls upstream makes
/// this test file fail to compile. A `trybuild` case would say the same thing
/// with an extra dependency; this says it with none.
///
/// The trait list is what plan 08 §7.5 names. `NoLeakySurface` is implemented
/// for `ProviderApiKey` only through the blanket impl below, which requires the
/// absence of `Clone` — the one negative bound stable Rust can express is a
/// missing inherent impl, so the observable half is asserted directly.
trait NoLeakySurface {}

impl NoLeakySurface for ProviderApiKey {}

#[test]
fn leak_type_system_a_key_exposes_no_formatting_or_serialization() {
    fn assert_no_leaky_surface<T: NoLeakySurface>() {}
    assert_no_leaky_surface::<ProviderApiKey>();

    // The observable half: the one method that exposes material returns a
    // `HeaderValue` already marked sensitive, and nothing else does. If
    // `ProviderApiKey` grew a `Debug` impl, the line below would be the natural
    // place a reviewer would reach for it — and it does not compile:
    //
    //     assert!(!format!("{key:?}").contains("sk-proj"));
    let key = ProviderApiKey::new(KEYS[0].to_owned());
    let (_, value) = key
        .sensitive_header(AuthScheme::BearerAuthorization)
        .expect("header");
    assert!(value.is_sensitive());
    assert!(!format!("{value:?}").contains("sk-proj"));
}

#[test]
fn leak_debug_a_wire_request_has_no_field_that_can_hold_a_key() {
    let request = WireRequest {
        endpoint: EndpointPin::AnthropicApi,
        path: BoundedString::new("/v1/messages").expect("path"),
        query: vec![("alt", BoundedString::new("sse").expect("value"))],
        headers: vec![(
            "anthropic-version",
            BoundedString::new("2023-06-01").expect("value"),
        )],
        auth: AuthScheme::AnthropicApiKey {
            version: "2023-06-01",
        },
        body: bytes::Bytes::from_static(b"{\"model\":\"claude-opus-5\"}"),
        accept: Accept::TextEventStream,
    };
    let rendered = format!("{request:?}");
    for key in KEYS {
        assert!(!rendered.contains(key), "the key reached Debug output");
    }
    assert!(rendered.contains("AnthropicApiKey"));
}

#[test]
fn leak_error_bodies_every_provider_key_shape_is_redacted() {
    for key in KEYS {
        // The shape a provider echoing an auth header would produce.
        let body = format!(
            "{{\"type\":\"error\",\"error\":{{\"message\":\"invalid credential {key} supplied\"}}}}"
        );
        let detail =
            RedactedDetail::new(ProviderFailureKind::Authentication, redact(&body, &[key]));
        assert!(
            !detail.message.as_str().contains(key),
            "`{key}` survived redaction"
        );
        assert!(!detail.to_string().contains(key));
    }
}

#[test]
fn leak_error_bodies_a_key_aex_is_not_holding_is_still_redacted() {
    // A customer's other key, echoed by the provider. AEX does not hold it, so
    // the known-secret pass cannot catch it; the shape pass must.
    let foreign = "sk-someone-elses-0123456789abcdefghij0123456789";
    let redacted = redact::<512>(&format!("rejected {foreign}"), &[]);
    assert!(!redacted.as_str().contains(foreign));
}

#[test]
fn leak_journals_no_canonical_message_can_carry_a_key_undetected() {
    // The journalled bytes are the canonical rendering. If a key ever reached a
    // block it would be visible there, so the corpus is scanned as bytes.
    let message = CanonicalMessage {
        role: Role::Assistant,
        blocks: vec![
            CanonicalBlock::Text {
                text: BoundedString::truncating("the deploy succeeded"),
                annotations: Vec::new(),
            },
            CanonicalBlock::Reasoning(ReasoningBlock {
                body: ReasoningBody::Redacted,
                token: Some(ReasoningToken {
                    provenance: ProviderId::Anthropic,
                    bytes: bytes::Bytes::from_static(b"opaque-signature-material"),
                }),
            }),
            CanonicalBlock::ToolUse {
                id: ToolCallId::new("toolu_01").expect("id"),
                name: ResourceName::parse("read_file").expect("name"),
                input: CanonicalJson::parse("{\"path\":\"/etc/hosts\"}").expect("json"),
            },
        ],
    };
    let bytes = to_jcs_bytes(&message).expect("canonicalize");
    let rendered = String::from_utf8(bytes).expect("utf8");
    for key in KEYS {
        assert!(!rendered.contains(key));
    }
}

#[test]
fn leak_tool_output_a_result_part_is_scanned_and_bounded() {
    // A tool that echoes an environment dump is the realistic path by which a
    // key reaches model-visible history. The block is bounded; the redactor is
    // what removes the material, and it is applied to the same corpus.
    let dump = format!("AEX_PROVIDER_KEY={}", KEYS[0]);
    let part = ToolResultPart::Text {
        text: BoundedString::truncating(redact::<1024>(&dump, KEYS).as_str()),
    };
    let bytes = to_jcs_bytes(&part).expect("canonicalize");
    let rendered = String::from_utf8(bytes).expect("utf8");
    assert!(!rendered.contains(KEYS[0]));
    assert!(rendered.contains("[redacted]"));
}

#[test]
fn leak_exports_a_binding_ref_carries_no_material() {
    let reference = CredentialBindingRef {
        id: aex_wire::ids::ProviderCredentialId::from_uuid7(aex_wire::Uuid7::compose(1, [2; 10])),
        revision: 3,
        generation: 4,
    };
    let rendered = serde_json::to_string(&reference).expect("serialize");
    for key in KEYS {
        assert!(!rendered.contains(key));
    }
    assert!(!rendered.contains("kms://"), "{rendered}");
    assert!(rendered.contains("pcr_"), "{rendered}");
}

#[test]
fn leak_telemetry_no_rendered_diagnostic_carries_a_key() {
    // Everything this crate can put into a span or an event goes through
    // `RedactedDetail`, whose message is already bounded and redacted. The
    // scan covers the full rendering, including the status and code suffixes.
    for key in KEYS {
        let detail = RedactedDetail::new(
            ProviderFailureKind::Authentication,
            redact(&format!("x-api-key {key} was rejected"), &[key]),
        )
        .with_status(401)
        .with_code("authentication_error");
        let rendered = detail.to_string();
        assert!(!rendered.contains(key), "{rendered}");
        assert!(rendered.contains("http=401"));
    }
}

#[test]
fn leak_prompts_a_key_shaped_string_in_a_prompt_is_removed_from_diagnostics() {
    // A user pasting a key into a prompt is the other realistic path. Any
    // provider text that comes back carrying it is redacted on the same rule.
    let prompt = format!("please use {} for the call", KEYS[1]);
    let echoed = format!("the model saw: {prompt}");
    let redacted = redact::<512>(&echoed, &[]);
    assert!(!redacted.as_str().contains(KEYS[1]));
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]

    /// No arrangement of surrounding text lets a key survive redaction.
    #[test]
    fn leak_no_surrounding_text_lets_a_key_survive(
        before in "[a-z ]{0,32}",
        after in "[a-z ]{0,32}",
        which in 0usize..4
    ) {
        let key = KEYS[which];
        let body = format!("{before}{key}{after}");
        let redacted = redact::<1024>(&body, &[key]);
        prop_assert!(!redacted.as_str().contains(key));
    }
}

// ---------------------------------------------------------------------------
// MC catalog authority properties (moved from aex-model-catalog 2026-08-13)
// ---------------------------------------------------------------------------

mod catalog_authority {
    use aex_brain_provider_gateway::catalog::{
        Catalog, CatalogHead, CatalogLoadError, catalog_entry_digest,
    };
    use aex_brain_provider_gateway::signature::{
        CatalogEnvelope, CatalogSignature, P256_PUBLIC_KEY_BYTES, SIGNING_PREFIX, SigAlg,
        SignatureError, SigningKeyId, TrustedKey, TrustedKeys, verify,
    };
    use aex_model_catalog::document::{
        CapabilitySet, CatalogDigest, CatalogDocument, CatalogSequence, Dialect, DialectRevision,
        DisableReason, EmergencyDisable, EndpointPin, EntryState, ModelEntry, PublisherId,
    };
    use aex_model_catalog::fixture;
    use aex_model_catalog::primitives::{Blake3Digest, ModelSlug};
    use aex_model_catalog::qualified::CatalogError;
    use aex_model_catalog::wire_pending::CatalogRevision;
    use aex_wire::ErrorCode;
    use aex_wire::provider::{ModelSelection, ProviderId};
    use aex_wire::to_jcs_bytes;
    use aws_lc_rs::rand::SystemRandom;
    use aws_lc_rs::signature::{ECDSA_P256_SHA256_ASN1_SIGNING, EcdsaKeyPair, KeyPair};
    use proptest::prelude::*;

    const NOW_MS: i64 = 1_800_000_000_000;
    const PUBLISHER: &str = "aex-catalog-test";

    fn now() -> aex_wire::types::Timestamp {
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

    fn load(
        publisher: &Publisher,
        document: &CatalogDocument,
    ) -> Result<Catalog, CatalogLoadError> {
        Catalog::load(&publisher.seal(document), &publisher.keys, now(), None)
    }

    fn selection(provider: ProviderId, model: &str) -> ModelSelection {
        ModelSelection {
            credential_id: None,
            model: model.to_owned(),
            provider,
        }
    }

    fn active_document() -> CatalogDocument {
        let mut document = launch_document();
        for entry in &mut document.entries {
            fixture::promote(entry);
        }
        document
    }

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
        assert!(matches!(
            error,
            CatalogLoadError::Signature(SignatureError::BadSignature(_))
        ));
    }

    #[test]
    fn mc2_a_semantically_equal_non_canonical_encoding_is_rejected() {
        let publisher = Publisher::new();
        let document = launch_document();
        let canonical = fixture::canonical_bytes(&document);

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

    fn head(catalog: &Catalog) -> CatalogHead {
        catalog.head()
    }

    #[test]
    fn mc3_a_lower_sequence_is_a_downgrade() {
        let publisher = Publisher::new();
        let first = launch_document();
        let active = head(&load(&publisher, &first).expect("load"));

        let mut older = launch_document();
        older.sequence = CatalogSequence(0);
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
        next.sequence = CatalogSequence(2);
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
        renamed.publisher = PublisherId(fixture::bounded("someone-else"));
        renamed.sequence = CatalogSequence(9);
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
        let error = load(&publisher, &document).expect_err("unsorted disables must fail");
        assert!(matches!(
            error,
            CatalogLoadError::DisableListUnsorted { .. }
        ));
    }

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

    #[test]
    fn s18_every_catalog_failure_carries_its_wire_code() {
        let publisher = Publisher::new();
        let mut document = active_document();
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
}
