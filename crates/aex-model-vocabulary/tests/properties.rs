//! Canonical-vocabulary properties (plan 08 §2.5 test-order slices S-0.1,
//! S-0.2, S-0.4).

use aex_model_vocabulary::CatalogRevision;
use aex_model_vocabulary::canonical::{
    AnnotationKind, CanonicalBlock, CanonicalMessage, NormalizedUsage, ReasoningBlock,
    ReasoningBody, ReasoningToken, Role, SealError, SealModel, StopReason, TextAnnotation,
    ToolResultPart, UsageCompleteness, seal,
};
use aex_model_vocabulary::primitives::{BoundedString, ModelSlug, ToolCallId};
use aex_wire::provider::ProviderId;
use aex_wire::types::Timestamp;
use aex_wire::{CanonicalJson, ContentHash, ResourceName, to_jcs_bytes};

fn bounded<const N: usize>(text: &str) -> BoundedString<N> {
    BoundedString::new(text).unwrap_or_else(|error| panic!("fixture literal is too long: {error}"))
}

fn text(body: &str) -> CanonicalBlock {
    CanonicalBlock::Text {
        text: bounded(body),
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

/// A fixed seal-model stub: the vocabulary must not depend on the catalog's
/// `QualifiedModel`, so the seal properties run against this shape instead.
struct StubModel {
    provider: ProviderId,
    model: ModelSlug,
    catalog: CatalogRevision,
    requires_reasoning_token: bool,
}

impl SealModel for StubModel {
    fn provider(&self) -> ProviderId {
        self.provider
    }

    fn model(&self) -> &ModelSlug {
        &self.model
    }

    fn catalog(&self) -> CatalogRevision {
        self.catalog
    }

    fn requires_reasoning_token(&self, _has_tool_use: bool) -> bool {
        self.requires_reasoning_token
    }
}

fn model() -> StubModel {
    StubModel {
        provider: ProviderId::Openai,
        model: ModelSlug::new("gpt-5.2").expect("slug"),
        catalog: CatalogRevision(aex_model_vocabulary::Blake3Digest::of(b"fixture")),
        requires_reasoning_token: false,
    }
}

fn demanding_model() -> StubModel {
    StubModel {
        requires_reasoning_token: true,
        ..model()
    }
}

#[test]
fn s01_the_canonical_corpus_round_trips_byte_identically() {
    let corpus = vec![
        text("hello"),
        CanonicalBlock::Text {
            text: bounded("cited"),
            annotations: vec![TextAnnotation {
                start: 0,
                end: 5,
                kind: AnnotationKind::Citation,
            }],
        },
        CanonicalBlock::Reasoning(ReasoningBlock {
            body: ReasoningBody::Text {
                text: bounded("step one"),
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
                    text: bounded("ok"),
                },
                ToolResultPart::Json {
                    value: CanonicalJson::parse("{\"b\":2,\"a\":1}").expect("json"),
                },
            ],
            is_error: false,
        },
        CanonicalBlock::Refusal {
            text: bounded("I cannot help with that"),
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
    assert_eq!(
        seal(
            Vec::new(),
            StopReason::EndTurn,
            &NormalizedUsage::default(),
            &model()
        ),
        Err(SealError::EmptyBlocks)
    );
}

#[test]
fn s02_seal_rejects_inconsistent_usage() {
    let usage = NormalizedUsage {
        output_tokens: 1,
        reasoning_tokens: 2,
        ..NormalizedUsage::default()
    };
    assert_eq!(
        seal(vec![text("answer")], StopReason::EndTurn, &usage, &model()),
        Err(SealError::InconsistentUsage)
    );
}

#[test]
fn s02_seal_rejects_a_duplicate_tool_call_id() {
    let error = seal(
        vec![tool_use("call_1"), tool_use("call_1")],
        StopReason::ToolUse,
        &NormalizedUsage::default(),
        &model(),
    )
    .expect_err("duplicate ids must be refused");
    assert!(matches!(error, SealError::DuplicateToolCallId(_)));
}

#[test]
fn s02_seal_rejects_an_unbalanced_tool_use() {
    assert_eq!(
        seal(
            vec![tool_use("call_1")],
            StopReason::EndTurn,
            &NormalizedUsage::default(),
            &model()
        ),
        Err(SealError::UnbalancedToolUse)
    );
    assert_eq!(
        seal(
            vec![text("no tools here")],
            StopReason::ToolUse,
            &NormalizedUsage::default(),
            &model()
        ),
        Err(SealError::UnbalancedToolUse)
    );
}

#[test]
fn s02_seal_rejects_a_refusal_with_no_content() {
    assert_eq!(
        seal(
            vec![CanonicalBlock::Refusal {
                text: bounded("no")
            }],
            StopReason::Refusal,
            &NormalizedUsage::default(),
            &model()
        ),
        Err(SealError::RefusalWithoutContent)
    );
}

#[test]
fn s02_seal_requires_reasoning_material_where_the_model_declares_it() {
    let bare = ReasoningBlock {
        body: ReasoningBody::Text {
            text: bounded("thinking"),
        },
        token: None,
    };
    assert_eq!(
        seal(
            vec![CanonicalBlock::Reasoning(bare.clone()), text("answer")],
            StopReason::EndTurn,
            &NormalizedUsage::default(),
            &demanding_model()
        ),
        Err(SealError::ReasoningTokenMissing)
    );

    let carried = ReasoningBlock {
        token: Some(ReasoningToken {
            provenance: ProviderId::Openai,
            bytes: bytes::Bytes::from_static(b"sig"),
        }),
        ..bare
    };
    seal(
        vec![CanonicalBlock::Reasoning(carried), text("answer")],
        StopReason::EndTurn,
        &NormalizedUsage::default(),
        &demanding_model(),
    )
    .expect("carried material seals");
}

#[test]
fn s02_seal_rejects_reasoning_material_from_another_provider() {
    let error = seal(
        vec![
            CanonicalBlock::Reasoning(ReasoningBlock {
                body: ReasoningBody::Redacted,
                token: Some(ReasoningToken {
                    provenance: ProviderId::Anthropic,
                    bytes: bytes::Bytes::from_static(b"sig"),
                }),
            }),
            text("answer"),
        ],
        StopReason::EndTurn,
        &NormalizedUsage::default(),
        &model(),
    )
    .expect_err("a foreign token must never be replayed");
    assert!(matches!(
        error,
        SealError::ReasoningProvenanceMismatch { .. }
    ));
}

#[test]
fn s02_the_proof_changes_with_every_input_it_covers() {
    let base = seal(
        vec![text("answer")],
        StopReason::EndTurn,
        &NormalizedUsage::default(),
        &model(),
    )
    .expect("seals");

    let other_stop = seal(
        vec![text("answer")],
        StopReason::MaxOutputTokens,
        &NormalizedUsage::default(),
        &model(),
    )
    .expect("seals");
    assert_ne!(base.proof, other_stop.proof);

    let other_text = seal(
        vec![text("answer!")],
        StopReason::EndTurn,
        &NormalizedUsage::default(),
        &model(),
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
        &model(),
    )
    .expect("seals");
    assert_ne!(base.proof, other_usage.proof);

    let same = seal(
        vec![text("answer")],
        StopReason::EndTurn,
        &NormalizedUsage::default(),
        &model(),
    )
    .expect("seals");
    assert_eq!(base.proof, same.proof, "sealing is deterministic");
}

#[test]
fn s02_a_sealed_message_carries_its_provider_model_and_revision() {
    let selected = model();
    let sealed = seal(
        vec![text("answer")],
        StopReason::EndTurn,
        &NormalizedUsage::default(),
        &selected,
    )
    .expect("seals");
    assert_eq!(sealed.provider, selected.provider);
    assert_eq!(&sealed.model, &selected.model);
    assert_eq!(sealed.catalog, selected.catalog);
}

#[test]
fn s04_reasoning_tokens_are_always_a_subset_of_output_tokens() {
    let candidates = 120u64;
    let thoughts = 45u64;
    let candidate_based = NormalizedUsage {
        input_tokens: 10,
        output_tokens: candidates + thoughts,
        reasoning_tokens: thoughts,
        provider_total_tokens: Some(175),
        completeness: UsageCompleteness::Exact,
        ..NormalizedUsage::default()
    };
    assert!(candidate_based.reasoning_tokens <= candidate_based.output_tokens);
    assert_eq!(candidate_based.output_tokens, 165);

    let anthropic = NormalizedUsage {
        input_tokens: 10,
        output_tokens: 165,
        reasoning_tokens: 45,
        completeness: UsageCompleteness::Exact,
        ..NormalizedUsage::default()
    };
    assert_eq!(anthropic.output_tokens, candidate_based.output_tokens);
    assert_eq!(anthropic.reasoning_tokens, candidate_based.reasoning_tokens);
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
    let selected = model();
    let sealed = seal(
        vec![text("answer")],
        StopReason::EndTurn,
        &NormalizedUsage::default(),
        &selected,
    )
    .expect("seals");
    assert!(sealed.proof.0.to_wire().starts_with("sha256:"));
    let _: ContentHash = sealed.proof.0;
    assert!(selected.catalog.to_wire().starts_with("mc1_"));
}

#[test]
fn a_bounded_string_refuses_growth_past_its_bound() {
    let too_long = "x".repeat(1025);
    assert!(BoundedString::<1024>::new(too_long.clone()).is_err());
    assert_eq!(BoundedString::<1024>::truncating(&too_long).len(), 1024);
}

#[test]
fn the_unix_epoch_is_a_valid_wire_timestamp() {
    let _: Timestamp = Timestamp::from_unix_millis(0).expect("epoch is in range");
}
