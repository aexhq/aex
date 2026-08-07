//! The catalog document schema (plan 08 §2.2).
//!
//! Every struct is `#[serde(deny_unknown_fields)]` and every string enum is
//! closed. An unknown field or an unknown enum member is a **load failure**,
//! never an ignored value — that is the "unknown capability" property of
//! BYOK-02 (D-04).

use aex_wire::provider::ProviderId;
use aex_wire::types::Timestamp;
use serde::{Deserialize, Serialize};

use crate::failure::ProviderFailureKind;
use crate::primitives::{Blake3Digest, BoundedString, ModelSlug};
use crate::receipt::{ConformanceReceipt, ProbeSuiteRevision};

/// The document schema version this crate understands. There is exactly one.
pub const SCHEMA_VERSION: u16 = 1;

/// blake3-256 of the canonical document bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct CatalogDigest(pub Blake3Digest);

/// A strictly increasing sequence per publisher.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct CatalogSequence(pub u64);

/// Who published a catalog release, for example `aex-catalog-prd`.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct PublisherId(pub BoundedString<32>);

/// blake3 of the adapter crate source tree that a receipt was earned against.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct AdapterSourceDigest(pub Blake3Digest);

/// A dialect's revision. Bumping it is a code release, never a data release.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct DialectRevision(pub u16);

/// The immutable signed catalog release.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CatalogDocument {
    /// Always [`SCHEMA_VERSION`]; anything else fails load.
    pub schema_version: u16,
    /// Who published this release.
    pub publisher: PublisherId,
    /// Strictly increasing per publisher.
    pub sequence: CatalogSequence,
    /// The digest of the release this one succeeds.
    pub predecessor: Option<CatalogDigest>,
    /// When the release was cut.
    pub issued_at: Timestamp,
    /// The document is inactive before this instant.
    pub not_before: Timestamp,
    /// No **new** session admission after this instant.
    pub expires_at: Timestamp,
    /// The document does not load at all after this instant.
    pub retired_at: Timestamp,
    /// Which probe suite the embedded receipts were earned against.
    pub probe_suite_revision: ProbeSuiteRevision,
    /// Which adapter source tree the embedded receipts were earned against.
    pub required_adapter_source: AdapterSourceDigest,
    /// Sorted by `(provider, model)`, unique.
    pub entries: Vec<ModelEntry>,
    /// Sorted by `(provider, model)`, unique.
    pub emergency_disable: Vec<EmergencyDisable>,
}

/// One `(provider, model)` pair and everything the adapter needs to speak to it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModelEntry {
    /// The provider half of the pair.
    pub provider: ProviderId,
    /// The exact provider-native model id.
    pub model: ModelSlug,
    /// Whether the pair is admissible.
    pub state: EntryState,
    /// Which wire dialect the adapter must speak.
    pub dialect: Dialect,
    /// The dialect revision.
    pub dialect_revision: DialectRevision,
    /// The compiled origin. There is no free-form base URL anywhere.
    pub endpoint: EndpointPin,
    /// What the pair can do.
    pub capabilities: CapabilitySet,
    /// Numeric bounds.
    pub limits: ModelLimits,
    /// Which sampling controls the pair honours.
    pub sampling: SamplingSupport,
    /// Reasoning behaviour and round-trip requirements.
    pub reasoning: ReasoningPolicy,
    /// Structured-output support and encoding.
    pub structured_output: StructuredOutputPolicy,
    /// Tool encoding and naming rules.
    pub tool_policy: ToolPolicy,
    /// Prompt-cache behaviour.
    pub cache_policy: CachePolicy,
    /// How provider usage maps onto [`crate::canonical::NormalizedUsage`].
    pub usage_map: UsageMapping,
    /// How provider finish tokens map onto [`crate::canonical::StopReason`].
    pub stop_reason_map: StopReasonMap,
    /// How provider statuses and codes map onto [`ProviderFailureKind`].
    pub error_map: ErrorClassMap,
    /// The in-call retry policy, restricted by D-20.
    pub retry_policy: PreDispatchRetryPolicy,
    /// Whether the provider offers a durable result lookup. `None` for all eight
    /// at launch.
    pub durable_operation: DurableOperationSupport,
    /// The provider-published account concurrency, carried as a hint.
    pub concurrency_hint: u32,
    /// An opaque pricing-context reference. Values stay synthetic or zero in
    /// public source (OD-09); model tokens are zero-dollar BYOK facts.
    pub pricing_context: Option<PricingContextRef>,
    /// The live conformance receipt for this exact entry.
    pub receipt: ConformanceReceipt,
}

/// Whether an entry is admissible.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EntryState {
    /// Present, described, and **not** admissible: no live receipt has earned
    /// it yet. Every entry ships in this state (OD-24).
    Staged,
    /// Admissible.
    Active,
    /// Present and readable for committed history, not admissible for new runs.
    Deprecated,
}

/// The wire dialect an adapter speaks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Dialect {
    /// `OpenAI` Responses.
    OpenAiResponses,
    /// Anthropic Messages.
    AnthropicMessages,
    /// `DeepSeek` chat completions.
    DeepSeekChat,
    /// Z.AI chat completions.
    ZaiChat,
    /// `Moonshot` chat completions.
    MoonshotChat,
    /// Gemini `streamGenerateContent`.
    GeminiGenerateContent,
    /// OpenRouter's OpenAI-compatible chat-completions surface.
    OpenRouterChat,
    /// Vercel AI Gateway's OpenAI-compatible chat-completions surface.
    VercelAiGatewayChat,
    /// Reserved and unimplemented. Adding a dialect arm is a code release
    /// (D-19); an entry declaring this dialect fails load.
    GeminiInteractions,
}

impl Dialect {
    /// The provider that owns the dialect.
    #[must_use]
    pub const fn provider(self) -> ProviderId {
        match self {
            Self::OpenAiResponses => ProviderId::Openai,
            Self::AnthropicMessages => ProviderId::Anthropic,
            Self::DeepSeekChat => ProviderId::Deepseek,
            Self::ZaiChat => ProviderId::Zai,
            Self::MoonshotChat => ProviderId::Moonshotai,
            Self::GeminiGenerateContent | Self::GeminiInteractions => ProviderId::Google,
            Self::OpenRouterChat => ProviderId::Openrouter,
            Self::VercelAiGatewayChat => ProviderId::VercelAiGateway,
        }
    }

    /// Whether an adapter for this dialect exists in the running binary.
    #[must_use]
    pub const fn is_implemented(self) -> bool {
        !matches!(self, Self::GeminiInteractions)
    }
}

/// The closed origin set. There is no free-form base URL anywhere in the crate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EndpointPin {
    /// `https://api.openai.com`.
    OpenAiApi,
    /// `https://api.anthropic.com`.
    AnthropicApi,
    /// `https://api.deepseek.com`.
    DeepSeekApi,
    /// `https://api.z.ai` — the general API only. The Coding-Plan and Anthropic
    /// compatible bases are launch exclusions and are unreachable from
    /// configuration.
    ZaiPaasV4,
    /// `https://api.moonshot.ai` — the international host. `api.moonshot.cn` is
    /// a launch exclusion and `platform.kimi.ai` is a docs host, never an API
    /// origin.
    MoonshotIntlV1,
    /// `https://generativelanguage.googleapis.com`.
    GeminiV1Beta,
    /// `https://openrouter.ai`.
    OpenRouterApiV1,
    /// `https://ai-gateway.vercel.sh`.
    VercelAiGatewayV1,
}

impl EndpointPin {
    /// Every pinned origin, for exhaustive tests.
    pub const ALL: [Self; 8] = [
        Self::OpenAiApi,
        Self::AnthropicApi,
        Self::DeepSeekApi,
        Self::ZaiPaasV4,
        Self::MoonshotIntlV1,
        Self::GeminiV1Beta,
        Self::OpenRouterApiV1,
        Self::VercelAiGatewayV1,
    ];

    /// The compiled origin.
    #[must_use]
    pub const fn origin(self) -> &'static str {
        match self {
            Self::OpenAiApi => "https://api.openai.com",
            Self::AnthropicApi => "https://api.anthropic.com",
            Self::DeepSeekApi => "https://api.deepseek.com",
            Self::ZaiPaasV4 => "https://api.z.ai",
            Self::MoonshotIntlV1 => "https://api.moonshot.ai",
            Self::GeminiV1Beta => "https://generativelanguage.googleapis.com",
            Self::OpenRouterApiV1 => "https://openrouter.ai",
            Self::VercelAiGatewayV1 => "https://ai-gateway.vercel.sh",
        }
    }

    /// The provider that owns the origin.
    #[must_use]
    pub const fn provider(self) -> ProviderId {
        match self {
            Self::OpenAiApi => ProviderId::Openai,
            Self::AnthropicApi => ProviderId::Anthropic,
            Self::DeepSeekApi => ProviderId::Deepseek,
            Self::ZaiPaasV4 => ProviderId::Zai,
            Self::MoonshotIntlV1 => ProviderId::Moonshotai,
            Self::GeminiV1Beta => ProviderId::Google,
            Self::OpenRouterApiV1 => ProviderId::Openrouter,
            Self::VercelAiGatewayV1 => ProviderId::VercelAiGateway,
        }
    }
}

/// A bit set over [`Capability`].
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default, Serialize, Deserialize,
)]
#[serde(transparent)]
pub struct CapabilitySet(pub u32);

/// One declared model capability.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Capability {
    /// Text input.
    TextIn,
    /// Text output.
    TextOut,
    /// Server-sent-event streaming.
    Streaming,
    /// Tool calling.
    Tools,
    /// More than one tool call per turn.
    ParallelTools,
    /// `tool_choice` "some tool must be called".
    ToolChoiceRequired,
    /// `tool_choice` "this exact tool".
    ToolChoiceNamed,
    /// `tool_choice` "no tool".
    ToolChoiceNone,
    /// Strict tool-schema enforcement.
    StrictToolSchema,
    /// Schema-constrained output.
    StructuredOutput,
    /// Reasoning.
    Reasoning,
    /// Reasoning round-trip material.
    ReasoningReplay,
    /// Caller-placed prompt-cache breakpoints.
    PromptCacheExplicit,
    /// Provider-managed prompt caching.
    PromptCacheImplicit,
    /// Caller stop sequences.
    StopSequences,
    /// Temperature.
    Temperature,
    /// Nucleus sampling.
    TopP,
    /// A dedicated system-instruction channel.
    SystemInstruction,
    /// Image input.
    ImageIn,
}

impl Capability {
    /// Every capability, for exhaustive tests and the probe requirement table.
    pub const ALL: [Self; 19] = [
        Self::TextIn,
        Self::TextOut,
        Self::Streaming,
        Self::Tools,
        Self::ParallelTools,
        Self::ToolChoiceRequired,
        Self::ToolChoiceNamed,
        Self::ToolChoiceNone,
        Self::StrictToolSchema,
        Self::StructuredOutput,
        Self::Reasoning,
        Self::ReasoningReplay,
        Self::PromptCacheExplicit,
        Self::PromptCacheImplicit,
        Self::StopSequences,
        Self::Temperature,
        Self::TopP,
        Self::SystemInstruction,
        Self::ImageIn,
    ];

    const fn bit(self) -> u32 {
        1u32 << (self as u32)
    }

    /// The wire token, used in load-error detail.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::TextIn => "text_in",
            Self::TextOut => "text_out",
            Self::Streaming => "streaming",
            Self::Tools => "tools",
            Self::ParallelTools => "parallel_tools",
            Self::ToolChoiceRequired => "tool_choice_required",
            Self::ToolChoiceNamed => "tool_choice_named",
            Self::ToolChoiceNone => "tool_choice_none",
            Self::StrictToolSchema => "strict_tool_schema",
            Self::StructuredOutput => "structured_output",
            Self::Reasoning => "reasoning",
            Self::ReasoningReplay => "reasoning_replay",
            Self::PromptCacheExplicit => "prompt_cache_explicit",
            Self::PromptCacheImplicit => "prompt_cache_implicit",
            Self::StopSequences => "stop_sequences",
            Self::Temperature => "temperature",
            Self::TopP => "top_p",
            Self::SystemInstruction => "system_instruction",
            Self::ImageIn => "image_in",
        }
    }
}

impl CapabilitySet {
    /// The empty set.
    pub const EMPTY: Self = Self(0);

    /// Builds a set from a slice.
    #[must_use]
    pub const fn from_slice(items: &[Capability]) -> Self {
        let mut bits = 0u32;
        let mut index = 0;
        while index < items.len() {
            bits |= items[index].bit();
            index += 1;
        }
        Self(bits)
    }

    /// Whether the capability is declared.
    #[must_use]
    pub const fn has(self, capability: Capability) -> bool {
        self.0 & capability.bit() != 0
    }

    /// Adds a capability.
    #[must_use]
    pub const fn with(self, capability: Capability) -> Self {
        Self(self.0 | capability.bit())
    }

    /// Every declared capability, in [`Capability::ALL`] order.
    #[must_use]
    pub fn declared(self) -> Vec<Capability> {
        Capability::ALL
            .into_iter()
            .filter(|c| self.has(*c))
            .collect()
    }

    /// Whether the set contains a bit outside [`Capability::ALL`], which means
    /// the document was produced by a newer publisher than this binary.
    #[must_use]
    pub fn has_unknown_bits(self) -> bool {
        let known = Capability::ALL
            .into_iter()
            .fold(0u32, |acc, c| acc | c.bit());
        self.0 & !known != 0
    }
}

/// Numeric bounds for a pair. Integer milli-units everywhere; no floats.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModelLimits {
    /// Total context window in tokens.
    pub context_window_tokens: u32,
    /// Maximum generated tokens.
    pub max_output_tokens: u32,
    /// Minimum generated tokens the provider accepts (`OpenAI` Responses: 16).
    pub min_output_tokens: u32,
    /// Maximum reasoning tokens, where the provider takes a budget.
    pub max_reasoning_tokens: Option<u32>,
    /// Minimum reasoning budget (Anthropic thinking: 1024).
    pub min_reasoning_tokens: Option<u32>,
    /// Maximum tool declarations (`DeepSeek`: 128).
    pub max_tools: u16,
    /// Maximum stop sequences (Z.AI 4, `Moonshot` 5, `DeepSeek` 16).
    pub max_stop_sequences: u8,
    /// Inclusive temperature range in milli-units.
    pub temperature_milli: Option<(u16, u16)>,
    /// Inclusive `top_p` range in milli-units.
    pub top_p_milli: Option<(u16, u16)>,
    /// The shortest prefix the provider will cache.
    pub min_cacheable_prefix_tokens: u32,
    /// Maximum request body (Anthropic documents 32 MiB).
    pub request_body_max_bytes: u32,
    /// Maximum bytes in one response frame.
    pub response_frame_max_bytes: u32,
    /// Milliseconds permitted between frames.
    pub stream_idle_timeout_ms: u32,
    /// Milliseconds permitted for the whole stream.
    pub total_stream_deadline_ms: u32,
}

/// Which sampling controls a pair honours.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SamplingSupport {
    /// None: sending one is a pre-dispatch rejection, not a silently ignored
    /// parameter.
    None,
    /// Temperature only.
    TemperatureOnly,
    /// Temperature and `top_p`.
    Full,
}

/// Reasoning behaviour for a pair.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReasoningPolicy {
    /// Whether reasoning is unsupported, optional or always on.
    pub mode: ReasoningMode,
    /// How the provider encodes it.
    pub encoding: ReasoningEncoding,
    /// Whether round-trip material must be echoed.
    pub replay: ReasoningReplay,
    /// `DeepSeek`'s thinking mode forbids `temperature`/`top_p`.
    pub excludes_sampling: bool,
}

/// Whether reasoning is available.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReasoningMode {
    /// Not available.
    Unsupported,
    /// Available and controllable.
    Optional,
    /// Always on and not disableable (Gemini 2.5 Pro).
    AlwaysOn,
}

/// How a provider encodes reasoning on the wire.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReasoningEncoding {
    /// No reasoning encoding.
    None,
    /// Anthropic `thinking` blocks plus a mandatory `signature`.
    AnthropicThinking,
    /// `OpenAI` `reasoning` items and `reasoning.encrypted_content`.
    OpenAiReasoning,
    /// `DeepSeek` `reasoning_content`.
    DeepSeekThinking,
    /// Z.AI `reasoning_content` under a `thinking` request object.
    ZaiThinking,
    /// `Moonshot` `reasoning_content`.
    MoonshotThinking,
    /// Gemini `thinkingConfig` plus `part.thought` and `part.thoughtSignature`.
    GeminiThinkingConfig,
}

/// Whether round-trip reasoning material must be echoed on the next turn.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReasoningReplay {
    /// Nothing to echo.
    NotRequired,
    /// Echoing improves results but omitting it is accepted.
    RecommendedEcho,
    /// The provider rejects a turn that carried tool calls without the
    /// material (Gemini 3 `thoughtSignature`, `DeepSeek` `reasoning_content`).
    RequiredWithToolCalls,
    /// The provider rejects any turn without it (Anthropic `signature`).
    RequiredAlways,
}

/// Structured-output support and encoding.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum StructuredOutputPolicy {
    /// Not supported; a structured-output request fails before dispatch.
    Unsupported,
    /// `{"type":"json_object"}` only; a schema request fails before dispatch.
    JsonObjectOnly,
    /// A real schema, in the named encoding.
    JsonSchema {
        /// Which request encoding the provider takes.
        encoding: SchemaEncoding,
        /// Whether `strict` defaults to true for this provider.
        strict_default: bool,
    },
}

/// The request encoding a provider uses for an output schema.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SchemaEncoding {
    /// `OpenAI` `text.format = {"type":"json_schema", …}`.
    OpenAiTextFormat,
    /// Anthropic `output_config.format`.
    AnthropicOutputConfig,
    /// `Moonshot` `response_format = {"type":"json_schema", …}`.
    MoonshotResponseFormat,
    /// Gemini flat `generationConfig.responseMimeType` + `responseSchema`.
    GeminiGenerationConfigFlat,
    /// Gemini nested `generationConfig.responseFormat.text`.
    GeminiResponseFormatNested,
}

/// Tool encoding and naming rules.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ToolPolicy {
    /// How tool declarations are shaped.
    pub encoding: ToolEncoding,
    /// Whether call arguments arrive as a JSON string or a JSON object.
    pub arguments: ToolArgumentEncoding,
    /// Z.AI needs `tool_stream: true` for streamed tool calls.
    pub requires_stream_opt_in: bool,
    /// Maximum tool-name length.
    pub max_name_bytes: u16,
    /// Which name grammar the provider enforces.
    pub name_pattern: NamePattern,
}

/// How tool declarations are shaped on the wire.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolEncoding {
    /// `OpenAI` Responses: `{"type":"function","name",…}`.
    OpenAiFlat,
    /// Chat completions: `{"type":"function","function":{…}}`.
    OpenAiNestedFunction,
    /// Anthropic: `{"name","description","input_schema"}`.
    AnthropicInputSchema,
    /// Gemini: `tools:[{functionDeclarations:[…]}]`.
    GeminiFunctionDeclarations,
}

/// Whether call arguments arrive as a JSON string or a JSON object.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolArgumentEncoding {
    /// A string holding JSON, assembled from fragments.
    JsonString,
    /// A whole JSON object.
    JsonObject,
}

/// Which tool-name grammar the provider enforces.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NamePattern {
    /// `[A-Za-z0-9_-]+`.
    OpenAiFunctionName,
    /// `[A-Za-z0-9_-]+`.
    AnthropicToolName,
    /// Must start with a letter or underscore, then `[A-Za-z0-9_.-]`.
    GeminiFunctionName,
}

impl NamePattern {
    /// Whether the name is acceptable under this grammar.
    #[must_use]
    pub fn accepts(self, name: &str) -> bool {
        if name.is_empty() {
            return false;
        }
        match self {
            Self::OpenAiFunctionName | Self::AnthropicToolName => name
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-'),
            Self::GeminiFunctionName => {
                let mut bytes = name.bytes();
                let Some(first) = bytes.next() else {
                    return false;
                };
                if !(first.is_ascii_alphabetic() || first == b'_') {
                    return false;
                }
                bytes.all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'.' || b == b'-')
            }
        }
    }
}

/// Prompt-cache behaviour.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CachePolicy {
    /// Whether caching is absent, provider-managed or caller-placed.
    pub mode: CacheMode,
    /// How many explicit breakpoints the provider accepts.
    pub max_breakpoints: u8,
}

/// Whether caching is absent, provider-managed or caller-placed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CacheMode {
    /// No prompt caching.
    None,
    /// Provider-managed; the caller places nothing.
    Implicit,
    /// Caller-placed breakpoints.
    Explicit,
}

/// How provider usage maps onto [`crate::canonical::NormalizedUsage`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UsageMapping {
    /// Whether the provider's output count already includes reasoning tokens.
    /// `false` for `google`, whose `thoughtsTokenCount` is documented as
    /// excluded from `candidatesTokenCount`.
    pub reasoning_included_in_output: bool,
    /// How the provider reports cache accounting.
    pub cache_read_field: CacheReadSemantics,
    /// Where in the stream usage arrives.
    pub stream_usage_delivery: StreamUsageDelivery,
    /// Fields this pair is known not to report.
    pub known_missing: crate::canonical::UsageFieldSet,
}

/// How a provider reports cache accounting.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CacheReadSemantics {
    /// No cache accounting.
    None,
    /// Separate read and write counters (Anthropic, `OpenAI`).
    SeparateReadWrite,
    /// A hit/miss split of the prompt (`DeepSeek`).
    HitMissSplit,
    /// A `cached_tokens` field that is a subset of the prompt count (Z.AI,
    /// `Moonshot`, Gemini).
    CachedTokensSubsetOfInput,
}

/// Where in the stream usage arrives.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StreamUsageDelivery {
    /// On the terminal dialect event (`OpenAI` `response.completed`, Gemini's
    /// last `usageMetadata`).
    TerminalEvent,
    /// In one trailing chunk whose `choices` array is empty (`DeepSeek`).
    TrailingChoicesEmptyChunk,
    /// On the last content chunk, the one carrying `finish_reason` (Z.AI).
    LastContentChunk,
    /// As cumulative deltas that overwrite rather than sum (Anthropic
    /// `message_delta.usage`).
    CumulativeDeltas,
    /// Only when `stream_options.include_usage` is set (`Moonshot`).
    RequiresIncludeUsageFlag,
}

/// How provider finish tokens map onto canonical stop reasons.
///
/// Failure tokens are listed separately and deliberately: a safety block or a
/// context overflow is a **failure**, never a stop reason (D-09).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StopReasonMap {
    /// Tokens meaning "the model finished its turn".
    pub end_turn: Vec<BoundedString<64>>,
    /// Tokens meaning "the model asked for a tool".
    pub tool_use: Vec<BoundedString<64>>,
    /// Tokens meaning "the output ceiling was reached".
    pub max_output_tokens: Vec<BoundedString<64>>,
    /// Tokens meaning "a stop sequence matched".
    pub stop_sequence: Vec<BoundedString<64>>,
    /// Tokens meaning "the model declined in words".
    pub refusal: Vec<BoundedString<64>>,
    /// Tokens that are terminal failures rather than stop reasons.
    pub failure: Vec<StopFailureMapping>,
}

/// A finish token that means the generation failed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StopFailureMapping {
    /// The provider token.
    pub token: BoundedString<64>,
    /// What it means.
    pub kind: ProviderFailureKind,
}

impl StopReasonMap {
    /// Resolves a provider finish token.
    #[must_use]
    pub fn resolve(&self, token: &str) -> StopResolution {
        let has = |list: &[BoundedString<64>]| list.iter().any(|t| t.as_str() == token);
        if has(&self.end_turn) {
            return StopResolution::Stop(crate::canonical::StopReason::EndTurn);
        }
        if has(&self.tool_use) {
            return StopResolution::Stop(crate::canonical::StopReason::ToolUse);
        }
        if has(&self.max_output_tokens) {
            return StopResolution::Stop(crate::canonical::StopReason::MaxOutputTokens);
        }
        if has(&self.stop_sequence) {
            return StopResolution::Stop(crate::canonical::StopReason::StopSequence);
        }
        if has(&self.refusal) {
            return StopResolution::Stop(crate::canonical::StopReason::Refusal);
        }
        if let Some(mapping) = self.failure.iter().find(|m| m.token.as_str() == token) {
            return StopResolution::Failure(mapping.kind);
        }
        StopResolution::Unknown
    }

    /// Every token the map names, used by the "no token appears twice" load
    /// invariant.
    #[must_use]
    pub fn tokens(&self) -> Vec<&str> {
        let mut out: Vec<&str> = Vec::new();
        for list in [
            &self.end_turn,
            &self.tool_use,
            &self.max_output_tokens,
            &self.stop_sequence,
            &self.refusal,
        ] {
            out.extend(list.iter().map(BoundedString::as_str));
        }
        out.extend(self.failure.iter().map(|m| m.token.as_str()));
        out
    }
}

/// What a provider finish token means.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StopResolution {
    /// A terminal stop reason.
    Stop(crate::canonical::StopReason),
    /// A terminal failure.
    Failure(ProviderFailureKind),
    /// Not in the map at all: a protocol violation for that dialect.
    Unknown,
}

/// How provider statuses and codes map onto failure kinds.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ErrorClassMap {
    /// Status-code rules, sorted and unique by status.
    pub status: Vec<StatusClassRule>,
    /// Provider `type`/`code` string rules, sorted and unique by code. A code
    /// rule always wins over a status rule, which is what makes `Moonshot`'s
    /// three-way 429 split expressible.
    pub codes: Vec<CodeClassRule>,
    /// The kind for anything neither rule names.
    pub default_kind: ProviderFailureKind,
}

/// One status-code rule.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StatusClassRule {
    /// The HTTP status.
    pub status: u16,
    /// What it means.
    pub kind: ProviderFailureKind,
}

/// One provider code rule.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CodeClassRule {
    /// The provider `type` or `code` string.
    pub code: BoundedString<64>,
    /// What it means.
    pub kind: ProviderFailureKind,
}

impl ErrorClassMap {
    /// Classifies a response. A provider code always wins over the status.
    #[must_use]
    pub fn classify(&self, status: u16, code: Option<&str>) -> ProviderFailureKind {
        if let Some(code) = code
            && let Some(rule) = self.codes.iter().find(|r| r.code.as_str() == code)
        {
            return rule.kind;
        }
        if let Some(rule) = self.status.iter().find(|r| r.status == status) {
            return rule.kind;
        }
        self.default_kind
    }
}

/// The in-call retry policy.
///
/// D-20 restricts in-call retry to HTTP 429 and 503 with zero frames observed:
/// those statuses are definitive non-generation rejections, so retrying them
/// cannot create a second generation. Any other status in
/// `in_call_status_retry` is a **load error**.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PreDispatchRetryPolicy {
    /// Total attempts including the first, at least 1 and at most 3.
    pub max_attempts: u16,
    /// The statuses this entry permits an in-call retry for.
    pub in_call_status_retry: Vec<u16>,
    /// First backoff step.
    pub initial_backoff_ms: u32,
    /// Backoff ceiling.
    pub max_backoff_ms: u32,
}

impl PreDispatchRetryPolicy {
    /// The only statuses any entry may declare.
    pub const PERMITTED_STATUSES: [u16; 2] = [429, 503];

    /// The default policy: three attempts over 429 and 503.
    #[must_use]
    pub fn conservative() -> Self {
        Self {
            max_attempts: 3,
            in_call_status_retry: vec![429, 503],
            initial_backoff_ms: 250,
            max_backoff_ms: 4_000,
        }
    }

    /// Whether an in-call retry is permitted for a status.
    #[must_use]
    pub fn permits(&self, status: u16) -> bool {
        self.in_call_status_retry.contains(&status)
    }

    /// Equal-jitter backoff for an attempt, without randomness: the caller
    /// supplies the jitter fraction so the policy stays pure and testable.
    #[must_use]
    pub fn backoff_ms(&self, attempt: u16, jitter_milli: u16) -> u32 {
        let step = self
            .initial_backoff_ms
            .saturating_mul(1u32 << attempt.min(16));
        let capped = step.min(self.max_backoff_ms);
        let half = capped / 2;
        let jitter = u64::from(half) * u64::from(jitter_milli.min(1000)) / 1000;
        // `jitter <= half <= u32::MAX`, so the narrowing is exact.
        half + u32::try_from(jitter).unwrap_or(half)
    }
}

/// Whether the provider offers a durable result lookup for a completed
/// streaming generation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum DurableOperationSupport {
    /// None. This is the launch answer for all eight authorities: Anthropic is
    /// stateless, `OpenAI`'s `GET /v1/responses/{id}` requires `store: true`
    /// which AEX deliberately disables, and Gemini Interactions is not the
    /// launch dialect (D-19).
    #[default]
    None,
    /// A completed result can be fetched by id within a window.
    ResultLookup {
        /// How long the provider retains the result.
        ttl_ms: u64,
    },
    /// A stream can be resumed from an event id within a window.
    ResumableStream {
        /// How long the provider retains the stream.
        ttl_ms: u64,
    },
}

/// An opaque pricing-context reference.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct PricingContextRef(pub BoundedString<64>);

/// A pair withdrawn from service without a new catalog release.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EmergencyDisable {
    /// The provider half of the pair.
    pub provider: ProviderId,
    /// The model half of the pair.
    pub model: ModelSlug,
    /// When the disable took effect.
    pub since: Timestamp,
    /// Why.
    pub reason: DisableReason,
}

/// Why a pair was emergency disabled.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DisableReason {
    /// The provider is having an incident.
    ProviderIncident,
    /// A security advisory affects the pair.
    SecurityAdvisory,
    /// Conformance regressed against a passing receipt.
    ConformanceRegression,
    /// The provider withdrew the model.
    Withdrawn,
}
