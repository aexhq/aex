//! The provider-neutral canonical result vocabulary (plan 08 §4, D-CANON).
//!
//! This module is the cross-stream artefact every Brain stream reads. It lives
//! here because `aex-model-catalog` is pure — `serde`, `thiserror`, `time`,
//! `blake3` — so `aex-brain-domain` and `aex-brain-provider-gateway` can both
//! depend on it without acquiring Tokio, HTTP or AWS.
//!
//! `aex-brain-domain` depends on this crate and re-exports these canonical
//! types. There is one authority for provider request, result, usage, preview
//! and receipt vocabulary across the model catalog, Brain and gateway.
//!
//! Two separations are structural rather than conventional:
//!
//! - [`PreviewFrame`] has no `From`/`Into` into any canonical type, so a
//!   non-authoritative delta cannot become model-visible history.
//! - [`CompleteAssistantMessage`] can only be built by [`seal`], which requires
//!   a terminal [`StopReason`] and a validated block set.

use aex_wire::provider::ProviderId;
use aex_wire::types::Timestamp;
use aex_wire::{CanonicalJson, ContentHash, ResourceName, to_jcs_bytes};
use serde::{Deserialize, Serialize};

use crate::dialect::DialectClass;
use crate::primitives::{
    BoundedString, ModelSlug, ProviderRequestId, ToolCallId, ToolName, base64_bytes,
};
use crate::wire_pending::CatalogRevision;

/// Byte bound on a single canonical text block.
pub const TEXT_MAX: usize = 1_048_576;
/// Byte bound on a single canonical reasoning block.
pub const REASON_MAX: usize = 1_048_576;
/// Byte bound on one preview delta.
pub const PREVIEW_MAX: usize = 16_384;

// ---------------------------------------------------------------------------
// content
// ---------------------------------------------------------------------------

/// One block of provider-neutral content.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum CanonicalBlock {
    /// Assistant or user prose.
    Text {
        /// The text itself.
        text: BoundedString<TEXT_MAX>,
        /// Provider-supplied spans over `text`.
        annotations: Vec<TextAnnotation>,
    },
    /// Model reasoning, in whichever form the provider exposes.
    Reasoning(ReasoningBlock),
    /// A model request to run a tool.
    ToolUse {
        /// The provider-assigned call id, carried verbatim.
        id: ToolCallId,
        /// The tool name as declared to the provider.
        name: ToolName,
        /// Arguments, normalized to canonical JSON whatever the provider's
        /// encoding was.
        input: CanonicalJson,
    },
    /// The result of a tool run, carried on a user turn.
    ToolResult {
        /// The call this result answers.
        call: ToolCallId,
        /// The result body.
        content: Vec<ToolResultPart>,
        /// Whether the tool itself failed.
        is_error: bool,
    },
    /// An in-words refusal produced by the model.
    Refusal {
        /// The refusal text.
        text: BoundedString<1024>,
    },
}

/// A model reasoning block plus any opaque round-trip material.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReasoningBlock {
    /// What the provider exposed of the reasoning itself.
    pub body: ReasoningBody,
    /// Opaque material that must be replayed to the same provider.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token: Option<ReasoningToken>,
}

/// The three shapes provider reasoning arrives in.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ReasoningBody {
    /// Raw thinking text (`thinking`, `reasoning_content`).
    Text {
        /// The reasoning text.
        text: BoundedString<REASON_MAX>,
    },
    /// A provider-produced summary, not the reasoning itself.
    Summary {
        /// The summary text.
        text: BoundedString<REASON_MAX>,
    },
    /// Encrypted or omitted; the material lives entirely in the token.
    Redacted,
}

/// Opaque provider round-trip material: Anthropic `signature`, Gemini
/// `thoughtSignature`, `OpenAI` `reasoning.encrypted_content`, `DeepSeek` and
/// `Moonshot` echoed `reasoning_content`.
///
/// `provenance` exists so a token from one provider can never be replayed to
/// another (D-12).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReasoningToken {
    /// The provider that minted the material.
    pub provenance: ProviderId,
    /// The material, opaque to AEX.
    #[serde(with = "base64_bytes")]
    pub bytes: bytes::Bytes,
}

/// One part of a tool result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ToolResultPart {
    /// Plain text.
    Text {
        /// The text.
        text: BoundedString<TEXT_MAX>,
    },
    /// Structured JSON.
    Json {
        /// The value.
        value: CanonicalJson,
    },
}

/// A provider-supplied span over a text block.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TextAnnotation {
    /// Inclusive start offset, in bytes.
    pub start: u32,
    /// Exclusive end offset, in bytes.
    pub end: u32,
    /// What the span means.
    pub kind: AnnotationKind,
}

/// The closed annotation vocabulary.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AnnotationKind {
    /// A provider citation over the span.
    Citation,
    /// A provider-marked quotation of supplied context.
    Quote,
}

// ---------------------------------------------------------------------------
// messages
// ---------------------------------------------------------------------------

/// Message roles. Tool results are [`CanonicalBlock::ToolResult`] blocks on a
/// user turn, which is what every one of the eight dialects actually models.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    /// The caller's turn.
    User,
    /// The model's turn.
    Assistant,
}

/// One turn of provider-neutral conversation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CanonicalMessage {
    /// Whose turn it is.
    pub role: Role,
    /// The turn's blocks, in order.
    pub blocks: Vec<CanonicalBlock>,
}

/// A finished assistant turn, provable complete.
///
/// The only constructor is [`seal`]. Nothing derived from a [`PreviewFrame`]
/// has a path into this type.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CompleteAssistantMessage {
    /// The validated block set.
    pub blocks: Vec<CanonicalBlock>,
    /// Why the model stopped. Always terminal.
    pub stop_reason: StopReason,
    /// Which provider produced it.
    pub provider: ProviderId,
    /// Which model produced it.
    pub model: ModelSlug,
    /// The catalog revision in force.
    pub catalog: CatalogRevision,
    /// The completeness proof.
    pub proof: CompleteProof,
}

/// SHA-256 over the canonical rendering of blocks, stop reason, usage, provider,
/// model and catalog revision.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct CompleteProof(pub ContentHash);

impl CompleteAssistantMessage {
    /// Whether the completeness proof covers this exact message and usage.
    ///
    /// This is the fold-side verifier. It deliberately does not require a
    /// live [`QualifiedModel`]: committed history remains verifiable after the
    /// catalog revision is no longer active.
    #[must_use]
    pub fn proof_covers(&self, usage: &NormalizedUsage) -> bool {
        if !usage.is_consistent() {
            return false;
        }
        complete_proof_fields(
            &self.blocks,
            self.stop_reason,
            usage,
            self.provider,
            &self.model,
            self.catalog,
        )
        .is_ok_and(|proof| proof == self.proof)
    }

    /// Projects a complete assistant result into model-visible history.
    #[must_use]
    pub fn as_message(&self) -> CanonicalMessage {
        CanonicalMessage {
            role: Role::Assistant,
            blocks: self.blocks.clone(),
        }
    }
}

/// Why a model turn ended. Terminal only.
///
/// Content filtering, safety blocks, malformed function calls, context overflow
/// and mid-stream provider faults are **failures**, not stop reasons (D-09), so
/// a poisoned or empty terminal cannot enter model-visible history.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StopReason {
    /// The model finished its turn.
    EndTurn,
    /// The model asked for one or more tools.
    ToolUse,
    /// The output token ceiling was reached.
    MaxOutputTokens,
    /// A caller-supplied stop sequence matched.
    StopSequence,
    /// The model declined in words, having produced content.
    Refusal,
}

/// Why [`seal`] rejected a block set.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SealError {
    /// A terminal with no blocks at all.
    #[error("a sealed assistant message must carry at least one block")]
    EmptyBlocks,
    /// `stop_reason` is `ToolUse` with no tool use, or tool uses with a
    /// non-`ToolUse` stop reason.
    #[error("tool-use blocks and the stop reason disagree")]
    UnbalancedToolUse,
    /// Two tool uses share an id.
    #[error("duplicate tool call id `{0}`")]
    DuplicateToolCallId(ToolCallId),
    /// A refusal arrived as the only content.
    #[error("a refusal stop reason requires at least one non-refusal block")]
    RefusalWithoutContent,
    /// The model's catalog entry requires round-trip material that is absent.
    #[error("the entry requires reasoning round-trip material and none was captured")]
    ReasoningTokenMissing,
    /// A reasoning token from a different provider.
    #[error("reasoning token provenance `{found}` does not match `{expected}`")]
    ReasoningProvenanceMismatch {
        /// The provider that must have minted it.
        expected: ProviderId,
        /// The provider recorded on the token.
        found: ProviderId,
    },
    /// More blocks than the shared-safety bound allows.
    #[error("block count exceeds the {limit} bound")]
    BlockLimit {
        /// The bound.
        limit: usize,
    },
    /// A tool input that could not be canonicalized.
    #[error("tool input is not canonical JSON")]
    InvalidToolInputJson,
    /// Provider usage claimed more reasoning tokens than total output tokens.
    #[error("reasoning token usage exceeds total output token usage")]
    InconsistentUsage,
}

/// Shared-safety bound on the number of blocks in one sealed message.
pub const MAX_SEALED_BLOCKS: usize = 512;

/// The identity and policy facts [`seal`] needs from an admitted model handle.
///
/// Implemented by `aex-model-catalog`'s `QualifiedModel` over the generated
/// admit table. A trait rather than a concrete parameter keeps this crate
/// free of catalog data while `seal` stays in the canonical vocabulary.
pub trait SealModel {
    /// The provider.
    fn provider(&self) -> ProviderId;

    /// The exact provider-native model id.
    fn model(&self) -> &ModelSlug;

    /// The catalog revision in force.
    fn catalog(&self) -> CatalogRevision;

    /// Whether a sealed turn must carry reasoning round-trip material.
    fn requires_reasoning_token(&self, has_tool_use: bool) -> bool;
}

/// Builds a [`CompleteAssistantMessage`], enforcing every completeness rule.
///
/// # Errors
///
/// Returns [`SealError`] for an empty, unbalanced, duplicated, refusal-only or
/// over-large block set, or for reasoning material that is missing or came from
/// the wrong provider.
pub fn seal(
    blocks: Vec<CanonicalBlock>,
    stop: StopReason,
    usage: &NormalizedUsage,
    model: &impl SealModel,
) -> Result<CompleteAssistantMessage, SealError> {
    if !usage.is_consistent() {
        return Err(SealError::InconsistentUsage);
    }
    if blocks.is_empty() {
        return Err(SealError::EmptyBlocks);
    }
    if blocks.len() > MAX_SEALED_BLOCKS {
        return Err(SealError::BlockLimit {
            limit: MAX_SEALED_BLOCKS,
        });
    }

    let mut tool_ids: Vec<&ToolCallId> = Vec::new();
    let mut non_refusal = 0usize;
    let mut has_refusal = false;
    let mut reasoning_blocks = 0usize;
    let mut reasoning_tokens = 0usize;

    for block in &blocks {
        match block {
            CanonicalBlock::ToolUse { id, .. } => {
                if tool_ids.contains(&id) {
                    return Err(SealError::DuplicateToolCallId(id.clone()));
                }
                tool_ids.push(id);
                non_refusal += 1;
            }
            CanonicalBlock::Refusal { .. } => has_refusal = true,
            CanonicalBlock::Reasoning(reasoning) => {
                reasoning_blocks += 1;
                if let Some(token) = &reasoning.token {
                    if token.provenance != model.provider() {
                        return Err(SealError::ReasoningProvenanceMismatch {
                            expected: model.provider(),
                            found: token.provenance,
                        });
                    }
                    reasoning_tokens += 1;
                }
                non_refusal += 1;
            }
            CanonicalBlock::Text { .. } | CanonicalBlock::ToolResult { .. } => non_refusal += 1,
        }
    }

    let has_tool_use = !tool_ids.is_empty();
    if has_tool_use != matches!(stop, StopReason::ToolUse) {
        return Err(SealError::UnbalancedToolUse);
    }
    if has_refusal && non_refusal == 0 && !matches!(stop, StopReason::Refusal) {
        return Err(SealError::RefusalWithoutContent);
    }
    if matches!(stop, StopReason::Refusal) && non_refusal == 0 {
        return Err(SealError::RefusalWithoutContent);
    }

    if reasoning_blocks > 0 && model.requires_reasoning_token(has_tool_use) && reasoning_tokens == 0
    {
        return Err(SealError::ReasoningTokenMissing);
    }

    let proof = complete_proof(&blocks, stop, usage, model)?;
    Ok(CompleteAssistantMessage {
        blocks,
        stop_reason: stop,
        provider: model.provider(),
        model: model.model().clone(),
        catalog: model.catalog(),
        proof,
    })
}

#[derive(Serialize)]
struct ProofInput<'a> {
    blocks: &'a [CanonicalBlock],
    catalog: CatalogRevision,
    model: &'a ModelSlug,
    provider: ProviderId,
    stop_reason: StopReason,
    usage: &'a NormalizedUsage,
}

fn complete_proof(
    blocks: &[CanonicalBlock],
    stop: StopReason,
    usage: &NormalizedUsage,
    model: &impl SealModel,
) -> Result<CompleteProof, SealError> {
    complete_proof_fields(
        blocks,
        stop,
        usage,
        model.provider(),
        model.model(),
        model.catalog(),
    )
}

fn complete_proof_fields(
    blocks: &[CanonicalBlock],
    stop: StopReason,
    usage: &NormalizedUsage,
    provider: ProviderId,
    model: &ModelSlug,
    catalog: CatalogRevision,
) -> Result<CompleteProof, SealError> {
    let input = ProofInput {
        blocks,
        catalog,
        model,
        provider,
        stop_reason: stop,
        usage,
    };
    // A canonicalization failure is refused rather than absorbed: a default
    // digest would make two different block sets share a proof, which is
    // exactly what the proof exists to prevent.
    let bytes = to_jcs_bytes(&input).map_err(|_| SealError::InvalidToolInputJson)?;
    Ok(CompleteProof(ContentHash::of(&bytes)))
}

// ---------------------------------------------------------------------------
// usage
// ---------------------------------------------------------------------------

/// Provider-neutral token accounting.
///
/// `output_tokens` **always** includes reasoning tokens and `reasoning_tokens`
/// is a subset of it (D-11). Providers disagree — Gemini documents
/// `thoughtsTokenCount` as excluded from `candidatesTokenCount` — so the
/// catalog entry's `reasoning_included_in_output` flag tells the adapter
/// whether to add, and one arithmetic definition reaches every consumer.
///
/// Under BYOK these are zero-dollar observability facts (A11-PRICING). This
/// type never carries money.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NormalizedUsage {
    /// Uncached prompt tokens billed as input.
    pub input_tokens: u64,
    /// Prompt tokens served from the provider's cache.
    pub cache_read_input_tokens: u64,
    /// Prompt tokens written into the provider's cache.
    pub cache_write_input_tokens: u64,
    /// Generated tokens, always including reasoning.
    pub output_tokens: u64,
    /// The reasoning subset of `output_tokens`.
    pub reasoning_tokens: u64,
    /// Tokens the provider attributes to tool-use prompting.
    pub tool_use_prompt_tokens: u64,
    /// The provider's own total, unmodified, where it publishes one.
    pub provider_total_tokens: Option<u64>,
    /// How much of the above the provider actually reported.
    pub completeness: UsageCompleteness,
}

impl NormalizedUsage {
    /// Whether the invariant "reasoning is a subset of output" holds.
    #[must_use]
    pub const fn is_consistent(&self) -> bool {
        self.reasoning_tokens <= self.output_tokens
    }

    /// Prompt tokens occupying the provider context window.
    #[must_use]
    pub const fn prompt_tokens(&self) -> u64 {
        self.input_tokens
            .saturating_add(self.cache_read_input_tokens)
            .saturating_add(self.cache_write_input_tokens)
    }

    /// Saturating accumulation for fold-level observability totals.
    pub const fn add(&mut self, other: Self) {
        self.input_tokens = self.input_tokens.saturating_add(other.input_tokens);
        self.cache_read_input_tokens = self
            .cache_read_input_tokens
            .saturating_add(other.cache_read_input_tokens);
        self.cache_write_input_tokens = self
            .cache_write_input_tokens
            .saturating_add(other.cache_write_input_tokens);
        self.output_tokens = self.output_tokens.saturating_add(other.output_tokens);
        self.reasoning_tokens = self.reasoning_tokens.saturating_add(other.reasoning_tokens);
        self.tool_use_prompt_tokens = self
            .tool_use_prompt_tokens
            .saturating_add(other.tool_use_prompt_tokens);
        self.provider_total_tokens = match (self.provider_total_tokens, other.provider_total_tokens)
        {
            (Some(left), Some(right)) => Some(left.saturating_add(right)),
            _ => None,
        };
        self.completeness = self.completeness.accumulated(other.completeness);
    }
}

/// How complete a [`NormalizedUsage`] is. Usage is never invented: an absent
/// field is recorded as absent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum UsageCompleteness {
    /// Every field the catalog entry declares present arrived.
    #[default]
    Exact,
    /// Some declared fields were missing.
    Partial {
        /// Which fields were missing.
        missing: UsageFieldSet,
    },
    /// No usage arrived at all.
    Absent,
}

impl UsageCompleteness {
    const fn accumulated(self, other: Self) -> Self {
        match (self, other) {
            (Self::Exact, Self::Exact) => Self::Exact,
            (Self::Partial { missing: left }, Self::Partial { missing: right }) => Self::Partial {
                missing: UsageFieldSet(left.0 | right.0),
            },
            (Self::Partial { missing }, Self::Exact) | (Self::Exact, Self::Partial { missing }) => {
                Self::Partial { missing }
            }
            (Self::Absent, _) | (_, Self::Absent) => Self::Absent,
        }
    }
}

/// A bit set over the usage fields a provider may fail to report.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Default, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
#[serde(transparent)]
pub struct UsageFieldSet(pub u16);

/// One reportable usage field.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum UsageField {
    /// `input_tokens`.
    InputTokens,
    /// `cache_read_input_tokens`.
    CacheReadInputTokens,
    /// `cache_write_input_tokens`.
    CacheWriteInputTokens,
    /// `output_tokens`.
    OutputTokens,
    /// `reasoning_tokens`.
    ReasoningTokens,
    /// `tool_use_prompt_tokens`.
    ToolUsePromptTokens,
    /// `provider_total_tokens`.
    ProviderTotalTokens,
}

impl UsageField {
    /// Every field, for exhaustive tests.
    pub const ALL: [Self; 7] = [
        Self::InputTokens,
        Self::CacheReadInputTokens,
        Self::CacheWriteInputTokens,
        Self::OutputTokens,
        Self::ReasoningTokens,
        Self::ToolUsePromptTokens,
        Self::ProviderTotalTokens,
    ];

    const fn bit(self) -> u16 {
        match self {
            Self::InputTokens => 1 << 0,
            Self::CacheReadInputTokens => 1 << 1,
            Self::CacheWriteInputTokens => 1 << 2,
            Self::OutputTokens => 1 << 3,
            Self::ReasoningTokens => 1 << 4,
            Self::ToolUsePromptTokens => 1 << 5,
            Self::ProviderTotalTokens => 1 << 6,
        }
    }
}

impl UsageFieldSet {
    /// The empty set.
    pub const EMPTY: Self = Self(0);

    /// Adds a field.
    #[must_use]
    pub const fn with(self, field: UsageField) -> Self {
        Self(self.0 | field.bit())
    }

    /// Whether a field is present.
    #[must_use]
    pub const fn contains(self, field: UsageField) -> bool {
        self.0 & field.bit() != 0
    }

    /// Whether the set is empty.
    #[must_use]
    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }
}

// ---------------------------------------------------------------------------
// request
// ---------------------------------------------------------------------------

/// One system-instruction block.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SystemBlock {
    /// The instruction text.
    pub text: BoundedString<TEXT_MAX>,
    /// Whether this block ends a cacheable prefix.
    pub cacheable: bool,
}

/// How the model must choose among declared tools.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ToolChoice {
    /// The model decides.
    Auto,
    /// No tool may be called.
    None,
    /// Some tool must be called.
    Required,
    /// This exact tool must be called.
    Named {
        /// The tool name.
        name: ToolName,
    },
}

/// A tool declaration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CanonicalToolDef {
    /// The tool name.
    pub name: ToolName,
    /// What the tool does.
    pub description: BoundedString<1024>,
    /// The JSON Schema for its input.
    pub input_schema: CanonicalJson,
    /// Whether the provider must enforce the schema strictly.
    pub strict: bool,
}

/// Whether and how the model should reason.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ReasoningRequest {
    /// Explicitly off.
    Disabled,
    /// Send nothing; take whatever the provider does by default.
    ProviderDefault,
    /// Explicitly on.
    Enabled {
        /// A token budget, where the provider takes one.
        budget_tokens: Option<u32>,
        /// An effort level, where the provider takes one.
        effort: Option<ReasoningEffort>,
    },
}

/// The neutral effort ladder. Each adapter maps it onto its provider's own set.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReasoningEffort {
    /// The least the provider offers above off.
    Minimal,
    /// Low.
    Low,
    /// Medium.
    Medium,
    /// High.
    High,
    /// The most the provider offers.
    Max,
}

/// A structured-output request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum StructuredOutputRequest {
    /// Any JSON object.
    JsonObject,
    /// A named JSON Schema.
    JsonSchema {
        /// The schema name the provider records.
        name: ResourceName,
        /// The schema.
        schema: CanonicalJson,
        /// Whether the provider must enforce it strictly.
        strict: bool,
    },
}

/// An explicit prompt-cache breakpoint, expressed against the request's own
/// structure rather than a provider field name.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum CacheBreakpoint {
    /// After the system blocks.
    AfterSystem,
    /// After the tool declarations.
    AfterTools,
    /// After the message at this index.
    AfterMessage {
        /// Zero-based index into `messages`.
        index: u16,
    },
}

/// A non-secret correlation handle. Derived from the effect id, never from
/// customer text and never from a credential.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct CorrelationId(pub BoundedString<64>);

impl CorrelationId {
    /// Builds the `aex-<hex>` rendering used as Z.AI's `request_id` (D-23).
    ///
    /// # Panics
    ///
    /// Never: the rendered length is a compile-time constant well under the
    /// 64-byte bound.
    #[must_use]
    pub fn from_effect(effect: [u8; 16]) -> Self {
        let rendered = format!("aex-{}", hex::encode(effect));
        Self(BoundedString::new(rendered).expect("aex- plus 32 hex is 36 bytes"))
    }

    /// The borrowed contents.
    #[must_use]
    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }
}

// ---------------------------------------------------------------------------
// preview (non-authoritative)
// ---------------------------------------------------------------------------

/// A non-authoritative streaming delta.
///
/// There is deliberately no conversion between this type and any canonical
/// type, in either direction: a preview delta must never become history.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "frame", rename_all = "snake_case", deny_unknown_fields)]
pub enum PreviewFrame {
    /// A block opened.
    BlockStart {
        /// Block index within the turn.
        index: u16,
        /// What kind of block opened.
        kind: PreviewBlockKind,
    },
    /// Text arrived.
    TextDelta {
        /// Block index.
        index: u16,
        /// The fragment.
        text: BoundedString<PREVIEW_MAX>,
    },
    /// Reasoning text arrived.
    ReasoningDelta {
        /// Block index.
        index: u16,
        /// The fragment.
        text: BoundedString<PREVIEW_MAX>,
    },
    /// A tool call opened.
    ToolCallStart {
        /// Block index.
        index: u16,
        /// The provider call id.
        id: ToolCallId,
        /// The tool name.
        name: ToolName,
    },
    /// A fragment of tool arguments arrived.
    ToolArgumentsDelta {
        /// Block index.
        index: u16,
        /// The raw fragment, which is generally not valid JSON on its own.
        fragment: BoundedString<PREVIEW_MAX>,
    },
    /// A block closed.
    BlockStop {
        /// Block index.
        index: u16,
    },
    /// Usage observed before the turn ended.
    InterimUsage(NormalizedUsage),
}

/// What kind of block a preview opened.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PreviewBlockKind {
    /// Prose.
    Text,
    /// Reasoning.
    Reasoning,
    /// A tool call.
    ToolUse,
    /// A refusal.
    Refusal,
}

// ---------------------------------------------------------------------------
// receipt
// ---------------------------------------------------------------------------

/// A binding reference: identity and generation only, never material.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CredentialBindingRef {
    /// The stable `pcr_` id.
    pub id: aex_wire::ids::ProviderCredentialId,
    /// The immutable binding revision.
    pub revision: u64,
    /// The `aex-secret-domain` source generation.
    pub generation: u64,
}

/// Bounded routing metadata reported by a customer-selected gateway.
///
/// The outer receipt remains authoritative for the requested gateway and model.
/// This record captures only what the gateway reported about the route it
/// actually used; vendor metadata objects are never copied into the journal.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GatewayRoute {
    /// The model identifier reported in the gateway response, when present.
    pub reported_model: Option<ModelSlug>,
    /// The upstream provider label reported by the gateway, when present.
    pub reported_provider: Option<BoundedString<64>>,
}

/// The durable evidence one dispatch produced.
///
/// The transport authority is rig, which does not publish byte counts or
/// stream bounds; those configuration facts died with the hand-rolled
/// transport (model-provider simplification 2026-08-13). Token counts on
/// [`NormalizedUsage`] are zero-dollar BYOK facts and are not on this type.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProviderReceipt {
    /// Which provider was called.
    pub provider: ProviderId,
    /// Which model was called.
    pub model: ModelSlug,
    /// The catalog revision in force.
    pub catalog: CatalogRevision,
    /// The dialect class used.
    pub dialect: DialectClass,
    /// Which credential binding was used. Identity only.
    pub credential: CredentialBindingRef,
    /// The provider's own request id, where one publishes one.
    pub provider_request_id: Option<ProviderRequestId>,
    /// Bounded actual-route metadata for a gateway dispatch, otherwise absent.
    pub gateway_route: Option<GatewayRoute>,
    /// The final HTTP status.
    pub http_status: u16,
    /// How many attempts the in-call retry policy used.
    pub attempts: u16,
    /// When the dispatch began.
    pub started_at: Timestamp,
    /// When the first decoded dialect frame arrived.
    pub first_frame_at: Option<Timestamp>,
    /// When the dispatch settled.
    pub completed_at: Timestamp,
    /// Rate-limit feedback the provider published, if any.
    pub rate_limit: Option<ReceiptRateLimit>,
    /// A hash of the sealed response, where one was produced.
    pub response_receipt: Option<ContentHash>,
}

impl ProviderReceipt {
    /// Whether this receipt identifies and commits to exactly one successful
    /// canonical outcome.
    #[must_use]
    pub fn matches_outcome(
        &self,
        message: &CompleteAssistantMessage,
        usage: &NormalizedUsage,
    ) -> bool {
        let first_frame_is_ordered = self
            .first_frame_at
            .is_some_and(|first| first >= self.started_at && first <= self.completed_at);
        (200..300).contains(&self.http_status)
            && self.attempts > 0
            && self.completed_at >= self.started_at
            && first_frame_is_ordered
            && self.dialect.provider() == self.provider
            && self.provider == message.provider
            && self.model == message.model
            && self.catalog == message.catalog
            && self.response_receipt == Some(message.proof.0)
            && message.proof_covers(usage)
    }
}

/// Rate-limit feedback as recorded on a receipt.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReceiptRateLimit {
    /// Seconds the provider asked the caller to wait.
    pub retry_after_ms: Option<u64>,
    /// Requests left in the window.
    pub requests_remaining: Option<u32>,
    /// Tokens left in the window.
    pub tokens_remaining: Option<u64>,
    /// When the window resets.
    pub reset_at: Option<Timestamp>,
    /// Where the feedback came from.
    pub source: RateLimitSource,
}

/// Where rate-limit feedback came from. `NotProvided` is a positive record that
/// the provider publishes none, not an absence of observation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RateLimitSource {
    /// A standard `Retry-After` header.
    RetryAfterHeader,
    /// Vendor-specific `x-ratelimit-*` headers.
    VendorHeaders,
    /// A code or delay inside the error body.
    ErrorBody,
    /// The provider publishes no rate-limit feedback at all.
    #[default]
    NotProvided,
}
