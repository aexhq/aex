//! The `moonshotai` dialect adapter (plan 08 §5.5): Kimi / `Moonshot`
//! international chat completions.
//!
//! `POST /v1/chat/completions` on the pinned `https://api.moonshot.ai` origin.
//! `platform.kimi.ai` is the docs and console host and is never an API origin;
//! `api.moonshot.cn` is a launch exclusion. Neither is reachable, because the
//! origin is an [`EndpointPin`] rather than configuration.
//!
//! # The 429 trap
//!
//! This provider's error body names its failure in `error.type` — **not**
//! `error.code` — and status `429` is overloaded across three unrelated causes:
//!
//! - `engine_overloaded_error` is [`ProviderFailureKind::Overloaded`], retryable;
//! - `rate_limit_reached_error` is [`ProviderFailureKind::RateLimited`], retryable;
//! - `exceeded_current_quota_error` is [`ProviderFailureKind::Quota`], which is
//!   **permanent**: retrying it is futile and merely burns the dispatch budget.
//!
//! [`MoonshotAdapter::classify_http`] therefore branches on `error.type` first
//! and falls back to the status only when the body names no documented type.
//!
//! # Other dialect facts
//!
//! - `max_completion_tokens` is sent; `max_tokens` is deprecated and never sent.
//! - `stream_options.include_usage` is always `true`, because usage arrives in a
//!   final chunk before `[DONE]` only when that flag is set
//!   ([`StreamUsageDelivery::RequiresIncludeUsageFlag`]).
//! - Sampling is **model-gated**: `temperature` / `top_p` are honoured only on
//!   `moonshot-v1*`. A sampling field on a `Kimi` model is
//!   [`RequestBuildError::SamplingUnsupported`], never a silently ignored
//!   parameter.
//! - Tool-call ids are **semantic** (`search:0`), not `UUID`s, so they are
//!   carried verbatim into [`ToolCallId`].
//! - The documented finish-reason enum has exactly three members, so anything
//!   else is a protocol violation rather than an unmapped stop reason.
//!
//! # Deliberately absent
//!
//! No in-stream `error` frame is decoded: plan 08 §6.1 records the definitive
//! terminal error for this provider as a non-2xx response with an
//! `{"error":{"type":…}}` body, and nothing else. A frame that is not a
//! `chat.completion.chunk` is a decode failure rather than a second error
//! channel.
//!
//! [`StreamUsageDelivery::RequiresIncludeUsageFlag`]: aex_model_catalog::document::StreamUsageDelivery::RequiresIncludeUsageFlag

use core::time::Duration;
use std::collections::BTreeMap;

use aex_model_catalog::QualifiedModel;
use aex_model_catalog::canonical::{
    CacheBreakpoint, CanonicalBlock, CanonicalMessage, CanonicalModelRequest, CanonicalToolDef,
    NormalizedUsage, REASON_MAX, ReasoningBlock, ReasoningBody, ReasoningEffort, ReasoningRequest,
    Role, StopReason, StructuredOutputRequest, SystemBlock, TEXT_MAX, ToolChoice, ToolResultPart,
    UsageCompleteness,
};
use aex_model_catalog::document::{
    AdapterSourceDigest, Capability, CapabilitySet, EndpointPin, ModelLimits, ReasoningMode,
    ReasoningPolicy, SamplingSupport, SchemaEncoding, StructuredOutputPolicy, ToolEncoding,
    ToolPolicy,
};
use aex_model_catalog::primitives::{
    Blake3Digest, BoundedString, ProviderRequestId, ToolCallId, ToolName,
};
use aex_wire::CanonicalJson;
use aex_wire::provider::ProviderId;
use bytes::Bytes;
use serde_json::{Map, Value, json};

use crate::adapter::{
    BoundedBody, DialectState, FrameDecodeError, FrameOutcome, HeaderView, PartialToolCall,
    ProviderAdapter, RequestBuildError, SealedResponse,
};
use crate::budget::{BudgetLedger, BudgetOverrun, StreamBudget};
use crate::error::{ProviderFailure, ProviderFailureKind, RateLimitFeedback, RedactedDetail};
use crate::redact::redact;
use crate::sse::SseEvent;
use crate::transport::{Accept, AuthScheme, WireRequest};

// ---------------------------------------------------------------------------
// compiled dialect facts
// ---------------------------------------------------------------------------

/// The most stop sequences this dialect accepts, whatever a catalog entry
/// declares. A document may narrow the bound; it may never widen it.
pub const MAX_STOP_SEQUENCES: u8 = 5;

/// The provider's own gateway timeout, after which it answers `504`. The
/// documented figure is 900 seconds.
pub const GATEWAY_TIMEOUT: Duration = Duration::from_mins(15);

/// The only path this dialect speaks.
const CHAT_COMPLETIONS_PATH: &str = "/v1/chat/completions";

/// The `object` discriminant every streamed frame carries.
const CHUNK_OBJECT: &str = "chat.completion.chunk";

/// The block index text and reasoning accumulate under. `n` is never sent, so
/// there is exactly one choice and therefore exactly one of each.
const SOLE_BLOCK: u16 = 0;

/// The seed the adapter source digest is taken over.
///
/// Bumping a dialect revision is a code release (D-19), so the seed names the
/// dialect and its revision rather than being computed from a file at runtime,
/// which no compiled binary can do.
const SOURCE_SEED: &[u8] = b"aex-brain-provider-gateway/moonshotai/1";

/// The documented finish-reason enum, whole. Anything outside it is a protocol
/// violation.
const STOP_TOKENS: [(&str, StopReason); 3] = [
    ("stop", StopReason::EndTurn),
    ("tool_calls", StopReason::ToolUse),
    ("length", StopReason::MaxOutputTokens),
];

/// The documented `error.type` vocabulary. A type always wins over the status,
/// which is what makes the three-way `429` split expressible.
const ERROR_TYPES: [(&str, ProviderFailureKind); 13] = [
    ("content_filter", ProviderFailureKind::ContentFiltered),
    ("invalid_request_error", ProviderFailureKind::InvalidRequest),
    (
        "invalid_authentication_error",
        ProviderFailureKind::Authentication,
    ),
    (
        "incorrect_api_key_error",
        ProviderFailureKind::Authentication,
    ),
    (
        "permission_denied_error",
        ProviderFailureKind::Authentication,
    ),
    (
        "resource_not_found_error",
        ProviderFailureKind::ModelNotFound,
    ),
    ("engine_overloaded_error", ProviderFailureKind::Overloaded),
    ("rate_limit_reached_error", ProviderFailureKind::RateLimited),
    ("exceeded_current_quota_error", ProviderFailureKind::Quota),
    ("client_closed_request", ProviderFailureKind::Cancelled),
    ("server_error", ProviderFailureKind::ServerError),
    ("unexpected_output", ProviderFailureKind::ServerError),
    ("server_unavailable", ProviderFailureKind::Overloaded),
];

/// The documented status vocabulary, consulted only when the body names no
/// documented type.
const ERROR_STATUSES: [(u16, ProviderFailureKind); 9] = [
    (400, ProviderFailureKind::InvalidRequest),
    (401, ProviderFailureKind::Authentication),
    (403, ProviderFailureKind::Authentication),
    (404, ProviderFailureKind::ModelNotFound),
    (429, ProviderFailureKind::RateLimited),
    (499, ProviderFailureKind::Cancelled),
    (500, ProviderFailureKind::ServerError),
    (503, ProviderFailureKind::Overloaded),
    (504, ProviderFailureKind::Timeout),
];

// ---------------------------------------------------------------------------
// the adapter
// ---------------------------------------------------------------------------

/// The `moonshotai` dialect adapter.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct MoonshotAdapter;

/// Which control vocabulary a model slug takes.
///
/// Slug-family gating is a recorded fact for this provider rather than an
/// inference: plan 08 §5.5 states that sampling is honoured only on
/// `moonshot-v1*`, and that `Kimi` k2.x takes a `thinking` object while k3 takes
/// `reasoning_effort`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ModelFamily {
    /// `moonshot-v1*`: `OpenAI`-style sampling, no reasoning controls.
    MoonshotV1,
    /// `kimi-k3*`: `reasoning_effort ∈ {low, high, max}`.
    KimiEffort,
    /// Every other `Kimi` slug: `thinking {type}`.
    KimiThinking,
}

/// A decoder state for one stream.
///
/// Usage starts [`UsageCompleteness::Absent`] rather than `Exact`: this dialect
/// reports usage only in the trailing chunk, so a stream that never delivers one
/// must record the absence rather than a zeroed tally.
fn fresh_state() -> DialectState {
    DialectState {
        usage: NormalizedUsage {
            completeness: UsageCompleteness::Absent,
            ..NormalizedUsage::default()
        },
        ..DialectState::default()
    }
}

/// Classifies a model slug into its control family.
fn family_of(slug: &str) -> ModelFamily {
    if slug.starts_with("moonshot-v1") {
        return ModelFamily::MoonshotV1;
    }
    if slug.starts_with("kimi-k3") {
        return ModelFamily::KimiEffort;
    }
    ModelFamily::KimiThinking
}

/// The catalog facts this dialect reads when it builds a request.
///
/// Lifted out of [`QualifiedModel`] so the body builder is a pure function of
/// plain data: a live catalog handle cannot be minted outside its own crate, and
/// the builder must be exercisable without one.
#[derive(Debug, Clone, Copy)]
struct EntryView<'a> {
    /// The exact provider-native model id.
    model: &'a str,
    /// The compiled origin.
    endpoint: EndpointPin,
    /// What the pair declares it can do.
    capabilities: CapabilitySet,
    /// Numeric bounds.
    limits: &'a ModelLimits,
    /// Which sampling controls the pair honours.
    sampling: SamplingSupport,
    /// Reasoning behaviour.
    reasoning: ReasoningPolicy,
    /// Structured-output support and encoding.
    structured_output: StructuredOutputPolicy,
    /// Tool encoding and naming rules.
    tool_policy: &'a ToolPolicy,
}

/// The request members this dialect encodes.
///
/// Mirrors [`CanonicalModelRequest`] minus `selection`, for the same reason
/// [`EntryView`] exists.
#[derive(Debug, Clone, Copy)]
struct RequestView<'a> {
    /// System instruction blocks.
    system: &'a [SystemBlock],
    /// Conversation history.
    messages: &'a [CanonicalMessage],
    /// Tool declarations.
    tools: &'a [CanonicalToolDef],
    /// How the model must choose among them.
    tool_choice: &'a ToolChoice,
    /// Whether parallel tool calls are permitted.
    parallel_tools: bool,
    /// The output ceiling.
    max_output_tokens: u32,
    /// Temperature in integer milli-units.
    temperature_milli: Option<u16>,
    /// Nucleus sampling in integer milli-units.
    top_p_milli: Option<u16>,
    /// Caller stop sequences.
    stop_sequences: &'a [BoundedString<64>],
    /// Reasoning request.
    reasoning: ReasoningRequest,
    /// Structured-output request.
    structured_output: Option<&'a StructuredOutputRequest>,
    /// Explicit prompt-cache breakpoints.
    cache_breakpoints: &'a [CacheBreakpoint],
}

impl ProviderAdapter for MoonshotAdapter {
    fn provider(&self) -> ProviderId {
        ProviderId::Moonshotai
    }

    fn source_digest(&self) -> AdapterSourceDigest {
        AdapterSourceDigest(Blake3Digest::of(SOURCE_SEED))
    }

    fn build_request(
        &self,
        model: &QualifiedModel,
        request: &CanonicalModelRequest,
    ) -> Result<WireRequest, RequestBuildError> {
        let entry = model.entry();
        let view = EntryView {
            model: model.model().as_str(),
            endpoint: model.endpoint(),
            capabilities: model.capabilities(),
            limits: model.limits(),
            sampling: entry.sampling,
            reasoning: entry.reasoning,
            structured_output: entry.structured_output,
            tool_policy: &entry.tool_policy,
        };
        let projection = RequestView {
            system: &request.system,
            messages: &request.messages,
            tools: &request.tools,
            tool_choice: &request.tool_choice,
            parallel_tools: request.parallel_tools,
            max_output_tokens: request.max_output_tokens,
            temperature_milli: request.temperature_milli,
            top_p_milli: request.top_p_milli,
            stop_sequences: &request.stop_sequences,
            reasoning: request.reasoning,
            structured_output: request.structured_output.as_ref(),
            cache_breakpoints: &request.cache_breakpoints,
        };
        build(&view, &projection)
    }

    fn new_state(&self, _model: &QualifiedModel) -> DialectState {
        fresh_state()
    }

    fn decode(
        &self,
        state: &mut DialectState,
        event: &SseEvent<'_>,
        budget: &StreamBudget,
    ) -> Result<FrameOutcome, FrameDecodeError> {
        if let Some(name) = event.name {
            return Err(FrameDecodeError::UnknownEvent {
                event: BoundedString::truncating(name),
            });
        }
        state
            .ledger
            .charge_response(budget, u64::try_from(event.data.len()).unwrap_or(u64::MAX))?;
        if event.is_done_sentinel() {
            state.ledger.count_frame();
            state.terminal = true;
            return Ok(FrameOutcome::Terminal);
        }
        let text = event.data_str().map_err(|_| FrameDecodeError::NotJson)?;
        let chunk: Value = serde_json::from_str(text).map_err(|_| FrameDecodeError::NotJson)?;
        state.ledger.count_frame();
        decode_chunk(state, &chunk, budget)
    }

    fn finish(&self, state: DialectState) -> Result<SealedResponse, FrameDecodeError> {
        if !state.terminal {
            return Err(FrameDecodeError::OutOfOrder {
                reason: "the stream ended without the `[DONE]` sentinel",
            });
        }
        let token = state
            .finish_token
            .as_deref()
            .ok_or(FrameDecodeError::MalformedField {
                field: "choices[].finish_reason",
            })?;
        let stop_reason = stop_for(token).ok_or(FrameDecodeError::MalformedField {
            field: "choices[].finish_reason",
        })?;
        let blocks = assemble_blocks(&state)?;
        if blocks.is_empty() {
            return Err(FrameDecodeError::OutOfOrder {
                reason: "the stream produced no content",
            });
        }
        let has_tools = blocks
            .iter()
            .any(|block| matches!(block, CanonicalBlock::ToolUse { .. }));
        if has_tools != matches!(stop_reason, StopReason::ToolUse) {
            return Err(FrameDecodeError::OutOfOrder {
                reason: "tool calls and the finish reason disagree",
            });
        }
        Ok(SealedResponse {
            blocks,
            stop_reason,
            usage: state.usage,
            provider_request_id: state.request_id,
        })
    }

    fn classify_http(
        &self,
        status: u16,
        headers: &HeaderView<'_>,
        body: &BoundedBody,
    ) -> ProviderFailure {
        let (code, message) = error_fields(body);
        let kind = classify(status, code.as_deref());
        let raw = message.unwrap_or_else(|| body.as_str().unwrap_or_default().to_owned());
        let mut detail = RedactedDetail::new(kind, redact::<512>(&raw, &[])).with_status(status);
        if let Some(code) = &code {
            detail = detail.with_code(redact::<64>(code, &[]).as_str());
        }
        ProviderFailure {
            detail,
            rate_limit: self.rate_limit_feedback(headers),
        }
    }

    fn rate_limit_feedback(&self, headers: &HeaderView<'_>) -> RateLimitFeedback {
        // `Retry-After` on 429 is the whole of this provider's published
        // backpressure: there is no `x-ratelimit-*` family, and the tier table
        // is catalog `concurrency_hint` rather than a response header.
        headers
            .get_u64("retry-after")
            .map_or_else(RateLimitFeedback::none, |seconds| {
                RateLimitFeedback::retry_after(Duration::from_secs(seconds))
            })
    }

    fn request_id(
        &self,
        headers: &HeaderView<'_>,
        state: &DialectState,
    ) -> Option<ProviderRequestId> {
        headers
            .get("x-request-id")
            .and_then(|value| ProviderRequestId::new(value).ok())
            .or_else(|| state.request_id.clone())
    }
}

// ---------------------------------------------------------------------------
// request building (pure: no I/O, no clock, no credential)
// ---------------------------------------------------------------------------

/// Builds the wire request from lifted catalog and request facts.
fn build(
    entry: &EntryView<'_>,
    request: &RequestView<'_>,
) -> Result<WireRequest, RequestBuildError> {
    if entry.endpoint != EndpointPin::MoonshotIntlV1 {
        return Err(RequestBuildError::Encoding {
            reason: "this dialect speaks only to the pinned international origin",
        });
    }
    if !entry.capabilities.has(Capability::Streaming) {
        return Err(RequestBuildError::CapabilityUnavailable {
            capability: Capability::Streaming,
        });
    }
    if !request.cache_breakpoints.is_empty() {
        return Err(RequestBuildError::CapabilityUnavailable {
            capability: Capability::PromptCacheExplicit,
        });
    }
    let limits = entry.limits;
    if request.max_output_tokens < limits.min_output_tokens
        || request.max_output_tokens > limits.max_output_tokens
    {
        return Err(RequestBuildError::OutputTokensOutOfRange {
            min: limits.min_output_tokens,
            max: limits.max_output_tokens,
        });
    }

    let mut body = Map::new();
    body.insert("model".to_owned(), Value::String(entry.model.to_owned()));
    body.insert(
        "messages".to_owned(),
        Value::Array(encode_messages(request)?),
    );
    body.insert("stream".to_owned(), Value::Bool(true));
    body.insert(
        "stream_options".to_owned(),
        json!({ "include_usage": true }),
    );
    body.insert(
        "max_completion_tokens".to_owned(),
        Value::from(request.max_output_tokens),
    );
    encode_stop(entry, request, &mut body)?;
    encode_sampling(entry, request, &mut body)?;
    encode_reasoning(entry, request, &mut body)?;
    encode_tools(entry, request, &mut body)?;
    encode_response_format(entry, request, &mut body)?;

    let bytes =
        serde_json::to_vec(&Value::Object(body)).map_err(|_| RequestBuildError::Encoding {
            reason: "the request body could not be serialized",
        })?;
    if u64::try_from(bytes.len()).unwrap_or(u64::MAX) > u64::from(limits.request_body_max_bytes) {
        return Err(RequestBuildError::BodyTooLarge {
            limit: limits.request_body_max_bytes,
        });
    }
    Ok(WireRequest {
        endpoint: entry.endpoint,
        path: bounded_path(CHAT_COMPLETIONS_PATH)?,
        query: Vec::new(),
        headers: vec![("content-type", bounded_header("application/json")?)],
        auth: AuthScheme::BearerAuthorization,
        body: Bytes::from(bytes),
        accept: Accept::TextEventStream,
    })
}

/// The compiled path, bounded.
fn bounded_path(path: &str) -> Result<BoundedString<256>, RequestBuildError> {
    BoundedString::new(path).map_err(|_| RequestBuildError::Encoding {
        reason: "the compiled path exceeds its bound",
    })
}

/// A compiled header value, bounded.
fn bounded_header(value: &str) -> Result<BoundedString<128>, RequestBuildError> {
    BoundedString::new(value).map_err(|_| RequestBuildError::Encoding {
        reason: "a compiled header value exceeds its bound",
    })
}

/// Encodes `stop`, enforcing the dialect's own five-sequence ceiling.
fn encode_stop(
    entry: &EntryView<'_>,
    request: &RequestView<'_>,
    body: &mut Map<String, Value>,
) -> Result<(), RequestBuildError> {
    if request.stop_sequences.is_empty() {
        return Ok(());
    }
    if !entry.capabilities.has(Capability::StopSequences) {
        return Err(RequestBuildError::CapabilityUnavailable {
            capability: Capability::StopSequences,
        });
    }
    let max = entry.limits.max_stop_sequences.min(MAX_STOP_SEQUENCES);
    if request.stop_sequences.len() > usize::from(max) {
        return Err(RequestBuildError::StopSequenceLimit { max });
    }
    body.insert(
        "stop".to_owned(),
        Value::Array(
            request
                .stop_sequences
                .iter()
                .map(|sequence| Value::String(sequence.as_str().to_owned()))
                .collect(),
        ),
    );
    Ok(())
}

/// Whether a milli-unit value sits inside a declared inclusive range.
fn in_range(range: Option<(u16, u16)>, value: u16) -> bool {
    range.is_some_and(|(low, high)| (low..=high).contains(&value))
}

/// Renders integer milli-units as a `JSON` decimal.
fn decimal(milli: u16) -> Result<Value, RequestBuildError> {
    let number = serde_json::Number::from_f64(f64::from(milli) / 1000.0).ok_or(
        RequestBuildError::Encoding {
            reason: "a sampling value could not be rendered as a JSON number",
        },
    )?;
    Ok(Value::Number(number))
}

/// Encodes `temperature` and `top_p`, which only `moonshot-v1*` honours.
fn encode_sampling(
    entry: &EntryView<'_>,
    request: &RequestView<'_>,
    body: &mut Map<String, Value>,
) -> Result<(), RequestBuildError> {
    let v1 = family_of(entry.model) == ModelFamily::MoonshotV1;
    if let Some(milli) = request.temperature_milli {
        let honoured = v1
            && entry.capabilities.has(Capability::Temperature)
            && matches!(
                entry.sampling,
                SamplingSupport::TemperatureOnly | SamplingSupport::Full
            )
            && in_range(entry.limits.temperature_milli, milli);
        if !honoured {
            return Err(RequestBuildError::SamplingUnsupported {
                field: "temperature",
            });
        }
        body.insert("temperature".to_owned(), decimal(milli)?);
    }
    if let Some(milli) = request.top_p_milli {
        let honoured = v1
            && entry.capabilities.has(Capability::TopP)
            && matches!(entry.sampling, SamplingSupport::Full)
            && in_range(entry.limits.top_p_milli, milli);
        if !honoured {
            return Err(RequestBuildError::SamplingUnsupported { field: "top_p" });
        }
        body.insert("top_p".to_owned(), decimal(milli)?);
    }
    Ok(())
}

/// The provider's own effort ladder, which has exactly three rungs.
const fn effort_for(effort: ReasoningEffort) -> &'static str {
    match effort {
        ReasoningEffort::Minimal | ReasoningEffort::Low => "low",
        ReasoningEffort::Medium | ReasoningEffort::High => "high",
        ReasoningEffort::Max => "max",
    }
}

/// Encodes the family-appropriate reasoning control.
fn encode_reasoning(
    entry: &EntryView<'_>,
    request: &RequestView<'_>,
    body: &mut Map<String, Value>,
) -> Result<(), RequestBuildError> {
    let (enabled, effort) = match request.reasoning {
        ReasoningRequest::ProviderDefault => return Ok(()),
        ReasoningRequest::Disabled => (false, None),
        ReasoningRequest::Enabled {
            budget_tokens,
            effort,
        } => {
            if budget_tokens.is_some() {
                return Err(RequestBuildError::Encoding {
                    reason: "this dialect takes an effort level, not a reasoning-token budget",
                });
            }
            (true, effort)
        }
    };
    if !entry.capabilities.has(Capability::Reasoning)
        || entry.reasoning.mode == ReasoningMode::Unsupported
    {
        return Err(RequestBuildError::CapabilityUnavailable {
            capability: Capability::Reasoning,
        });
    }
    if !enabled && entry.reasoning.mode == ReasoningMode::AlwaysOn {
        return Err(RequestBuildError::Encoding {
            reason: "this pair cannot disable reasoning",
        });
    }
    match family_of(entry.model) {
        ModelFamily::MoonshotV1 => Err(RequestBuildError::CapabilityUnavailable {
            capability: Capability::Reasoning,
        }),
        ModelFamily::KimiThinking => {
            let mode = if enabled { "enabled" } else { "disabled" };
            body.insert("thinking".to_owned(), json!({ "type": mode }));
            Ok(())
        }
        ModelFamily::KimiEffort => {
            if !enabled {
                return Err(RequestBuildError::Encoding {
                    reason: "this model has no reasoning-off setting",
                });
            }
            if let Some(effort) = effort {
                body.insert(
                    "reasoning_effort".to_owned(),
                    Value::String(effort_for(effort).to_owned()),
                );
            }
            Ok(())
        }
    }
}

/// Encodes one `OpenAI`-nested tool declaration.
fn encode_tool(entry: &EntryView<'_>, tool: &CanonicalToolDef) -> Result<Value, RequestBuildError> {
    let policy = entry.tool_policy;
    if policy.encoding != ToolEncoding::OpenAiNestedFunction {
        return Err(RequestBuildError::Encoding {
            reason: "this dialect declares tools in the nested function shape only",
        });
    }
    let name = tool.name.as_str();
    if !policy.name_pattern.accepts(name) || name.len() > usize::from(policy.max_name_bytes) {
        return Err(RequestBuildError::ToolNameInvalid {
            name: tool.name.clone(),
        });
    }
    let mut function = Map::new();
    function.insert("name".to_owned(), Value::String(name.to_owned()));
    function.insert(
        "description".to_owned(),
        Value::String(tool.description.as_str().to_owned()),
    );
    function.insert("parameters".to_owned(), tool.input_schema.to_value());
    if tool.strict {
        if !entry.capabilities.has(Capability::StrictToolSchema) {
            return Err(RequestBuildError::CapabilityUnavailable {
                capability: Capability::StrictToolSchema,
            });
        }
        function.insert("strict".to_owned(), Value::Bool(true));
    }
    Ok(json!({ "type": "function", "function": Value::Object(function) }))
}

/// Encodes `tool_choice`, refusing any mode the pair does not declare.
fn encode_tool_choice(
    entry: &EntryView<'_>,
    choice: &ToolChoice,
) -> Result<Value, RequestBuildError> {
    let refused = || RequestBuildError::ToolChoiceUnsupported {
        requested: Box::new(choice.clone()),
    };
    match choice {
        ToolChoice::Auto => Ok(Value::String("auto".to_owned())),
        ToolChoice::None if entry.capabilities.has(Capability::ToolChoiceNone) => {
            Ok(Value::String("none".to_owned()))
        }
        ToolChoice::Required if entry.capabilities.has(Capability::ToolChoiceRequired) => {
            Ok(Value::String("required".to_owned()))
        }
        ToolChoice::Named { name } if entry.capabilities.has(Capability::ToolChoiceNamed) => {
            Ok(json!({ "type": "function", "function": { "name": name.as_str() } }))
        }
        ToolChoice::None | ToolChoice::Required | ToolChoice::Named { .. } => Err(refused()),
    }
}

/// Encodes `tools` and `tool_choice`.
fn encode_tools(
    entry: &EntryView<'_>,
    request: &RequestView<'_>,
    body: &mut Map<String, Value>,
) -> Result<(), RequestBuildError> {
    if request.tools.is_empty() {
        return match request.tool_choice {
            ToolChoice::Auto | ToolChoice::None => Ok(()),
            other => Err(RequestBuildError::ToolChoiceUnsupported {
                requested: Box::new(other.clone()),
            }),
        };
    }
    if !entry.capabilities.has(Capability::Tools) {
        return Err(RequestBuildError::CapabilityUnavailable {
            capability: Capability::Tools,
        });
    }
    if !request.parallel_tools {
        // The documented request has no parallel-tool switch, so honouring
        // "sequential only" is impossible. Refusing beats dropping it silently.
        return Err(RequestBuildError::Encoding {
            reason: "this dialect has no parallel-tool-calls switch",
        });
    }
    if request.tools.len() > usize::from(entry.limits.max_tools) {
        return Err(RequestBuildError::ToolLimit {
            max: entry.limits.max_tools,
        });
    }
    let mut declared = Vec::with_capacity(request.tools.len());
    for tool in request.tools {
        declared.push(encode_tool(entry, tool)?);
    }
    body.insert("tools".to_owned(), Value::Array(declared));
    body.insert(
        "tool_choice".to_owned(),
        encode_tool_choice(entry, request.tool_choice)?,
    );
    Ok(())
}

/// Encodes `response_format`. This is one of only two providers in the set with
/// real schema support, so a schema request is honoured rather than refused.
fn encode_response_format(
    entry: &EntryView<'_>,
    request: &RequestView<'_>,
    body: &mut Map<String, Value>,
) -> Result<(), RequestBuildError> {
    let Some(requested) = request.structured_output else {
        return Ok(());
    };
    let refused = || RequestBuildError::StructuredOutputUnsupported {
        requested: Box::new(requested.clone()),
    };
    if !entry.capabilities.has(Capability::StructuredOutput) {
        return Err(refused());
    }
    match (requested, entry.structured_output) {
        (
            StructuredOutputRequest::JsonObject,
            StructuredOutputPolicy::JsonObjectOnly | StructuredOutputPolicy::JsonSchema { .. },
        ) => {
            body.insert(
                "response_format".to_owned(),
                json!({ "type": "json_object" }),
            );
            Ok(())
        }
        (
            StructuredOutputRequest::JsonSchema {
                name,
                schema,
                strict,
            },
            StructuredOutputPolicy::JsonSchema {
                encoding: SchemaEncoding::MoonshotResponseFormat,
                ..
            },
        ) => {
            body.insert(
                "response_format".to_owned(),
                json!({
                    "type": "json_schema",
                    "json_schema": {
                        "name": name.as_str(),
                        "strict": *strict,
                        "schema": schema.to_value(),
                    }
                }),
            );
            Ok(())
        }
        _ => Err(refused()),
    }
}

// ---------------------------------------------------------------------------
// message encoding
// ---------------------------------------------------------------------------

/// Appends a line, so blocks of one turn join without running together.
fn push_line(buffer: &mut String, text: &str) {
    if !buffer.is_empty() {
        buffer.push('\n');
    }
    buffer.push_str(text);
}

/// Every tool-call id declared in this conversation, mapped to its tool name.
///
/// The documented result shape carries `name` alongside `tool_call_id`, and the
/// canonical result block carries only the id, so the name is recovered from the
/// call it answers rather than invented.
fn tool_names(messages: &[CanonicalMessage]) -> BTreeMap<&str, &str> {
    let mut out = BTreeMap::new();
    for message in messages {
        for block in &message.blocks {
            if let CanonicalBlock::ToolUse { id, name, .. } = block {
                out.insert(id.as_str(), name.as_str());
            }
        }
    }
    out
}

/// Renders a tool result's parts as one string.
///
/// `is_error` has no wire field on this dialect; the failure text is the content
/// itself, so nothing is dropped by having no place to put the flag.
fn render_parts(parts: &[ToolResultPart]) -> String {
    let mut out = String::new();
    for part in parts {
        match part {
            ToolResultPart::Text { text } => push_line(&mut out, text.as_str()),
            ToolResultPart::Json { value } => push_line(&mut out, value.as_str()),
        }
    }
    out
}

/// Encodes one user turn: tool results become `tool` messages, prose becomes one
/// `user` message.
fn encode_user_turn(
    blocks: &[CanonicalBlock],
    names: &BTreeMap<&str, &str>,
    out: &mut Vec<Value>,
) -> Result<(), RequestBuildError> {
    let mut text = String::new();
    for block in blocks {
        match block {
            CanonicalBlock::Text { text: body, .. } => push_line(&mut text, body.as_str()),
            CanonicalBlock::ToolResult { call, content, .. } => {
                let name = names
                    .get(call.as_str())
                    .ok_or(RequestBuildError::Encoding {
                        reason: "a tool result names no tool call in this conversation",
                    })?;
                out.push(json!({
                    "role": "tool",
                    "tool_call_id": call.as_str(),
                    "name": *name,
                    "content": render_parts(content),
                }));
            }
            CanonicalBlock::Reasoning(_)
            | CanonicalBlock::Refusal { .. }
            | CanonicalBlock::ToolUse { .. } => {
                return Err(RequestBuildError::Encoding {
                    reason: "a user turn may carry only text and tool results",
                });
            }
        }
    }
    if !text.is_empty() {
        out.push(json!({ "role": "user", "content": text }));
    }
    Ok(())
}

/// Encodes one assistant turn, echoing `reasoning_content` where the history
/// carries it (`ReasoningReplay::RecommendedEcho`).
fn encode_assistant_turn(
    blocks: &[CanonicalBlock],
    out: &mut Vec<Value>,
) -> Result<(), RequestBuildError> {
    let mut text = String::new();
    let mut reasoning = String::new();
    let mut calls = Vec::new();
    for block in blocks {
        match block {
            CanonicalBlock::Text { text: body, .. } => push_line(&mut text, body.as_str()),
            // There is no refusal channel on this dialect, so a refusal replays
            // as the assistant prose it literally was.
            CanonicalBlock::Refusal { text: body } => push_line(&mut text, body.as_str()),
            CanonicalBlock::Reasoning(reasoning_block) => {
                check_provenance(reasoning_block)?;
                match &reasoning_block.body {
                    ReasoningBody::Text { text: body } | ReasoningBody::Summary { text: body } => {
                        push_line(&mut reasoning, body.as_str());
                    }
                    ReasoningBody::Redacted => {}
                }
            }
            CanonicalBlock::ToolUse { id, name, input } => calls.push(json!({
                "id": id.as_str(),
                "type": "function",
                "function": { "name": name.as_str(), "arguments": input.as_str() },
            })),
            CanonicalBlock::ToolResult { .. } => {
                return Err(RequestBuildError::Encoding {
                    reason: "an assistant turn may not carry a tool result",
                });
            }
        }
    }
    if text.is_empty() && calls.is_empty() {
        return Err(RequestBuildError::Encoding {
            reason: "an assistant turn carried no content",
        });
    }
    let mut message = Map::new();
    message.insert("role".to_owned(), Value::String("assistant".to_owned()));
    if !text.is_empty() {
        message.insert("content".to_owned(), Value::String(text));
    }
    if !reasoning.is_empty() {
        message.insert("reasoning_content".to_owned(), Value::String(reasoning));
    }
    if !calls.is_empty() {
        message.insert("tool_calls".to_owned(), Value::Array(calls));
    }
    out.push(Value::Object(message));
    Ok(())
}

/// Refuses reasoning material minted by another provider (D-12).
fn check_provenance(block: &ReasoningBlock) -> Result<(), RequestBuildError> {
    if let Some(token) = &block.token
        && token.provenance != ProviderId::Moonshotai
    {
        return Err(RequestBuildError::ReasoningProvenanceMismatch {
            expected: ProviderId::Moonshotai,
            found: token.provenance,
        });
    }
    Ok(())
}

/// Encodes the whole `messages` array.
fn encode_messages(request: &RequestView<'_>) -> Result<Vec<Value>, RequestBuildError> {
    let names = tool_names(request.messages);
    let mut out = Vec::new();
    for block in request.system {
        out.push(json!({ "role": "system", "content": block.text.as_str() }));
    }
    for message in request.messages {
        match message.role {
            Role::User => encode_user_turn(&message.blocks, &names, &mut out)?,
            Role::Assistant => encode_assistant_turn(&message.blocks, &mut out)?,
        }
    }
    if out.is_empty() {
        return Err(RequestBuildError::Encoding {
            reason: "the request carried no messages",
        });
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// frame decoding
// ---------------------------------------------------------------------------

/// Resolves a provider finish token against the documented enum.
fn stop_for(token: &str) -> Option<StopReason> {
    STOP_TOKENS
        .iter()
        .find(|(name, _)| *name == token)
        .map(|(_, stop)| *stop)
}

/// The failure an undocumented finish reason produces.
fn undocumented_finish(token: &str) -> ProviderFailure {
    let message = format!("`{token}` is not a documented finish reason");
    let detail = RedactedDetail::new(
        ProviderFailureKind::ProtocolViolation,
        redact::<512>(&message, &[]),
    )
    .with_code(redact::<64>(token, &[]).as_str());
    ProviderFailure::new(detail)
}

/// Reads a required unsigned integer member.
fn number(value: &Value, field: &'static str) -> Result<u64, FrameDecodeError> {
    value
        .get(field)
        .and_then(Value::as_u64)
        .ok_or(FrameDecodeError::MalformedField { field })
}

/// Applies the usage chunk that `stream_options.include_usage` produces.
///
/// `cached_tokens` is a subset of `prompt_tokens`
/// ([`aex_model_catalog::document::CacheReadSemantics::CachedTokensSubsetOfInput`]),
/// so it is subtracted out of `input_tokens` rather than added to it.
fn apply_usage(state: &mut DialectState, usage: &Value) -> Result<(), FrameDecodeError> {
    let prompt = number(usage, "prompt_tokens")?;
    let completion = number(usage, "completion_tokens")?;
    let cached = usage
        .get("cached_tokens")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    state.usage = NormalizedUsage {
        input_tokens: prompt.saturating_sub(cached),
        cache_read_input_tokens: cached,
        cache_write_input_tokens: 0,
        output_tokens: completion,
        reasoning_tokens: 0,
        tool_use_prompt_tokens: 0,
        provider_total_tokens: usage.get("total_tokens").and_then(Value::as_u64),
        completeness: UsageCompleteness::Exact,
    };
    Ok(())
}

/// Accumulates assistant prose.
fn append_text(
    state: &mut DialectState,
    budget: &StreamBudget,
    text: &str,
) -> Result<(), BudgetOverrun> {
    if text.is_empty() {
        return Ok(());
    }
    if !state.open_text.contains_key(&SOLE_BLOCK) {
        state.ledger.open_block(budget)?;
    }
    state
        .ledger
        .charge_text(budget, u64::try_from(text.len()).unwrap_or(u64::MAX))?;
    state
        .open_text
        .entry(SOLE_BLOCK)
        .or_default()
        .push_str(text);
    Ok(())
}

/// Accumulates `reasoning_content`.
fn append_reasoning(
    state: &mut DialectState,
    budget: &StreamBudget,
    text: &str,
) -> Result<(), BudgetOverrun> {
    if text.is_empty() {
        return Ok(());
    }
    if !state.open_reasoning.contains_key(&SOLE_BLOCK) {
        state.ledger.open_block(budget)?;
    }
    state
        .ledger
        .charge_reasoning(budget, u64::try_from(text.len()).unwrap_or(u64::MAX))?;
    state
        .open_reasoning
        .entry(SOLE_BLOCK)
        .or_default()
        .push_str(text);
    Ok(())
}

/// Applies one `tool_calls[]` fragment.
///
/// The documented rule is that the **first** chunk for an index carries `id` and
/// `function.name` and every later chunk carries only `function.arguments`. A
/// later chunk that repeats either is a protocol violation, not a duplicate to
/// absorb.
fn apply_tool_fragment(
    state: &mut DialectState,
    call: &Value,
    budget: &StreamBudget,
) -> Result<(), FrameDecodeError> {
    let raw_index = number(call, "index")?;
    let index = u16::try_from(raw_index).map_err(|_| FrameDecodeError::MalformedField {
        field: "delta.tool_calls[].index",
    })?;
    let id = call.get("id").and_then(Value::as_str);
    let function = call.get("function");
    let name = function
        .and_then(|value| value.get("name"))
        .and_then(Value::as_str);
    let arguments = function
        .and_then(|value| value.get("arguments"))
        .and_then(Value::as_str)
        .unwrap_or_default();

    if state.open_tools.contains_key(&index) {
        if id.is_some() || name.is_some() {
            return Err(FrameDecodeError::OutOfOrder {
                reason: "only the first tool-call chunk may carry `id` and `function.name`",
            });
        }
        let Some(open) = state.open_tools.get_mut(&index) else {
            return Err(FrameDecodeError::OutOfOrder {
                reason: "a tool-call fragment vanished between lookup and use",
            });
        };
        open.arguments.push_str(arguments);
        let accumulated = open.arguments.len();
        let call_id = open
            .id
            .clone()
            .unwrap_or_else(|| ToolCallId::truncating(""));
        BudgetLedger::check_tool_arguments(budget, &call_id, accumulated)?;
        return Ok(());
    }

    let Some(id) = id else {
        return Err(FrameDecodeError::MalformedField {
            field: "delta.tool_calls[].id",
        });
    };
    let Some(name) = name else {
        return Err(FrameDecodeError::MalformedField {
            field: "delta.tool_calls[].function.name",
        });
    };
    // Call ids are semantic (`search:0`), never UUIDs, so they are carried
    // verbatim into the bounded id rather than re-minted.
    let call_id = ToolCallId::new(id).map_err(|_| FrameDecodeError::MalformedField {
        field: "delta.tool_calls[].id",
    })?;
    state.ledger.open_tool_call(budget)?;
    state.ledger.open_block(budget)?;
    BudgetLedger::check_tool_arguments(budget, &call_id, arguments.len())?;
    state.open_tools.insert(
        index,
        PartialToolCall {
            id: Some(call_id),
            name: Some(name.to_owned()),
            arguments: arguments.to_owned(),
        },
    );
    Ok(())
}

/// Applies one `choices[].delta`.
fn apply_delta(
    state: &mut DialectState,
    delta: &Value,
    budget: &StreamBudget,
) -> Result<(), FrameDecodeError> {
    if let Some(content) = delta.get("content").and_then(Value::as_str) {
        append_text(state, budget, content)?;
    }
    if let Some(reasoning) = delta.get("reasoning_content").and_then(Value::as_str) {
        append_reasoning(state, budget, reasoning)?;
    }
    if let Some(calls) = delta.get("tool_calls").filter(|value| !value.is_null()) {
        let calls = calls.as_array().ok_or(FrameDecodeError::MalformedField {
            field: "delta.tool_calls",
        })?;
        for call in calls {
            apply_tool_fragment(state, call, budget)?;
        }
    }
    Ok(())
}

/// Applies one `choices[0]`, returning the failure an undocumented finish
/// reason produces.
fn apply_choice(
    state: &mut DialectState,
    choice: &Value,
    budget: &StreamBudget,
) -> Result<Option<Box<ProviderFailure>>, FrameDecodeError> {
    if let Some(delta) = choice.get("delta").filter(|value| !value.is_null()) {
        apply_delta(state, delta, budget)?;
    }
    match choice.get("finish_reason") {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(token)) => {
            if stop_for(token).is_none() {
                return Ok(Some(Box::new(undocumented_finish(token))));
            }
            state.finish_token = Some(token.clone());
            Ok(None)
        }
        Some(_) => Err(FrameDecodeError::MalformedField {
            field: "choices[].finish_reason",
        }),
    }
}

/// Decodes one `chat.completion.chunk`.
fn decode_chunk(
    state: &mut DialectState,
    chunk: &Value,
    budget: &StreamBudget,
) -> Result<FrameOutcome, FrameDecodeError> {
    let object = chunk
        .get("object")
        .and_then(Value::as_str)
        .ok_or(FrameDecodeError::MalformedField { field: "object" })?;
    if object != CHUNK_OBJECT {
        return Err(FrameDecodeError::UnknownEvent {
            event: BoundedString::truncating(object),
        });
    }
    if state.request_id.is_none()
        && let Some(id) = chunk.get("id").and_then(Value::as_str)
    {
        state.request_id = ProviderRequestId::new(id).ok();
    }
    if let Some(usage) = chunk.get("usage").filter(|value| !value.is_null()) {
        apply_usage(state, usage)?;
    }
    let choices = chunk
        .get("choices")
        .and_then(Value::as_array)
        .map_or(&[][..], Vec::as_slice);
    if choices.is_empty() {
        // The trailing usage chunk. It is not proof that the provider is
        // generating, so it never moves the effect to `response_started`.
        return Ok(if state.response_started {
            FrameOutcome::Progress
        } else {
            FrameOutcome::Ignored
        });
    }
    if choices.len() > 1 {
        return Err(FrameDecodeError::OutOfOrder {
            reason: "more than one choice arrived; `n` is never sent",
        });
    }
    if let Some(failure) = apply_choice(state, &choices[0], budget)? {
        return Ok(FrameOutcome::Failed(failure));
    }
    Ok(state.mark_started())
}

// ---------------------------------------------------------------------------
// sealing
// ---------------------------------------------------------------------------

/// Bounds accumulated prose without silently truncating it.
fn bounded_text<const N: usize>(
    text: &str,
    overrun: BudgetOverrun,
) -> Result<BoundedString<N>, FrameDecodeError> {
    BoundedString::new(text).map_err(|_| FrameDecodeError::Budget(overrun))
}

/// Assembles the canonical block set: reasoning, then prose, then tool calls.
fn assemble_blocks(state: &DialectState) -> Result<Vec<CanonicalBlock>, FrameDecodeError> {
    let mut blocks = Vec::new();
    for text in state.open_reasoning.values() {
        let limit = u64::try_from(REASON_MAX).unwrap_or(u64::MAX);
        blocks.push(CanonicalBlock::Reasoning(ReasoningBlock {
            body: ReasoningBody::Text {
                text: bounded_text::<REASON_MAX>(text, BudgetOverrun::Reasoning { limit })?,
            },
            // `RecommendedEcho`: the material *is* the reasoning text, which the
            // body already carries, so a duplicate opaque token buys nothing.
            token: None,
        }));
    }
    for text in state.open_text.values() {
        let limit = u64::try_from(TEXT_MAX).unwrap_or(u64::MAX);
        blocks.push(CanonicalBlock::Text {
            text: bounded_text::<TEXT_MAX>(text, BudgetOverrun::Text { limit })?,
            annotations: Vec::new(),
        });
    }
    for open in state.open_tools.values() {
        blocks.push(seal_tool_call(open)?);
    }
    Ok(blocks)
}

/// Turns one reassembled fragment set into a canonical tool-use block.
fn seal_tool_call(open: &PartialToolCall) -> Result<CanonicalBlock, FrameDecodeError> {
    let Some(id) = open.id.clone() else {
        return Err(FrameDecodeError::MalformedField {
            field: "delta.tool_calls[].id",
        });
    };
    let Some(raw_name) = open.name.as_deref() else {
        return Err(FrameDecodeError::MalformedField {
            field: "delta.tool_calls[].function.name",
        });
    };
    let name = ToolName::parse(raw_name).map_err(|_| FrameDecodeError::MalformedField {
        field: "delta.tool_calls[].function.name",
    })?;
    let text = open.arguments.trim();
    let text = if text.is_empty() { "{}" } else { text };
    let input = CanonicalJson::parse(text)
        .map_err(|_| FrameDecodeError::ToolArgumentsNotJson { call: id.clone() })?;
    Ok(CanonicalBlock::ToolUse { id, name, input })
}

// ---------------------------------------------------------------------------
// error classification
// ---------------------------------------------------------------------------

/// Reads `error.type` and `error.message` from a bounded error body.
fn error_fields(body: &BoundedBody) -> (Option<String>, Option<String>) {
    let Some(value) = body.as_json() else {
        return (None, None);
    };
    let error = value.get("error");
    let kind = error
        .and_then(|value| value.get("type"))
        .and_then(Value::as_str)
        .map(str::to_owned);
    let message = error
        .and_then(|value| value.get("message"))
        .and_then(Value::as_str)
        .map(str::to_owned);
    (kind, message)
}

/// The single classification site. `error.type` always wins over the status,
/// which is what splits `429` three ways.
fn classify(status: u16, code: Option<&str>) -> ProviderFailureKind {
    if let Some(code) = code
        && let Some((_, kind)) = ERROR_TYPES.iter().find(|(name, _)| *name == code)
    {
        return *kind;
    }
    ERROR_STATUSES
        .iter()
        .find(|(candidate, _)| *candidate == status)
        .map_or(ProviderFailureKind::ServerError, |(_, kind)| *kind)
}

#[cfg(test)]
mod tests {
    use aex_model_catalog::canonical::{
        CacheBreakpoint, CanonicalBlock, CanonicalMessage, CanonicalToolDef, NormalizedUsage,
        ReasoningBlock, ReasoningBody, ReasoningEffort, ReasoningRequest, ReasoningToken, Role,
        StopReason, StructuredOutputRequest, SystemBlock, ToolChoice, ToolResultPart,
        UsageCompleteness,
    };
    use aex_model_catalog::document::{
        Capability, CapabilitySet, EndpointPin, ModelLimits, NamePattern, ReasoningEncoding,
        ReasoningMode, ReasoningPolicy, ReasoningReplay, SamplingSupport, SchemaEncoding,
        StructuredOutputPolicy, ToolArgumentEncoding, ToolEncoding, ToolPolicy,
    };
    use aex_model_catalog::primitives::{BoundedString, ToolCallId, ToolName};
    use aex_wire::CanonicalJson;
    use aex_wire::provider::ProviderId;
    use serde_json::{Value, json};

    use super::{
        ERROR_STATUSES, ERROR_TYPES, EntryView, GATEWAY_TIMEOUT, MAX_STOP_SEQUENCES, ModelFamily,
        MoonshotAdapter, RequestView, STOP_TOKENS, build, classify, family_of, fresh_state,
        stop_for,
    };
    use crate::adapter::{
        BoundedBody, DialectState, FrameDecodeError, FrameOutcome, HeaderView, ProviderAdapter,
        RequestBuildError, SealedResponse,
    };
    use crate::budget::StreamBudget;
    use crate::error::{ProviderFailure, ProviderFailureClass, ProviderFailureKind};
    use crate::sse::SseEvent;
    use crate::transport::{Accept, AuthScheme, WireRequest};

    // -----------------------------------------------------------------------
    // fixtures
    // -----------------------------------------------------------------------

    fn short(value: &str) -> BoundedString<64> {
        BoundedString::new(value).expect("literal fits")
    }

    fn tool_name(value: &str) -> ToolName {
        ToolName::parse(value).expect("literal is a resource name")
    }

    fn schema() -> CanonicalJson {
        CanonicalJson::parse(r#"{"type":"object"}"#).expect("literal schema")
    }

    /// Every capability, so a test opts *out* of one deliberately.
    fn all_capabilities() -> CapabilitySet {
        CapabilitySet::from_slice(&[
            Capability::TextIn,
            Capability::TextOut,
            Capability::Streaming,
            Capability::Tools,
            Capability::ParallelTools,
            Capability::ToolChoiceRequired,
            Capability::ToolChoiceNamed,
            Capability::ToolChoiceNone,
            Capability::StrictToolSchema,
            Capability::StructuredOutput,
            Capability::Reasoning,
            Capability::StopSequences,
            Capability::Temperature,
            Capability::TopP,
            Capability::SystemInstruction,
        ])
    }

    /// The bounds the plan records for this provider: five stop sequences.
    fn limits() -> ModelLimits {
        ModelLimits {
            context_window_tokens: 256_000,
            max_output_tokens: 8_192,
            min_output_tokens: 1,
            max_reasoning_tokens: None,
            min_reasoning_tokens: None,
            max_tools: 128,
            max_stop_sequences: MAX_STOP_SEQUENCES,
            temperature_milli: Some((0, 1_000)),
            top_p_milli: Some((10, 1_000)),
            min_cacheable_prefix_tokens: 1_024,
            request_body_max_bytes: 32 * 1024 * 1024,
            response_frame_max_bytes: 1024 * 1024,
            stream_idle_timeout_ms: 60_000,
            total_stream_deadline_ms: 900_000,
        }
    }

    fn tool_policy() -> ToolPolicy {
        ToolPolicy {
            encoding: ToolEncoding::OpenAiNestedFunction,
            arguments: ToolArgumentEncoding::JsonString,
            requires_stream_opt_in: false,
            max_name_bytes: 64,
            name_pattern: NamePattern::OpenAiFunctionName,
        }
    }

    fn user_text(value: &str) -> CanonicalMessage {
        CanonicalMessage {
            role: Role::User,
            blocks: vec![CanonicalBlock::Text {
                text: BoundedString::new(value).expect("literal fits"),
                annotations: Vec::new(),
            }],
        }
    }

    fn declared_tool(name: &str) -> CanonicalToolDef {
        CanonicalToolDef {
            name: tool_name(name),
            description: BoundedString::new("does a thing").expect("literal fits"),
            input_schema: schema(),
            strict: false,
        }
    }

    /// Owns everything the two lifted views borrow.
    ///
    /// `QualifiedModel` and therefore `CanonicalModelRequest` can only be minted
    /// inside `aex-model-catalog`, so the builder is exercised through the same
    /// lifted views the trait implementation fills.
    struct Fixture {
        model: String,
        endpoint: EndpointPin,
        capabilities: CapabilitySet,
        limits: ModelLimits,
        sampling: SamplingSupport,
        reasoning_policy: ReasoningPolicy,
        structured_policy: StructuredOutputPolicy,
        policy: ToolPolicy,
        system: Vec<SystemBlock>,
        messages: Vec<CanonicalMessage>,
        tools: Vec<CanonicalToolDef>,
        tool_choice: ToolChoice,
        parallel_tools: bool,
        max_output_tokens: u32,
        temperature_milli: Option<u16>,
        top_p_milli: Option<u16>,
        stop_sequences: Vec<BoundedString<64>>,
        reasoning: ReasoningRequest,
        structured_output: Option<StructuredOutputRequest>,
        cache_breakpoints: Vec<CacheBreakpoint>,
    }

    impl Fixture {
        fn new() -> Self {
            Self {
                model: "kimi-k3".to_owned(),
                endpoint: EndpointPin::MoonshotIntlV1,
                capabilities: all_capabilities(),
                limits: limits(),
                sampling: SamplingSupport::Full,
                reasoning_policy: ReasoningPolicy {
                    mode: ReasoningMode::Optional,
                    encoding: ReasoningEncoding::MoonshotThinking,
                    replay: ReasoningReplay::RecommendedEcho,
                    excludes_sampling: false,
                },
                structured_policy: StructuredOutputPolicy::JsonSchema {
                    encoding: SchemaEncoding::MoonshotResponseFormat,
                    strict_default: true,
                },
                policy: tool_policy(),
                system: Vec::new(),
                messages: vec![user_text("hello")],
                tools: Vec::new(),
                tool_choice: ToolChoice::Auto,
                parallel_tools: true,
                max_output_tokens: 1_024,
                temperature_milli: None,
                top_p_milli: None,
                stop_sequences: Vec::new(),
                reasoning: ReasoningRequest::ProviderDefault,
                structured_output: None,
                cache_breakpoints: Vec::new(),
            }
        }

        fn entry(&self) -> EntryView<'_> {
            EntryView {
                model: &self.model,
                endpoint: self.endpoint,
                capabilities: self.capabilities,
                limits: &self.limits,
                sampling: self.sampling,
                reasoning: self.reasoning_policy,
                structured_output: self.structured_policy,
                tool_policy: &self.policy,
            }
        }

        fn request(&self) -> RequestView<'_> {
            RequestView {
                system: &self.system,
                messages: &self.messages,
                tools: &self.tools,
                tool_choice: &self.tool_choice,
                parallel_tools: self.parallel_tools,
                max_output_tokens: self.max_output_tokens,
                temperature_milli: self.temperature_milli,
                top_p_milli: self.top_p_milli,
                stop_sequences: &self.stop_sequences,
                reasoning: self.reasoning,
                structured_output: self.structured_output.as_ref(),
                cache_breakpoints: &self.cache_breakpoints,
            }
        }

        fn wire(&self) -> Result<WireRequest, RequestBuildError> {
            build(&self.entry(), &self.request())
        }

        fn body(&self) -> Value {
            let wire = self.wire().expect("the fixture builds");
            serde_json::from_slice(&wire.body).expect("the body is JSON")
        }

        fn error(&self) -> RequestBuildError {
            self.wire().expect_err("the fixture must be refused")
        }
    }

    // -----------------------------------------------------------------------
    // request goldens
    // -----------------------------------------------------------------------

    #[test]
    fn the_request_targets_the_pinned_international_origin_over_bearer_auth() {
        let wire = Fixture::new().wire().expect("builds");
        assert_eq!(wire.endpoint, EndpointPin::MoonshotIntlV1);
        assert_eq!(wire.auth, AuthScheme::BearerAuthorization);
        assert_eq!(wire.accept, Accept::TextEventStream);
        let url = wire.url().expect("assembles");
        assert_eq!(
            url.as_str(),
            "https://api.moonshot.ai/v1/chat/completions",
            "the docs host and the China host are both unreachable"
        );
    }

    #[test]
    fn a_request_sends_max_completion_tokens_and_never_max_tokens() {
        let body = Fixture::new().body();
        assert_eq!(body["max_completion_tokens"], json!(1_024));
        assert!(
            body.get("max_tokens").is_none(),
            "`max_tokens` is deprecated on this provider"
        );
    }

    #[test]
    fn a_request_always_sets_stream_options_include_usage() {
        let body = Fixture::new().body();
        assert_eq!(body["stream"], json!(true));
        assert_eq!(body["stream_options"], json!({ "include_usage": true }));
    }

    #[test]
    fn system_blocks_and_turns_encode_as_openai_chat_messages() {
        let mut fixture = Fixture::new();
        fixture.system = vec![SystemBlock {
            text: BoundedString::new("be brief").expect("literal fits"),
            cacheable: false,
        }];
        let body = fixture.body();
        assert_eq!(
            body["messages"],
            json!([
                { "role": "system", "content": "be brief" },
                { "role": "user", "content": "hello" },
            ])
        );
    }

    #[test]
    fn an_assistant_turn_echoes_reasoning_content_and_semantic_tool_call_ids() {
        let mut fixture = Fixture::new();
        fixture.messages = vec![
            user_text("search please"),
            CanonicalMessage {
                role: Role::Assistant,
                blocks: vec![
                    CanonicalBlock::Reasoning(ReasoningBlock {
                        body: ReasoningBody::Text {
                            text: BoundedString::new("the user wants a search")
                                .expect("literal fits"),
                        },
                        token: None,
                    }),
                    CanonicalBlock::ToolUse {
                        id: ToolCallId::new("search:0").expect("semantic id fits"),
                        name: tool_name("search"),
                        input: CanonicalJson::parse(r#"{"q":"rust"}"#).expect("literal"),
                    },
                ],
            },
            CanonicalMessage {
                role: Role::User,
                blocks: vec![CanonicalBlock::ToolResult {
                    call: ToolCallId::new("search:0").expect("semantic id fits"),
                    content: vec![ToolResultPart::Text {
                        text: BoundedString::new("one hit").expect("literal fits"),
                    }],
                    is_error: false,
                }],
            },
        ];
        let body = fixture.body();
        let messages = body["messages"].as_array().expect("array");
        assert_eq!(
            messages[1]["reasoning_content"],
            json!("the user wants a search")
        );
        assert_eq!(messages[1]["tool_calls"][0]["id"], json!("search:0"));
        assert_eq!(
            messages[1]["tool_calls"][0]["function"]["name"],
            json!("search")
        );
        assert_eq!(messages[2]["role"], json!("tool"));
        assert_eq!(messages[2]["tool_call_id"], json!("search:0"));
        assert_eq!(messages[2]["name"], json!("search"));
        assert_eq!(messages[2]["content"], json!("one hit"));
    }

    #[test]
    fn tools_encode_in_the_openai_nested_function_shape() {
        let mut fixture = Fixture::new();
        fixture.tools = vec![declared_tool("search")];
        fixture.tool_choice = ToolChoice::Named {
            name: tool_name("search"),
        };
        let body = fixture.body();
        assert_eq!(body["tools"][0]["type"], json!("function"));
        assert_eq!(body["tools"][0]["function"]["name"], json!("search"));
        assert_eq!(
            body["tool_choice"],
            json!({ "type": "function", "function": { "name": "search" } })
        );
    }

    #[test]
    fn a_json_schema_request_is_encoded_with_strict_true() {
        let mut fixture = Fixture::new();
        fixture.structured_output = Some(StructuredOutputRequest::JsonSchema {
            name: tool_name("answer"),
            schema: schema(),
            strict: true,
        });
        let body = fixture.body();
        assert_eq!(body["response_format"]["type"], json!("json_schema"));
        assert_eq!(
            body["response_format"]["json_schema"]["name"],
            json!("answer")
        );
        assert_eq!(
            body["response_format"]["json_schema"]["strict"],
            json!(true)
        );
    }

    #[test]
    fn a_json_object_request_is_encoded_as_json_object() {
        let mut fixture = Fixture::new();
        fixture.structured_output = Some(StructuredOutputRequest::JsonObject);
        assert_eq!(
            fixture.body()["response_format"],
            json!({ "type": "json_object" })
        );
    }

    #[test]
    fn sampling_is_rendered_as_a_decimal_on_a_moonshot_v1_model() {
        let mut fixture = Fixture::new();
        fixture.model = "moonshot-v1-128k".to_owned();
        fixture.temperature_milli = Some(700);
        fixture.top_p_milli = Some(950);
        let body = fixture.body();
        assert_eq!(body["temperature"], json!(0.7));
        assert_eq!(body["top_p"], json!(0.95));
    }

    #[test]
    fn a_kimi_k3_model_takes_a_reasoning_effort_rung() {
        let mut fixture = Fixture::new();
        fixture.reasoning = ReasoningRequest::Enabled {
            budget_tokens: None,
            effort: Some(ReasoningEffort::High),
        };
        assert_eq!(fixture.body()["reasoning_effort"], json!("high"));
    }

    #[test]
    fn a_kimi_k2_model_takes_a_thinking_object() {
        let mut fixture = Fixture::new();
        fixture.model = "kimi-k2-turbo".to_owned();
        fixture.reasoning = ReasoningRequest::Enabled {
            budget_tokens: None,
            effort: None,
        };
        assert_eq!(fixture.body()["thinking"], json!({ "type": "enabled" }));
    }

    #[test]
    fn a_built_request_carries_only_the_auth_tag_and_no_credential_field() {
        let wire = Fixture::new().wire().expect("builds");
        let rendered = format!("{wire:?}");
        assert!(rendered.contains("BearerAuthorization"));
        assert!(!rendered.contains("sk-"));
        assert_eq!(
            wire.headers,
            vec![(
                "content-type",
                BoundedString::new("application/json").expect("literal")
            )]
        );
    }

    // -----------------------------------------------------------------------
    // the model-gated sampling rule
    // -----------------------------------------------------------------------

    #[test]
    fn a_sampling_field_on_a_non_moonshot_v1_model_is_refused_not_ignored() {
        let mut fixture = Fixture::new();
        fixture.temperature_milli = Some(500);
        assert_eq!(
            fixture.error(),
            RequestBuildError::SamplingUnsupported {
                field: "temperature"
            }
        );
        let mut fixture = Fixture::new();
        fixture.top_p_milli = Some(500);
        assert_eq!(
            fixture.error(),
            RequestBuildError::SamplingUnsupported { field: "top_p" }
        );
    }

    #[test]
    fn top_p_needs_full_sampling_support_even_on_moonshot_v1() {
        let mut fixture = Fixture::new();
        fixture.model = "moonshot-v1-8k".to_owned();
        fixture.sampling = SamplingSupport::TemperatureOnly;
        fixture.top_p_milli = Some(500);
        assert_eq!(
            fixture.error(),
            RequestBuildError::SamplingUnsupported { field: "top_p" }
        );
    }

    #[test]
    fn a_sampling_value_outside_the_declared_range_is_refused() {
        let mut fixture = Fixture::new();
        fixture.model = "moonshot-v1-8k".to_owned();
        fixture.temperature_milli = Some(1_500);
        assert_eq!(
            fixture.error(),
            RequestBuildError::SamplingUnsupported {
                field: "temperature"
            }
        );
    }

    #[test]
    fn the_model_family_split_follows_the_documented_slugs() {
        assert_eq!(family_of("moonshot-v1-128k"), ModelFamily::MoonshotV1);
        assert_eq!(family_of("kimi-k3"), ModelFamily::KimiEffort);
        assert_eq!(family_of("kimi-k2-turbo"), ModelFamily::KimiThinking);
    }

    // -----------------------------------------------------------------------
    // the remaining request-build refusals
    // -----------------------------------------------------------------------

    #[test]
    fn more_than_five_stop_sequences_are_refused() {
        let mut fixture = Fixture::new();
        fixture.stop_sequences = (0..6).map(|index| short(&format!("s{index}"))).collect();
        assert_eq!(
            fixture.error(),
            RequestBuildError::StopSequenceLimit {
                max: MAX_STOP_SEQUENCES
            }
        );
    }

    #[test]
    fn exactly_five_stop_sequences_fit() {
        let mut fixture = Fixture::new();
        fixture.stop_sequences = (0..5).map(|index| short(&format!("s{index}"))).collect();
        assert_eq!(fixture.body()["stop"].as_array().expect("array").len(), 5);
    }

    #[test]
    fn a_catalog_bound_wider_than_five_is_still_clamped_to_the_dialect_ceiling() {
        let mut fixture = Fixture::new();
        fixture.limits.max_stop_sequences = 16;
        fixture.stop_sequences = (0..6).map(|index| short(&format!("s{index}"))).collect();
        assert_eq!(
            fixture.error(),
            RequestBuildError::StopSequenceLimit {
                max: MAX_STOP_SEQUENCES
            },
            "a document may narrow a dialect bound, never widen it"
        );
    }

    #[test]
    fn a_narrower_catalog_bound_wins_over_the_dialect_ceiling() {
        let mut fixture = Fixture::new();
        fixture.limits.max_stop_sequences = 2;
        fixture.stop_sequences = vec![short("a"), short("b"), short("c")];
        assert_eq!(
            fixture.error(),
            RequestBuildError::StopSequenceLimit { max: 2 }
        );
    }

    #[test]
    fn stop_sequences_need_the_declared_capability() {
        let mut fixture = Fixture::new();
        fixture.capabilities = CapabilitySet::from_slice(&[Capability::Streaming]);
        fixture.stop_sequences = vec![short("a")];
        assert_eq!(
            fixture.error(),
            RequestBuildError::CapabilityUnavailable {
                capability: Capability::StopSequences
            }
        );
    }

    #[test]
    fn a_pair_that_does_not_declare_streaming_is_refused_before_dispatch() {
        let mut fixture = Fixture::new();
        fixture.capabilities = CapabilitySet::EMPTY;
        assert_eq!(
            fixture.error(),
            RequestBuildError::CapabilityUnavailable {
                capability: Capability::Streaming
            }
        );
    }

    #[test]
    fn an_explicit_cache_breakpoint_is_refused_because_the_cache_is_implicit() {
        let mut fixture = Fixture::new();
        fixture.cache_breakpoints = vec![CacheBreakpoint::AfterSystem];
        assert_eq!(
            fixture.error(),
            RequestBuildError::CapabilityUnavailable {
                capability: Capability::PromptCacheExplicit
            }
        );
    }

    #[test]
    fn an_output_ceiling_outside_the_declared_range_is_refused() {
        let mut fixture = Fixture::new();
        fixture.max_output_tokens = 100_000;
        assert_eq!(
            fixture.error(),
            RequestBuildError::OutputTokensOutOfRange { min: 1, max: 8_192 }
        );
    }

    #[test]
    fn a_body_over_the_declared_bound_is_refused() {
        let mut fixture = Fixture::new();
        fixture.limits.request_body_max_bytes = 32;
        assert_eq!(
            fixture.error(),
            RequestBuildError::BodyTooLarge { limit: 32 }
        );
    }

    #[test]
    fn more_tools_than_the_declared_bound_are_refused() {
        let mut fixture = Fixture::new();
        fixture.limits.max_tools = 1;
        fixture.tools = vec![declared_tool("one"), declared_tool("two")];
        assert_eq!(fixture.error(), RequestBuildError::ToolLimit { max: 1 });
    }

    #[test]
    fn a_tool_name_the_grammar_rejects_is_refused() {
        let mut fixture = Fixture::new();
        let mut declared = declared_tool("search");
        declared.name = tool_name("a.dotted.name");
        fixture.tools = vec![declared];
        assert_eq!(
            fixture.error(),
            RequestBuildError::ToolNameInvalid {
                name: tool_name("a.dotted.name")
            }
        );
    }

    #[test]
    fn a_strict_tool_needs_the_strict_schema_capability() {
        let mut fixture = Fixture::new();
        fixture.capabilities = CapabilitySet::from_slice(&[
            Capability::Streaming,
            Capability::Tools,
            Capability::ParallelTools,
        ]);
        let mut declared = declared_tool("search");
        declared.strict = true;
        fixture.tools = vec![declared];
        assert_eq!(
            fixture.error(),
            RequestBuildError::CapabilityUnavailable {
                capability: Capability::StrictToolSchema
            }
        );
    }

    #[test]
    fn an_undeclared_tool_choice_mode_is_refused() {
        let mut fixture = Fixture::new();
        fixture.capabilities = CapabilitySet::from_slice(&[
            Capability::Streaming,
            Capability::Tools,
            Capability::ParallelTools,
        ]);
        fixture.tools = vec![declared_tool("search")];
        fixture.tool_choice = ToolChoice::Required;
        assert_eq!(
            fixture.error(),
            RequestBuildError::ToolChoiceUnsupported {
                requested: Box::new(ToolChoice::Required)
            }
        );
    }

    #[test]
    fn a_named_tool_choice_without_tools_is_refused() {
        let mut fixture = Fixture::new();
        fixture.tool_choice = ToolChoice::Named {
            name: tool_name("search"),
        };
        assert!(matches!(
            fixture.error(),
            RequestBuildError::ToolChoiceUnsupported { .. }
        ));
    }

    #[test]
    fn a_schema_request_against_a_json_object_only_pair_is_refused() {
        let mut fixture = Fixture::new();
        fixture.structured_policy = StructuredOutputPolicy::JsonObjectOnly;
        fixture.structured_output = Some(StructuredOutputRequest::JsonSchema {
            name: tool_name("answer"),
            schema: schema(),
            strict: true,
        });
        assert!(matches!(
            fixture.error(),
            RequestBuildError::StructuredOutputUnsupported { .. }
        ));
    }

    #[test]
    fn a_foreign_schema_encoding_is_refused() {
        let mut fixture = Fixture::new();
        fixture.structured_policy = StructuredOutputPolicy::JsonSchema {
            encoding: SchemaEncoding::OpenAiTextFormat,
            strict_default: true,
        };
        fixture.structured_output = Some(StructuredOutputRequest::JsonSchema {
            name: tool_name("answer"),
            schema: schema(),
            strict: true,
        });
        assert!(matches!(
            fixture.error(),
            RequestBuildError::StructuredOutputUnsupported { .. }
        ));
    }

    #[test]
    fn reasoning_material_from_another_provider_cannot_be_replayed() {
        let mut fixture = Fixture::new();
        fixture.messages = vec![
            user_text("hi"),
            CanonicalMessage {
                role: Role::Assistant,
                blocks: vec![
                    CanonicalBlock::Reasoning(ReasoningBlock {
                        body: ReasoningBody::Redacted,
                        token: Some(ReasoningToken {
                            provenance: ProviderId::Anthropic,
                            bytes: bytes::Bytes::from_static(b"signature"),
                        }),
                    }),
                    CanonicalBlock::Text {
                        text: BoundedString::new("hello").expect("literal fits"),
                        annotations: Vec::new(),
                    },
                ],
            },
        ];
        assert_eq!(
            fixture.error(),
            RequestBuildError::ReasoningProvenanceMismatch {
                expected: ProviderId::Moonshotai,
                found: ProviderId::Anthropic,
            }
        );
    }

    #[test]
    fn a_reasoning_token_budget_is_refused_because_this_dialect_takes_an_effort() {
        let mut fixture = Fixture::new();
        fixture.reasoning = ReasoningRequest::Enabled {
            budget_tokens: Some(2_048),
            effort: None,
        };
        assert_eq!(
            fixture.error(),
            RequestBuildError::Encoding {
                reason: "this dialect takes an effort level, not a reasoning-token budget"
            }
        );
    }

    #[test]
    fn a_kimi_k3_model_cannot_turn_reasoning_off() {
        let mut fixture = Fixture::new();
        fixture.reasoning = ReasoningRequest::Disabled;
        assert_eq!(
            fixture.error(),
            RequestBuildError::Encoding {
                reason: "this model has no reasoning-off setting"
            }
        );
    }

    #[test]
    fn an_always_on_pair_cannot_be_asked_to_stop_reasoning() {
        let mut fixture = Fixture::new();
        fixture.model = "kimi-k2-turbo".to_owned();
        fixture.reasoning_policy.mode = ReasoningMode::AlwaysOn;
        fixture.reasoning = ReasoningRequest::Disabled;
        assert_eq!(
            fixture.error(),
            RequestBuildError::Encoding {
                reason: "this pair cannot disable reasoning"
            }
        );
    }

    #[test]
    fn reasoning_on_a_moonshot_v1_model_is_a_missing_capability() {
        let mut fixture = Fixture::new();
        fixture.model = "moonshot-v1-8k".to_owned();
        fixture.reasoning = ReasoningRequest::Enabled {
            budget_tokens: None,
            effort: Some(ReasoningEffort::High),
        };
        assert_eq!(
            fixture.error(),
            RequestBuildError::CapabilityUnavailable {
                capability: Capability::Reasoning
            }
        );
    }

    #[test]
    fn a_foreign_endpoint_pin_is_refused() {
        let mut fixture = Fixture::new();
        fixture.endpoint = EndpointPin::DeepSeekApi;
        assert_eq!(
            fixture.error(),
            RequestBuildError::Encoding {
                reason: "this dialect speaks only to the pinned international origin"
            }
        );
    }

    #[test]
    fn disabling_parallel_tool_calls_is_refused_rather_than_dropped() {
        let mut fixture = Fixture::new();
        fixture.tools = vec![declared_tool("search")];
        fixture.parallel_tools = false;
        assert_eq!(
            fixture.error(),
            RequestBuildError::Encoding {
                reason: "this dialect has no parallel-tool-calls switch"
            }
        );
    }

    #[test]
    fn a_tool_result_answering_no_known_call_is_refused() {
        let mut fixture = Fixture::new();
        fixture.messages = vec![CanonicalMessage {
            role: Role::User,
            blocks: vec![CanonicalBlock::ToolResult {
                call: ToolCallId::new("search:0").expect("id fits"),
                content: Vec::new(),
                is_error: false,
            }],
        }];
        assert_eq!(
            fixture.error(),
            RequestBuildError::Encoding {
                reason: "a tool result names no tool call in this conversation"
            }
        );
    }

    #[test]
    fn a_request_with_no_messages_at_all_is_refused() {
        let mut fixture = Fixture::new();
        fixture.messages = Vec::new();
        assert_eq!(
            fixture.error(),
            RequestBuildError::Encoding {
                reason: "the request carried no messages"
            }
        );
    }

    // -----------------------------------------------------------------------
    // frame taxonomy
    // -----------------------------------------------------------------------

    fn event(data: &str) -> SseEvent<'_> {
        SseEvent {
            name: None,
            data: data.as_bytes(),
            id: None,
        }
    }

    fn feed(state: &mut DialectState, data: &str) -> Result<FrameOutcome, FrameDecodeError> {
        MoonshotAdapter.decode(state, &event(data), &StreamBudget::default())
    }

    fn content_chunk(text: &str) -> String {
        format!(
            r#"{{"id":"chatcmpl-1","object":"chat.completion.chunk","choices":[{{"index":0,"delta":{{"content":"{text}"}},"finish_reason":null}}]}}"#
        )
    }

    fn finish_chunk(reason: &str) -> String {
        format!(
            r#"{{"id":"chatcmpl-1","object":"chat.completion.chunk","choices":[{{"index":0,"delta":{{}},"finish_reason":"{reason}"}}]}}"#
        )
    }

    const USAGE_CHUNK: &str = r#"{"id":"chatcmpl-1","object":"chat.completion.chunk","choices":[],"usage":{"prompt_tokens":100,"completion_tokens":20,"total_tokens":120,"cached_tokens":40}}"#;

    fn run(script: &[&str]) -> Result<SealedResponse, FrameDecodeError> {
        let mut state = fresh_state();
        for frame in script {
            feed(&mut state, frame)?;
        }
        MoonshotAdapter.finish(state)
    }

    #[test]
    fn the_first_chunk_with_a_choice_starts_the_response_exactly_once() {
        let mut state = fresh_state();
        assert_eq!(
            feed(&mut state, &content_chunk("a")).expect("decodes"),
            FrameOutcome::ResponseStarted
        );
        assert_eq!(
            feed(&mut state, &content_chunk("b")).expect("decodes"),
            FrameOutcome::Progress
        );
    }

    #[test]
    fn a_usage_only_chunk_before_any_content_does_not_start_the_response() {
        let mut state = fresh_state();
        assert_eq!(
            feed(&mut state, USAGE_CHUNK).expect("decodes"),
            FrameOutcome::Ignored
        );
        assert!(!state.response_started);
    }

    #[test]
    fn the_done_sentinel_is_the_terminal_frame() {
        let mut state = fresh_state();
        assert_eq!(
            feed(&mut state, "[DONE]").expect("decodes"),
            FrameOutcome::Terminal
        );
        assert!(state.terminal);
    }

    #[test]
    fn an_unknown_chunk_object_is_a_frame_this_dialect_does_not_send() {
        let mut state = fresh_state();
        let error =
            feed(&mut state, r#"{"object":"chat.completion","choices":[]}"#).expect_err("refused");
        assert!(matches!(error, FrameDecodeError::UnknownEvent { .. }));
    }

    #[test]
    fn a_named_sse_event_is_refused_because_this_dialect_is_data_only() {
        let mut state = fresh_state();
        let named = SseEvent {
            name: Some("message"),
            data: b"{}",
            id: None,
        };
        let error = MoonshotAdapter
            .decode(&mut state, &named, &StreamBudget::default())
            .expect_err("refused");
        assert!(matches!(error, FrameDecodeError::UnknownEvent { .. }));
    }

    #[test]
    fn a_frame_that_is_not_json_is_refused() {
        let mut state = fresh_state();
        assert_eq!(
            feed(&mut state, "not json").expect_err("refused"),
            FrameDecodeError::NotJson
        );
    }

    #[test]
    fn a_chunk_without_an_object_discriminant_is_malformed() {
        let mut state = fresh_state();
        assert_eq!(
            feed(&mut state, r#"{"choices":[]}"#).expect_err("refused"),
            FrameDecodeError::MalformedField { field: "object" }
        );
    }

    #[test]
    fn more_than_one_choice_is_out_of_order_because_n_is_never_sent() {
        let mut state = fresh_state();
        let error = feed(
            &mut state,
            r#"{"object":"chat.completion.chunk","choices":[{"index":0,"delta":{}},{"index":1,"delta":{}}]}"#,
        )
        .expect_err("refused");
        assert!(matches!(error, FrameDecodeError::OutOfOrder { .. }));
    }

    #[test]
    fn a_stream_that_never_sends_done_cannot_be_sealed() {
        let mut state = fresh_state();
        feed(&mut state, &content_chunk("a")).expect("decodes");
        feed(&mut state, &finish_chunk("stop")).expect("decodes");
        let error = MoonshotAdapter.finish(state).expect_err("refused");
        assert!(matches!(error, FrameDecodeError::OutOfOrder { .. }));
    }

    #[test]
    fn a_stream_that_never_sends_a_finish_reason_cannot_be_sealed() {
        let error = run(&[&content_chunk("a"), "[DONE]"]).expect_err("refused");
        assert_eq!(
            error,
            FrameDecodeError::MalformedField {
                field: "choices[].finish_reason"
            }
        );
    }

    #[test]
    fn a_complete_text_stream_seals_as_end_turn() {
        let sealed = run(&[
            &content_chunk("hel"),
            &content_chunk("lo"),
            &finish_chunk("stop"),
            USAGE_CHUNK,
            "[DONE]",
        ])
        .expect("seals");
        assert_eq!(sealed.stop_reason, StopReason::EndTurn);
        assert_eq!(
            sealed.blocks,
            vec![CanonicalBlock::Text {
                text: BoundedString::new("hello").expect("literal fits"),
                annotations: Vec::new(),
            }]
        );
        assert_eq!(
            sealed
                .provider_request_id
                .as_ref()
                .map(BoundedString::as_str),
            Some("chatcmpl-1")
        );
    }

    #[test]
    fn reasoning_content_seals_as_a_reasoning_block_with_no_opaque_token() {
        let reasoning = r#"{"object":"chat.completion.chunk","choices":[{"index":0,"delta":{"reasoning_content":"thinking"}}]}"#;
        let sealed = run(&[
            reasoning,
            &content_chunk("done"),
            &finish_chunk("stop"),
            "[DONE]",
        ])
        .expect("seals");
        assert_eq!(
            sealed.blocks[0],
            CanonicalBlock::Reasoning(ReasoningBlock {
                body: ReasoningBody::Text {
                    text: BoundedString::new("thinking").expect("literal fits"),
                },
                token: None,
            })
        );
    }

    #[test]
    fn a_stream_that_produced_no_content_cannot_be_sealed() {
        let error = run(&[&finish_chunk("stop"), "[DONE]"]).expect_err("refused");
        assert!(matches!(error, FrameDecodeError::OutOfOrder { .. }));
    }

    #[test]
    fn an_undocumented_finish_reason_is_a_protocol_violation() {
        let mut state = fresh_state();
        feed(&mut state, &content_chunk("a")).expect("decodes");
        let outcome = feed(&mut state, &finish_chunk("content_filter")).expect("decodes");
        let FrameOutcome::Failed(failure) = &outcome else {
            panic!("an undocumented finish reason must fail the frame: {outcome:?}");
        };
        assert_eq!(failure.kind(), ProviderFailureKind::ProtocolViolation);
        assert_eq!(failure.class(), ProviderFailureClass::Permanent);
        assert_eq!(
            failure
                .detail
                .provider_code
                .as_ref()
                .map(BoundedString::as_str),
            Some("content_filter")
        );
    }

    // -----------------------------------------------------------------------
    // tool-call reassembly
    // -----------------------------------------------------------------------

    fn tool_open(id: &str, name: &str, arguments: &str) -> String {
        format!(
            r#"{{"object":"chat.completion.chunk","choices":[{{"index":0,"delta":{{"tool_calls":[{{"index":0,"id":"{id}","type":"function","function":{{"name":"{name}","arguments":{arguments}}}}}]}}}}]}}"#
        )
    }

    fn tool_more(arguments: &str) -> String {
        format!(
            r#"{{"object":"chat.completion.chunk","choices":[{{"index":0,"delta":{{"tool_calls":[{{"index":0,"function":{{"arguments":{arguments}}}}}]}}}}]}}"#
        )
    }

    #[test]
    fn tool_arguments_reassemble_from_fragments() {
        let sealed = run(&[
            &tool_open("search:0", "search", r#""{\"q\":""#),
            &tool_more(r#""\"rust\"}""#),
            &finish_chunk("tool_calls"),
            "[DONE]",
        ])
        .expect("seals");
        let CanonicalBlock::ToolUse { input, .. } = &sealed.blocks[0] else {
            panic!("expected a tool use, got {:?}", sealed.blocks[0]);
        };
        assert_eq!(input.as_str(), r#"{"q":"rust"}"#);
        assert_eq!(sealed.stop_reason, StopReason::ToolUse);
    }

    #[test]
    fn a_fragment_boundary_inside_a_string_escape_reassembles() {
        // The split lands between a backslash and the character it escapes.
        let sealed = run(&[
            &tool_open("search:0", "search", r#""{\"q\":\"a\\""#),
            &tool_more(r#""\"b\"}""#),
            &finish_chunk("tool_calls"),
            "[DONE]",
        ])
        .expect("seals");
        let CanonicalBlock::ToolUse { input, .. } = &sealed.blocks[0] else {
            panic!("expected a tool use");
        };
        assert_eq!(input.as_str(), r#"{"q":"a\"b"}"#);
    }

    #[test]
    fn a_semantic_tool_call_id_is_carried_verbatim() {
        let sealed = run(&[
            &tool_open("search:0", "search", r#""{}""#),
            &finish_chunk("tool_calls"),
            "[DONE]",
        ])
        .expect("seals");
        let CanonicalBlock::ToolUse { id, name, .. } = &sealed.blocks[0] else {
            panic!("expected a tool use");
        };
        assert_eq!(
            id.as_str(),
            "search:0",
            "a semantic id is not a UUID and is never re-minted"
        );
        assert_eq!(name.as_str(), "search");
    }

    #[test]
    fn only_the_first_tool_call_chunk_may_carry_the_id_and_name() {
        let mut state = fresh_state();
        feed(&mut state, &tool_open("search:0", "search", r#""{}""#)).expect("decodes");
        let repeat = r#"{"object":"chat.completion.chunk","choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"id":"search:0","function":{"arguments":""}}]}}]}"#;
        let error = feed(&mut state, repeat).expect_err("refused");
        assert_eq!(
            error,
            FrameDecodeError::OutOfOrder {
                reason: "only the first tool-call chunk may carry `id` and `function.name`"
            }
        );
    }

    #[test]
    fn a_first_tool_call_chunk_without_an_id_is_malformed() {
        let mut state = fresh_state();
        let error = feed(&mut state, &tool_more(r#""{}""#)).expect_err("refused");
        assert_eq!(
            error,
            FrameDecodeError::MalformedField {
                field: "delta.tool_calls[].id"
            }
        );
    }

    #[test]
    fn tool_arguments_that_never_become_json_are_refused() {
        let error = run(&[
            &tool_open("search:0", "search", r#""{\"q\":""#),
            &finish_chunk("tool_calls"),
            "[DONE]",
        ])
        .expect_err("refused");
        assert!(matches!(
            error,
            FrameDecodeError::ToolArgumentsNotJson { .. }
        ));
    }

    #[test]
    fn tool_calls_and_the_finish_reason_must_agree() {
        let error = run(&[
            &tool_open("search:0", "search", r#""{}""#),
            &finish_chunk("stop"),
            "[DONE]",
        ])
        .expect_err("refused");
        assert_eq!(
            error,
            FrameDecodeError::OutOfOrder {
                reason: "tool calls and the finish reason disagree"
            }
        );
    }

    // -----------------------------------------------------------------------
    // usage
    // -----------------------------------------------------------------------

    #[test]
    fn usage_maps_cached_tokens_out_of_the_input_count() {
        let sealed = run(&[
            &content_chunk("a"),
            &finish_chunk("stop"),
            USAGE_CHUNK,
            "[DONE]",
        ])
        .expect("seals");
        assert_eq!(
            sealed.usage,
            NormalizedUsage {
                input_tokens: 60,
                cache_read_input_tokens: 40,
                cache_write_input_tokens: 0,
                output_tokens: 20,
                reasoning_tokens: 0,
                tool_use_prompt_tokens: 0,
                provider_total_tokens: Some(120),
                completeness: UsageCompleteness::Exact,
            }
        );
        assert!(sealed.usage.is_consistent());
    }

    #[test]
    fn a_stream_with_no_usage_chunk_records_the_absence() {
        let sealed = run(&[&content_chunk("a"), &finish_chunk("stop"), "[DONE]"]).expect("seals");
        assert_eq!(sealed.usage.completeness, UsageCompleteness::Absent);
        assert_eq!(sealed.usage.input_tokens, 0, "usage is never invented");
    }

    #[test]
    fn a_usage_object_missing_a_required_count_is_malformed() {
        let mut state = fresh_state();
        let error = feed(
            &mut state,
            r#"{"object":"chat.completion.chunk","choices":[],"usage":{"prompt_tokens":1}}"#,
        )
        .expect_err("refused");
        assert_eq!(
            error,
            FrameDecodeError::MalformedField {
                field: "completion_tokens"
            }
        );
    }

    // -----------------------------------------------------------------------
    // the two compiled tables
    // -----------------------------------------------------------------------

    #[test]
    fn the_stop_mapping_table_is_exhaustive_and_closed() {
        assert_eq!(
            STOP_TOKENS,
            [
                ("stop", StopReason::EndTurn),
                ("tool_calls", StopReason::ToolUse),
                ("length", StopReason::MaxOutputTokens),
            ]
        );
        for (token, expected) in STOP_TOKENS {
            assert_eq!(stop_for(token), Some(expected));
        }
        for absent in [
            "content_filter",
            "sensitive",
            "insufficient_system_resource",
            "STOP",
            "",
        ] {
            assert_eq!(
                stop_for(absent),
                None,
                "`{absent}` is outside the documented enum"
            );
        }
    }

    #[test]
    fn the_error_mapping_table_is_exhaustive() {
        for (code, expected) in ERROR_TYPES {
            assert_eq!(
                classify(200, Some(code)),
                expected,
                "`{code}` maps to the wrong kind"
            );
        }
        for (status, expected) in ERROR_STATUSES {
            assert_eq!(
                classify(status, None),
                expected,
                "status {status} maps to the wrong kind"
            );
        }
        assert_eq!(
            classify(418, Some("something_new")),
            ProviderFailureKind::ServerError,
            "an unnamed status and an unnamed type fall back together"
        );
    }

    // -----------------------------------------------------------------------
    // the three-way 429 split
    // -----------------------------------------------------------------------

    fn refuse(status: u16, kind: &str, retry_after: Option<&str>) -> ProviderFailure {
        let body = BoundedBody::new(
            format!(r#"{{"error":{{"type":"{kind}","message":"nope"}}}}"#).into_bytes(),
            false,
        );
        let mut headers = reqwest::header::HeaderMap::new();
        if let Some(value) = retry_after {
            headers.insert("retry-after", value.parse().expect("header value"));
        }
        MoonshotAdapter.classify_http(status, &HeaderView::new(&headers), &body)
    }

    #[test]
    fn an_engine_overloaded_429_is_overloaded_and_retryable() {
        let failure = refuse(429, "engine_overloaded_error", None);
        assert_eq!(failure.kind(), ProviderFailureKind::Overloaded);
        assert_eq!(failure.class(), ProviderFailureClass::Overloaded);
        assert!(failure.kind().is_in_call_retryable());
    }

    #[test]
    fn a_rate_limit_reached_429_is_rate_limited_and_retryable() {
        let failure = refuse(429, "rate_limit_reached_error", Some("12"));
        assert_eq!(failure.kind(), ProviderFailureKind::RateLimited);
        assert_eq!(failure.class(), ProviderFailureClass::Overloaded);
        assert!(failure.kind().is_in_call_retryable());
        assert_eq!(
            failure.rate_limit.retry_after,
            Some(core::time::Duration::from_secs(12))
        );
    }

    #[test]
    fn an_exceeded_quota_429_is_permanent_and_never_retried() {
        let failure = refuse(429, "exceeded_current_quota_error", None);
        assert_eq!(failure.kind(), ProviderFailureKind::Quota);
        assert_eq!(
            failure.class(),
            ProviderFailureClass::Permanent,
            "retrying an exhausted quota is futile"
        );
        assert!(!failure.kind().is_in_call_retryable());
    }

    #[test]
    fn the_three_429_causes_do_not_collapse_into_one_class() {
        let classes = [
            refuse(429, "engine_overloaded_error", None).class(),
            refuse(429, "rate_limit_reached_error", None).class(),
            refuse(429, "exceeded_current_quota_error", None).class(),
        ];
        assert_eq!(
            classes,
            [
                ProviderFailureClass::Overloaded,
                ProviderFailureClass::Overloaded,
                ProviderFailureClass::Permanent,
            ]
        );
    }

    #[test]
    fn the_error_body_names_its_failure_in_type_not_code() {
        let body = BoundedBody::new(
            br#"{"error":{"code":"exceeded_current_quota_error","message":"nope"}}"#.to_vec(),
            false,
        );
        let headers = reqwest::header::HeaderMap::new();
        let failure = MoonshotAdapter.classify_http(429, &HeaderView::new(&headers), &body);
        assert_eq!(
            failure.kind(),
            ProviderFailureKind::RateLimited,
            "`code` is not this provider's discriminator, so the status decides"
        );
    }

    #[test]
    fn a_credential_echoed_into_an_error_body_is_redacted() {
        let key = "sk-0123456789abcdefghijklmnopqrstuvwxyz";
        let body = BoundedBody::new(
            format!(
                r#"{{"error":{{"type":"incorrect_api_key_error","message":"bad key {key}"}}}}"#
            )
            .into_bytes(),
            false,
        );
        let headers = reqwest::header::HeaderMap::new();
        let failure = MoonshotAdapter.classify_http(401, &HeaderView::new(&headers), &body);
        assert_eq!(failure.kind(), ProviderFailureKind::Authentication);
        assert!(!failure.detail.message.as_str().contains(key));
    }

    #[test]
    fn an_unparsable_error_body_still_classifies_by_status() {
        let body = BoundedBody::new(b"<html>502</html>".to_vec(), true);
        let headers = reqwest::header::HeaderMap::new();
        let failure = MoonshotAdapter.classify_http(503, &HeaderView::new(&headers), &body);
        assert_eq!(failure.kind(), ProviderFailureKind::Overloaded);
        assert_eq!(failure.detail.provider_code, None);
    }

    #[test]
    fn retry_after_is_the_only_published_backpressure() {
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert(
            "x-ratelimit-remaining-requests",
            "7".parse().expect("value"),
        );
        let feedback = MoonshotAdapter.rate_limit_feedback(&HeaderView::new(&headers));
        assert!(
            feedback.is_absent(),
            "this provider publishes no `x-ratelimit-*` family"
        );
        headers.insert("retry-after", "3".parse().expect("value"));
        let feedback = MoonshotAdapter.rate_limit_feedback(&HeaderView::new(&headers));
        assert_eq!(
            feedback.retry_after,
            Some(core::time::Duration::from_secs(3))
        );
        assert_eq!(feedback.requests_remaining, None);
    }

    // -----------------------------------------------------------------------
    // adapter identity
    // -----------------------------------------------------------------------

    #[test]
    fn the_adapter_speaks_for_exactly_one_provider() {
        assert_eq!(MoonshotAdapter.provider(), ProviderId::Moonshotai);
    }

    #[test]
    fn the_source_digest_is_stable_across_calls() {
        assert_eq!(
            MoonshotAdapter.source_digest(),
            MoonshotAdapter.source_digest()
        );
    }

    #[test]
    fn the_documented_gateway_timeout_is_nine_hundred_seconds() {
        assert_eq!(GATEWAY_TIMEOUT.as_secs(), 900);
    }

    #[test]
    fn a_request_id_header_wins_over_the_chunk_id() {
        let mut state = fresh_state();
        feed(&mut state, &content_chunk("a")).expect("decodes");
        let mut headers = reqwest::header::HeaderMap::new();
        assert_eq!(
            MoonshotAdapter
                .request_id(&HeaderView::new(&headers), &state)
                .as_ref()
                .map(BoundedString::as_str),
            Some("chatcmpl-1")
        );
        headers.insert("x-request-id", "req_9".parse().expect("value"));
        assert_eq!(
            MoonshotAdapter
                .request_id(&HeaderView::new(&headers), &state)
                .as_ref()
                .map(BoundedString::as_str),
            Some("req_9")
        );
    }
}
