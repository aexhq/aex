//! The `DeepSeek` chat-completions dialect (plan 08 §5.3).
//!
//! One surface: `POST /chat/completions` on [`EndpointPin::DeepSeekApi`] with
//! `Authorization: Bearer …`. The `/v1` suffix is compatibility only; `/beta`,
//! `/anthropic` and the Responses surface are launch exclusions and are
//! unreachable from configuration because the origin is compiled.
//!
//! # Facts this module encodes, and the traps it exists to survive
//!
//! - **Usage arrives in a trailing chunk whose `choices` array is empty.**
//!   Every other chunk carries `usage: null`, so a decoder that merged a null
//!   would zero the tally on the way past. Merging is skipped for anything that
//!   is not an object.
//! - **Reasoning is on by default and forbids sampling.** `temperature` and
//!   `top_p` are refused before dispatch rather than sent and ignored, and the
//!   deprecated `frequency_penalty`/`presence_penalty` are never emitted at all.
//! - **`reasoning_effort` is sent twice on purpose** (D-24): nested inside
//!   `thinking` as the reference documents it, and at the top level as the guide
//!   shows it. Probe R-2 records which the server honours.
//! - **Structured output is `json_object` only**, and the documented "the word
//!   `json` must appear in the prompt" rule is a caller obligation surfaced as
//!   [`RequestBuildError::StructuredOutputPromptRequirement`] rather than
//!   silently patched into the prompt.
//! - **No rate-limit feedback exists.** `DeepSeek` publishes no `x-ratelimit-*`
//!   and no `Retry-After`; concurrency is the published control and lives on the
//!   entry as `concurrency_hint`. [`RateLimitSource::NotProvided`] is therefore a
//!   positive record, not an omission.
//! - **The error body shape is undocumented.** An `OpenAI`-shaped body is parsed
//!   opportunistically and never depended on: classification comes from the
//!   status alone when the body does not parse.
//!
//! # What this module deliberately does not do
//!
//! It does not estimate prompt tokens, so it never raises
//! [`RequestBuildError::ContextOverflowEstimated`]. `DeepSeek` publishes a 1M
//! context window and no tokenizer; inventing an estimate would reject valid
//! requests on a guess. The provider's own 400 is the authority.
//!
//! [`RateLimitSource::NotProvided`]: crate::error::RateLimitSource::NotProvided

use aex_model_catalog::QualifiedModel;
use aex_model_catalog::canonical::{
    CanonicalBlock, CanonicalMessage, CanonicalModelRequest, CanonicalToolDef, NormalizedUsage,
    REASON_MAX, ReasoningBlock, ReasoningBody, ReasoningEffort, ReasoningRequest, ReasoningToken,
    Role, StopReason, StructuredOutputRequest, SystemBlock, TEXT_MAX, ToolChoice, ToolResultPart,
    UsageCompleteness, UsageField, UsageFieldSet,
};
use aex_model_catalog::document::{
    Capability, Dialect, EndpointPin, EntryState, ModelEntry, ReasoningReplay, SamplingSupport,
    StructuredOutputPolicy,
};
use aex_model_catalog::primitives::{BoundedString, ProviderRequestId, ToolCallId, ToolName};
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

/// The one path this dialect posts to.
pub const CHAT_COMPLETIONS_PATH: &str = "/chat/completions";

/// The `object` discriminant every streamed chunk carries.
pub const CHUNK_OBJECT: &str = "chat.completion.chunk";

/// The most stop sequences `DeepSeek` accepts.
pub const MAX_STOP_SEQUENCES: u8 = 16;

/// The most tool declarations `DeepSeek` accepts.
pub const MAX_TOOLS: u16 = 128;

/// The widest temperature `DeepSeek` accepts, in milli-units (`0`–`2`).
pub const MAX_TEMPERATURE_MILLI: u16 = 2_000;

/// The effort `DeepSeek` reasons at when the caller names none.
pub const DEFAULT_REASONING_EFFORT: &str = "high";

/// The `DeepSeek` chat-completions adapter.
#[derive(Debug, Clone, Copy, Default)]
pub struct DeepSeekAdapter;

// ---------------------------------------------------------------------------
// request
// ---------------------------------------------------------------------------

/// The parts of a [`CanonicalModelRequest`] this dialect reads.
///
/// Split out so the body builder is a pure function of catalog data plus request
/// data. A [`QualifiedModel`] is a live handle into a loaded catalog revision and
/// cannot be minted outside `aex-model-catalog`; borrowing only what the dialect
/// needs keeps every build rule reachable from a test.
#[derive(Debug, Clone, Copy)]
struct RequestView<'a> {
    system: &'a [SystemBlock],
    messages: &'a [CanonicalMessage],
    tools: &'a [CanonicalToolDef],
    tool_choice: &'a ToolChoice,
    parallel_tools: bool,
    max_output_tokens: u32,
    temperature_milli: Option<u16>,
    top_p_milli: Option<u16>,
    stop_sequences: &'a [BoundedString<64>],
    reasoning: ReasoningRequest,
    structured_output: Option<&'a StructuredOutputRequest>,
}

/// Candidate request view used only by the protected conformance runner.
///
/// Production generation continues to require a [`QualifiedModel`]. This
/// separate seam accepts one explicit [`EntryState::Staged`] entry so the
/// probes needed to earn its receipt do not depend on already having that
/// receipt. It contains no credential, origin, provider default or free-form
/// endpoint and reuses the same private request builder as production.
#[derive(Debug, Clone, Copy)]
pub struct QualificationRequest<'a> {
    /// System instruction blocks.
    pub system: &'a [SystemBlock],
    /// Canonical conversation history.
    pub messages: &'a [CanonicalMessage],
    /// Tool declarations.
    pub tools: &'a [CanonicalToolDef],
    /// Requested tool selection mode.
    pub tool_choice: &'a ToolChoice,
    /// Whether parallel tool calls are permitted.
    pub parallel_tools: bool,
    /// Requested output ceiling.
    pub max_output_tokens: u32,
    /// Temperature in integer milli-units.
    pub temperature_milli: Option<u16>,
    /// Nucleus sampling in integer milli-units.
    pub top_p_milli: Option<u16>,
    /// Caller stop sequences.
    pub stop_sequences: &'a [BoundedString<64>],
    /// Reasoning request.
    pub reasoning: ReasoningRequest,
    /// Structured output request.
    pub structured_output: Option<&'a StructuredOutputRequest>,
}

impl<'a> From<QualificationRequest<'a>> for RequestView<'a> {
    fn from(request: QualificationRequest<'a>) -> Self {
        Self {
            system: request.system,
            messages: request.messages,
            tools: request.tools,
            tool_choice: request.tool_choice,
            parallel_tools: request.parallel_tools,
            max_output_tokens: request.max_output_tokens,
            temperature_milli: request.temperature_milli,
            top_p_milli: request.top_p_milli,
            stop_sequences: request.stop_sequences,
            reasoning: request.reasoning,
            structured_output: request.structured_output,
        }
    }
}

/// Builds the exact production `DeepSeek` wire request for a staged candidate.
///
/// # Errors
///
/// Rejects any non-staged entry, provider/dialect/endpoint mismatch, missing
/// capability or invalid request bound before a credential or socket exists.
pub fn build_qualification_request(
    entry: &ModelEntry,
    request: QualificationRequest<'_>,
) -> Result<WireRequest, RequestBuildError> {
    if entry.state != EntryState::Staged {
        return Err(RequestBuildError::Encoding {
            reason: "qualification accepts only a staged catalog entry",
        });
    }
    if entry.provider != ProviderId::Deepseek
        || entry.dialect != Dialect::DeepSeekChat
        || entry.endpoint != EndpointPin::DeepSeekApi
    {
        return Err(RequestBuildError::Encoding {
            reason: "the staged entry does not pin the DeepSeek production dialect",
        });
    }
    build(entry, request.into())
}

/// Builds the provider's documented non-streamed shape for protected parity
/// qualification of one staged candidate.
///
/// Production generation remains streaming-only. This seam starts with the
/// exact production request builder, changes only the documented `stream`
/// controls, and retains the same pinned endpoint, path, auth tag and body
/// bound.
///
/// # Errors
///
/// As [`build_qualification_request`], plus an encoding failure if the closed
/// request body cannot be projected onto the documented non-streamed form.
pub fn build_non_streamed_qualification_request(
    entry: &ModelEntry,
    request: QualificationRequest<'_>,
) -> Result<WireRequest, RequestBuildError> {
    let mut wire = build_qualification_request(entry, request)?;
    let mut body: Value =
        serde_json::from_slice(&wire.body).map_err(|_| RequestBuildError::Encoding {
            reason: "the production DeepSeek request body is not JSON",
        })?;
    let object = body.as_object_mut().ok_or(RequestBuildError::Encoding {
        reason: "the production DeepSeek request body is not an object",
    })?;
    object.insert("stream".to_owned(), Value::Bool(false));
    object.remove("stream_options");
    let bytes = serde_json::to_vec(&body).map_err(|_| RequestBuildError::Encoding {
        reason: "the non-streamed qualification body is not serializable JSON",
    })?;
    if u64::try_from(bytes.len()).unwrap_or(u64::MAX)
        > u64::from(entry.limits.request_body_max_bytes)
    {
        return Err(RequestBuildError::BodyTooLarge {
            limit: entry.limits.request_body_max_bytes,
        });
    }
    wire.body = Bytes::from(bytes);
    wire.accept = Accept::Json;
    Ok(wire)
}

/// Decodes the documented non-streamed response through the production
/// `DeepSeek` state machine for protected stream-parity qualification.
///
/// The response is projected onto the dialect's equivalent single chunk and
/// terminal sentinel. Content, reasoning, tool calls, finish tokens and usage
/// are then validated and sealed by the same decoder used in production.
///
/// # Errors
///
/// Rejects a truncated, malformed, unknown or unsealable response shape.
pub fn decode_non_streamed_qualification_response(
    body: &BoundedBody,
    budget: &StreamBudget,
) -> Result<SealedResponse, FrameDecodeError> {
    if body.is_truncated() {
        return Err(FrameDecodeError::Budget(BudgetOverrun::Response {
            limit: budget.max_response_bytes,
        }));
    }
    let value = body.as_json().ok_or(FrameDecodeError::NotJson)?;
    let object = value
        .as_object()
        .ok_or(FrameDecodeError::MalformedField { field: "response" })?;
    if object.get("object").and_then(Value::as_str) != Some("chat.completion") {
        return Err(FrameDecodeError::UnknownEvent {
            event: BoundedString::truncating(
                object
                    .get("object")
                    .and_then(Value::as_str)
                    .unwrap_or("non_stream_response"),
            ),
        });
    }
    let choices = object
        .get("choices")
        .and_then(Value::as_array)
        .ok_or(FrameDecodeError::MalformedField { field: "choices" })?;
    let choice = choices
        .first()
        .and_then(Value::as_object)
        .ok_or(FrameDecodeError::MalformedField { field: "choices.0" })?;
    let message = choice.get("message").and_then(Value::as_object).ok_or(
        FrameDecodeError::MalformedField {
            field: "choices.0.message",
        },
    )?;
    let mut delta = Map::new();
    for key in ["role", "content", "reasoning_content", "tool_calls"] {
        if let Some(member) = message.get(key) {
            delta.insert(key.to_owned(), member.clone());
        }
    }
    let finish_reason =
        choice
            .get("finish_reason")
            .cloned()
            .ok_or(FrameDecodeError::MalformedField {
                field: "choices.0.finish_reason",
            })?;
    let chunk = json!({
        "object": CHUNK_OBJECT,
        "choices": [{
            "index": choice.get("index").cloned().unwrap_or(Value::from(0)),
            "delta": Value::Object(delta),
            "finish_reason": finish_reason,
        }],
        "usage": object.get("usage").cloned().unwrap_or(Value::Null),
    });
    let encoded = serde_json::to_vec(&chunk)
        .map_err(|_| FrameDecodeError::MalformedField { field: "response" })?;
    let mut state = DialectState::new();
    match DeepSeekAdapter.decode(
        &mut state,
        &SseEvent {
            name: None,
            data: &encoded,
            id: None,
        },
        budget,
    )? {
        FrameOutcome::Failed(_) => {
            return Err(FrameDecodeError::MalformedField {
                field: "finish_reason",
            });
        }
        FrameOutcome::Ignored
        | FrameOutcome::ResponseStarted
        | FrameOutcome::Progress
        | FrameOutcome::Terminal => {}
    }
    DeepSeekAdapter.decode(
        &mut state,
        &SseEvent {
            name: None,
            data: b"[DONE]",
            id: None,
        },
        budget,
    )?;
    DeepSeekAdapter.finish(state)
}

impl<'a> RequestView<'a> {
    fn of(request: &'a CanonicalModelRequest) -> Self {
        Self {
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
        }
    }

    /// Whether reasoning will be on for this call.
    ///
    /// `ProviderDefault` counts as **on**: `DeepSeek` reasons by default, so a
    /// request that sends nothing still lands in thinking mode, and the sampling
    /// prohibition still applies. Treating it as off would let `temperature`
    /// through to a surface that rejects it.
    const fn reasoning_is_on(self) -> bool {
        match self.reasoning {
            ReasoningRequest::Disabled => false,
            ReasoningRequest::ProviderDefault | ReasoningRequest::Enabled { .. } => true,
        }
    }
}

/// The neutral effort ladder onto `DeepSeek`'s own set.
///
/// The ends collapse by design: `Minimal` is "the least the provider offers
/// above off" and `Max` is "the most it offers", which for this provider are
/// `low` and `high`.
const fn effort_token(effort: ReasoningEffort) -> &'static str {
    match effort {
        ReasoningEffort::Minimal | ReasoningEffort::Low => "low",
        ReasoningEffort::Medium => "medium",
        ReasoningEffort::High | ReasoningEffort::Max => DEFAULT_REASONING_EFFORT,
    }
}

/// Renders integer milli-units as an exact JSON decimal.
///
/// The workspace carries sampling in milli-units precisely so no float
/// arithmetic happens; the decimal is assembled textually and parsed, so
/// `700` is `0.7` on the wire rather than `0.7000000000000001`.
fn milli_number(milli: u16) -> Result<Value, RequestBuildError> {
    let whole = milli / 1_000;
    let fraction = milli % 1_000;
    let text = if fraction == 0 {
        whole.to_string()
    } else {
        let mut digits = format!("{fraction:03}");
        while digits.ends_with('0') {
            digits.pop();
        }
        format!("{whole}.{digits}")
    };
    serde_json::from_str::<Value>(&text).map_err(|_| RequestBuildError::Encoding {
        reason: "a milli-unit sampling value is not representable as JSON",
    })
}

fn require(entry: &ModelEntry, capability: Capability) -> Result<(), RequestBuildError> {
    if entry.capabilities.has(capability) {
        return Ok(());
    }
    Err(RequestBuildError::CapabilityUnavailable { capability })
}

fn check_output_tokens(entry: &ModelEntry, view: RequestView<'_>) -> Result<(), RequestBuildError> {
    let limits = &entry.limits;
    if view.max_output_tokens < limits.min_output_tokens
        || view.max_output_tokens > limits.max_output_tokens
    {
        return Err(RequestBuildError::OutputTokensOutOfRange {
            min: limits.min_output_tokens,
            max: limits.max_output_tokens,
        });
    }
    Ok(())
}

fn check_reasoning(entry: &ModelEntry, view: RequestView<'_>) -> Result<(), RequestBuildError> {
    let ReasoningRequest::Enabled { budget_tokens, .. } = view.reasoning else {
        return Ok(());
    };
    require(entry, Capability::Reasoning)?;
    let Some(budget) = budget_tokens else {
        return Ok(());
    };
    // `DeepSeek` takes an effort, never a token count. An entry that declares no
    // budget range therefore has an empty one, and any budget is outside it.
    let (min, max) = match (
        entry.limits.min_reasoning_tokens,
        entry.limits.max_reasoning_tokens,
    ) {
        (Some(min), Some(max)) => (min, max),
        _ => (0, 0),
    };
    if budget < min || budget > max {
        return Err(RequestBuildError::ReasoningBudgetOutOfRange { min, max });
    }
    Ok(())
}

fn check_sampling(entry: &ModelEntry, view: RequestView<'_>) -> Result<(), RequestBuildError> {
    let reasoning_on = view.reasoning_is_on() && entry.reasoning.excludes_sampling;
    if view.temperature_milli.is_some() && reasoning_on {
        return Err(RequestBuildError::SamplingWithReasoning {
            field: "temperature",
        });
    }
    if view.top_p_milli.is_some() && reasoning_on {
        return Err(RequestBuildError::SamplingWithReasoning { field: "top_p" });
    }
    if let Some(temperature) = view.temperature_milli {
        if matches!(entry.sampling, SamplingSupport::None)
            || !entry.capabilities.has(Capability::Temperature)
        {
            return Err(RequestBuildError::SamplingUnsupported {
                field: "temperature",
            });
        }
        let (min, max) = entry.limits.temperature_milli.unwrap_or((0, 0));
        if temperature < min || temperature > max || temperature > MAX_TEMPERATURE_MILLI {
            return Err(RequestBuildError::SamplingUnsupported {
                field: "temperature",
            });
        }
    }
    if let Some(top_p) = view.top_p_milli {
        if !matches!(entry.sampling, SamplingSupport::Full)
            || !entry.capabilities.has(Capability::TopP)
        {
            return Err(RequestBuildError::SamplingUnsupported { field: "top_p" });
        }
        let (min, max) = entry.limits.top_p_milli.unwrap_or((0, 0));
        if top_p < min || top_p > max {
            return Err(RequestBuildError::SamplingUnsupported { field: "top_p" });
        }
    }
    Ok(())
}

fn check_stop_sequences(
    entry: &ModelEntry,
    view: RequestView<'_>,
) -> Result<Option<Value>, RequestBuildError> {
    if view.stop_sequences.is_empty() {
        return Ok(None);
    }
    require(entry, Capability::StopSequences)?;
    let max = entry.limits.max_stop_sequences.min(MAX_STOP_SEQUENCES);
    if view.stop_sequences.len() > usize::from(max) {
        return Err(RequestBuildError::StopSequenceLimit { max });
    }
    Ok(Some(Value::Array(
        view.stop_sequences
            .iter()
            .map(|stop| Value::String(stop.as_str().to_owned()))
            .collect(),
    )))
}

fn check_tools(entry: &ModelEntry, view: RequestView<'_>) -> Result<(), RequestBuildError> {
    if view.tools.is_empty() {
        return check_tool_choice_without_tools(view);
    }
    require(entry, Capability::Tools)?;
    let max = entry.limits.max_tools.min(MAX_TOOLS);
    if view.tools.len() > usize::from(max) {
        return Err(RequestBuildError::ToolLimit { max });
    }
    for tool in view.tools {
        let name = tool.name.as_str();
        if !entry.tool_policy.name_pattern.accepts(name)
            || name.len() > usize::from(entry.tool_policy.max_name_bytes)
        {
            return Err(RequestBuildError::ToolNameInvalid {
                name: tool.name.clone(),
            });
        }
        if tool.strict {
            require(entry, Capability::StrictToolSchema)?;
        }
    }
    if view.parallel_tools {
        require(entry, Capability::ParallelTools)?;
    } else {
        // The chat-completions surface has no `parallel_tool_calls` switch, so
        // "one tool at a time" cannot be expressed. Refused rather than dropped.
        return Err(RequestBuildError::SamplingUnsupported {
            field: "parallel_tool_calls",
        });
    }
    check_tool_choice(entry, view)
}

fn check_tool_choice_without_tools(view: RequestView<'_>) -> Result<(), RequestBuildError> {
    match view.tool_choice {
        ToolChoice::Auto | ToolChoice::None => Ok(()),
        requested @ (ToolChoice::Required | ToolChoice::Named { .. }) => {
            Err(RequestBuildError::ToolChoiceUnsupported {
                requested: Box::new(requested.clone()),
            })
        }
    }
}

fn check_tool_choice(entry: &ModelEntry, view: RequestView<'_>) -> Result<(), RequestBuildError> {
    match view.tool_choice {
        ToolChoice::Auto => Ok(()),
        ToolChoice::None => require(entry, Capability::ToolChoiceNone),
        ToolChoice::Required => require(entry, Capability::ToolChoiceRequired),
        ToolChoice::Named { name } => {
            require(entry, Capability::ToolChoiceNamed)?;
            if view.tools.iter().any(|tool| tool.name == *name) {
                return Ok(());
            }
            Err(RequestBuildError::ToolChoiceUnsupported {
                requested: Box::new(view.tool_choice.clone()),
            })
        }
    }
}

/// Whether the word `json` appears anywhere in the canonical system or user
/// text, which `DeepSeek` documents as a requirement of `json_object` mode.
fn mentions_json(view: RequestView<'_>) -> bool {
    let in_system = view
        .system
        .iter()
        .any(|block| block.text.as_str().to_ascii_lowercase().contains("json"));
    if in_system {
        return true;
    }
    view.messages
        .iter()
        .filter(|message| matches!(message.role, Role::User))
        .flat_map(|message| message.blocks.iter())
        .any(|block| match block {
            CanonicalBlock::Text { text, .. } => {
                text.as_str().to_ascii_lowercase().contains("json")
            }
            _ => false,
        })
}

fn check_structured_output(
    entry: &ModelEntry,
    view: RequestView<'_>,
) -> Result<Option<Value>, RequestBuildError> {
    let Some(requested) = view.structured_output else {
        return Ok(None);
    };
    if matches!(requested, StructuredOutputRequest::JsonSchema { .. })
        || matches!(entry.structured_output, StructuredOutputPolicy::Unsupported)
    {
        return Err(RequestBuildError::StructuredOutputUnsupported {
            requested: Box::new(requested.clone()),
        });
    }
    require(entry, Capability::StructuredOutput)?;
    if !mentions_json(view) {
        return Err(RequestBuildError::StructuredOutputPromptRequirement);
    }
    Ok(Some(json!({ "type": "json_object" })))
}

fn encode_tools(tools: &[CanonicalToolDef]) -> Vec<Value> {
    tools
        .iter()
        .map(|tool| {
            json!({
                "type": "function",
                "function": {
                    "name": tool.name.as_str(),
                    "description": tool.description.as_str(),
                    "parameters": tool.input_schema.to_value(),
                    "strict": tool.strict,
                },
            })
        })
        .collect()
}

fn encode_tool_choice(choice: &ToolChoice) -> Value {
    match choice {
        ToolChoice::Auto => Value::String("auto".to_owned()),
        ToolChoice::None => Value::String("none".to_owned()),
        ToolChoice::Required => Value::String("required".to_owned()),
        ToolChoice::Named { name } => json!({
            "type": "function",
            "function": { "name": name.as_str() },
        }),
    }
}

/// The reasoning text a turn must echo back, honouring provenance.
fn replayed_reasoning(
    entry: &ModelEntry,
    reasoning: &ReasoningBlock,
) -> Result<String, RequestBuildError> {
    if let Some(token) = &reasoning.token {
        if token.provenance != entry.provider {
            return Err(RequestBuildError::ReasoningProvenanceMismatch {
                expected: entry.provider,
                found: token.provenance,
            });
        }
        return String::from_utf8(token.bytes.to_vec()).map_err(|_| RequestBuildError::Encoding {
            reason: "reasoning round-trip material is not UTF-8",
        });
    }
    match &reasoning.body {
        ReasoningBody::Text { text } | ReasoningBody::Summary { text } => {
            Ok(text.as_str().to_owned())
        }
        ReasoningBody::Redacted => Err(RequestBuildError::ReasoningTokenRequired),
    }
}

fn encode_tool_result(call: &ToolCallId, content: &[ToolResultPart]) -> Value {
    let joined = content
        .iter()
        .map(|part| match part {
            ToolResultPart::Text { text } => text.as_str().to_owned(),
            ToolResultPart::Json { value } => value.as_str().to_owned(),
        })
        .collect::<Vec<_>>()
        .join("\n");
    json!({
        "role": "tool",
        "tool_call_id": call.as_str(),
        "content": joined,
    })
}

fn encode_user_turn(
    message: &CanonicalMessage,
    out: &mut Vec<Value>,
) -> Result<(), RequestBuildError> {
    let mut text = String::new();
    for block in &message.blocks {
        match block {
            CanonicalBlock::ToolResult { call, content, .. } => {
                out.push(encode_tool_result(call, content));
            }
            CanonicalBlock::Text { text: fragment, .. } => {
                if !text.is_empty() {
                    text.push('\n');
                }
                text.push_str(fragment.as_str());
            }
            CanonicalBlock::ToolUse { .. }
            | CanonicalBlock::Reasoning(_)
            | CanonicalBlock::Refusal { .. } => {
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

fn encode_assistant_turn(
    entry: &ModelEntry,
    message: &CanonicalMessage,
    out: &mut Vec<Value>,
) -> Result<(), RequestBuildError> {
    let mut text = String::new();
    let mut reasoning: Option<String> = None;
    let mut calls: Vec<Value> = Vec::new();
    for block in &message.blocks {
        match block {
            CanonicalBlock::Text { text: fragment, .. } => {
                if !text.is_empty() {
                    text.push('\n');
                }
                text.push_str(fragment.as_str());
            }
            CanonicalBlock::Refusal { text: fragment } => {
                if !text.is_empty() {
                    text.push('\n');
                }
                text.push_str(fragment.as_str());
            }
            CanonicalBlock::Reasoning(block) => {
                reasoning = Some(replayed_reasoning(entry, block)?);
            }
            CanonicalBlock::ToolUse { id, name, input } => {
                calls.push(json!({
                    "id": id.as_str(),
                    "type": "function",
                    "function": { "name": name.as_str(), "arguments": input.as_str() },
                }));
            }
            CanonicalBlock::ToolResult { .. } => {
                return Err(RequestBuildError::Encoding {
                    reason: "a tool result belongs on a user turn, not an assistant turn",
                });
            }
        }
    }
    if !calls.is_empty()
        && reasoning.is_none()
        && matches!(
            entry.reasoning.replay,
            ReasoningReplay::RequiredWithToolCalls
        )
    {
        return Err(RequestBuildError::ReasoningTokenRequired);
    }
    let mut turn = Map::new();
    turn.insert("role".to_owned(), Value::String("assistant".to_owned()));
    if !text.is_empty() || calls.is_empty() {
        turn.insert("content".to_owned(), Value::String(text));
    }
    if let Some(reasoning) = reasoning {
        turn.insert("reasoning_content".to_owned(), Value::String(reasoning));
    }
    if !calls.is_empty() {
        turn.insert("tool_calls".to_owned(), Value::Array(calls));
    }
    out.push(Value::Object(turn));
    Ok(())
}

fn encode_messages(
    entry: &ModelEntry,
    view: RequestView<'_>,
) -> Result<Vec<Value>, RequestBuildError> {
    let mut out: Vec<Value> = Vec::new();
    if !view.system.is_empty() {
        require(entry, Capability::SystemInstruction)?;
        let joined = view
            .system
            .iter()
            .map(|block| block.text.as_str())
            .collect::<Vec<_>>()
            .join("\n\n");
        out.push(json!({ "role": "system", "content": joined }));
    }
    for message in view.messages {
        match message.role {
            Role::User => encode_user_turn(message, &mut out)?,
            Role::Assistant => encode_assistant_turn(entry, message, &mut out)?,
        }
    }
    Ok(out)
}

fn build_body(entry: &ModelEntry, view: RequestView<'_>) -> Result<Value, RequestBuildError> {
    if entry.provider != ProviderId::Deepseek {
        return Err(RequestBuildError::Encoding {
            reason: "this adapter speaks only for deepseek",
        });
    }
    require(entry, Capability::Streaming)?;
    check_output_tokens(entry, view)?;
    check_reasoning(entry, view)?;
    check_sampling(entry, view)?;
    let stop = check_stop_sequences(entry, view)?;
    check_tools(entry, view)?;
    let response_format = check_structured_output(entry, view)?;
    let messages = encode_messages(entry, view)?;

    let mut body = Map::new();
    body.insert(
        "model".to_owned(),
        Value::String(entry.model.as_str().to_owned()),
    );
    body.insert("messages".to_owned(), Value::Array(messages));
    body.insert("max_tokens".to_owned(), Value::from(view.max_output_tokens));
    body.insert("stream".to_owned(), Value::Bool(true));
    body.insert(
        "stream_options".to_owned(),
        json!({ "include_usage": true }),
    );
    if let Some(stop) = stop {
        body.insert("stop".to_owned(), stop);
    }
    if let Some(format) = response_format {
        body.insert("response_format".to_owned(), format);
    }
    if !view.tools.is_empty() {
        body.insert("tools".to_owned(), Value::Array(encode_tools(view.tools)));
        body.insert(
            "tool_choice".to_owned(),
            encode_tool_choice(view.tool_choice),
        );
    }
    if let Some(temperature) = view.temperature_milli {
        body.insert("temperature".to_owned(), milli_number(temperature)?);
    }
    if let Some(top_p) = view.top_p_milli {
        body.insert("top_p".to_owned(), milli_number(top_p)?);
    }
    insert_thinking(&mut body, view.reasoning);
    Ok(Value::Object(body))
}

/// Writes the thinking controls.
///
/// D-24: when reasoning is explicitly enabled AEX sends `reasoning_effort` in
/// **both** documented positions, because the reference nests it inside
/// `thinking` and the guide shows it at the top level. Probe R-2 records which
/// the server honours; until it does, sending one and guessing would be the
/// silent failure.
fn insert_thinking(body: &mut Map<String, Value>, reasoning: ReasoningRequest) {
    match reasoning {
        ReasoningRequest::ProviderDefault => {}
        ReasoningRequest::Disabled => {
            body.insert("thinking".to_owned(), json!({ "type": "disabled" }));
        }
        ReasoningRequest::Enabled { effort, .. } => {
            let token = effort.map_or(DEFAULT_REASONING_EFFORT, effort_token);
            body.insert(
                "thinking".to_owned(),
                json!({ "type": "enabled", "reasoning_effort": token }),
            );
            body.insert(
                "reasoning_effort".to_owned(),
                Value::String(token.to_owned()),
            );
        }
    }
}

fn build(entry: &ModelEntry, view: RequestView<'_>) -> Result<WireRequest, RequestBuildError> {
    let body = build_body(entry, view)?;
    let bytes = serde_json::to_vec(&body).map_err(|_| RequestBuildError::Encoding {
        reason: "the assembled body is not serializable JSON",
    })?;
    let limit = entry.limits.request_body_max_bytes;
    if u64::try_from(bytes.len()).unwrap_or(u64::MAX) > u64::from(limit) {
        return Err(RequestBuildError::BodyTooLarge { limit });
    }
    let bound = |text: &str| -> Result<BoundedString<256>, RequestBuildError> {
        BoundedString::new(text).map_err(|_| RequestBuildError::Encoding {
            reason: "a compiled request member exceeds its bound",
        })
    };
    let header =
        BoundedString::new("application/json").map_err(|_| RequestBuildError::Encoding {
            reason: "a compiled request member exceeds its bound",
        })?;
    Ok(WireRequest {
        endpoint: EndpointPin::DeepSeekApi,
        path: bound(CHAT_COMPLETIONS_PATH)?,
        query: Vec::new(),
        headers: vec![("content-type", header)],
        auth: AuthScheme::BearerAuthorization,
        body: Bytes::from(bytes),
        accept: Accept::TextEventStream,
    })
}

// ---------------------------------------------------------------------------
// stop and error tables
// ---------------------------------------------------------------------------

/// What a `DeepSeek` `finish_reason` means. The two failure tokens are failures,
/// never stop reasons (D-09).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FinishToken {
    Stop(StopReason),
    Failure(ProviderFailureKind),
}

/// The compiled `finish_reason` table.
const fn finish_token(token: &str) -> Option<FinishToken> {
    match token.as_bytes() {
        b"stop" => Some(FinishToken::Stop(StopReason::EndTurn)),
        b"tool_calls" => Some(FinishToken::Stop(StopReason::ToolUse)),
        b"length" => Some(FinishToken::Stop(StopReason::MaxOutputTokens)),
        b"content_filter" => Some(FinishToken::Failure(ProviderFailureKind::ContentFiltered)),
        b"insufficient_system_resource" => Some(FinishToken::Failure(
            ProviderFailureKind::InsufficientProviderResource,
        )),
        _ => None,
    }
}

/// The compiled status table. `DeepSeek` documents these seven and no body
/// shape, so the status alone must be enough.
const fn status_kind(status: u16) -> ProviderFailureKind {
    match status {
        400 | 422 => ProviderFailureKind::InvalidRequest,
        401 => ProviderFailureKind::Authentication,
        402 => ProviderFailureKind::Billing,
        404 => ProviderFailureKind::ModelNotFound,
        429 => ProviderFailureKind::RateLimited,
        503 => ProviderFailureKind::Overloaded,
        _ => ProviderFailureKind::ServerError,
    }
}

/// Recognizes the one bounded `DeepSeek` diagnostic that proves a numeric
/// context-window overflow. A generic 400, a code token, or a substring match
/// is deliberately insufficient because all other malformed requests share
/// the same status and error type.
fn is_exact_context_overflow_message(message: &str) -> bool {
    const PREFIX: &str = "This model's maximum context length is ";
    const MAX_SUFFIX: &str = " tokens. However, you requested ";
    const REQUESTED_SUFFIX: &str = " tokens (";
    const MESSAGE_SUFFIX: &str = " in the messages, ";
    const COMPLETION_SUFFIX: &str =
        " in the completion). Please reduce the length of the messages or completion.";

    if message.len() > 256 {
        return false;
    }
    let Some(rest) = message.strip_prefix(PREFIX) else {
        return false;
    };
    let Some((maximum, rest)) = take_bounded_decimal(rest, MAX_SUFFIX) else {
        return false;
    };
    let Some((requested, rest)) = take_bounded_decimal(rest, REQUESTED_SUFFIX) else {
        return false;
    };
    let Some((message_tokens, rest)) = take_bounded_decimal(rest, MESSAGE_SUFFIX) else {
        return false;
    };
    let Some((completion_tokens, rest)) = take_bounded_decimal(rest, COMPLETION_SUFFIX) else {
        return false;
    };
    rest.is_empty()
        && maximum > 0
        && requested > maximum
        && message_tokens
            .checked_add(completion_tokens)
            .is_some_and(|total| total == requested)
}

fn take_bounded_decimal<'a>(input: &'a str, suffix: &str) -> Option<(u64, &'a str)> {
    let split = input.find(suffix)?;
    let digits = &input[..split];
    if digits.is_empty()
        || digits.len() > 10
        || !digits.bytes().all(|byte| byte.is_ascii_digit())
        || (digits.len() > 1 && digits.starts_with('0'))
    {
        return None;
    }
    let value = digits.parse().ok()?;
    Some((value, &input[split + suffix.len()..]))
}

// ---------------------------------------------------------------------------
// decoding
// ---------------------------------------------------------------------------

fn read_u64(map: &Map<String, Value>, key: &str) -> Option<u64> {
    map.get(key).and_then(Value::as_u64)
}

/// Folds one `usage` object into the tally.
///
/// Only called for an actual object: every non-terminal chunk carries
/// `usage: null`, and merging a null would zero a tally that the trailing chunk
/// already filled.
fn merge_usage(target: &mut NormalizedUsage, usage: &Map<String, Value>) {
    let mut missing = UsageFieldSet::EMPTY;
    let hit = read_u64(usage, "prompt_cache_hit_tokens");
    let miss = read_u64(usage, "prompt_cache_miss_tokens");
    match (miss, read_u64(usage, "prompt_tokens")) {
        (Some(miss), _) => target.input_tokens = miss,
        (None, Some(prompt)) => target.input_tokens = prompt,
        (None, None) => missing = missing.with(UsageField::InputTokens),
    }
    match hit {
        Some(hit) => target.cache_read_input_tokens = hit,
        None => missing = missing.with(UsageField::CacheReadInputTokens),
    }
    match read_u64(usage, "completion_tokens") {
        Some(completion) => target.output_tokens = completion,
        None => missing = missing.with(UsageField::OutputTokens),
    }
    match usage
        .get("completion_tokens_details")
        .and_then(Value::as_object)
        .and_then(|details| read_u64(details, "reasoning_tokens"))
    {
        Some(reasoning) => target.reasoning_tokens = reasoning,
        None => missing = missing.with(UsageField::ReasoningTokens),
    }
    target.provider_total_tokens = read_u64(usage, "total_tokens");
    if target.provider_total_tokens.is_none() {
        missing = missing.with(UsageField::ProviderTotalTokens);
    }
    target.completeness = if missing.is_empty() {
        UsageCompleteness::Exact
    } else {
        UsageCompleteness::Partial { missing }
    };
}

/// Folds one `delta.tool_calls[]` fragment into the open call at its index.
fn merge_tool_fragment(
    state: &mut DialectState,
    budget: &StreamBudget,
    fragment: &Value,
) -> Result<(), FrameDecodeError> {
    let Some(fragment) = fragment.as_object() else {
        return Err(FrameDecodeError::MalformedField {
            field: "delta.tool_calls",
        });
    };
    let index = read_u64(fragment, "index").unwrap_or(0);
    let index = u16::try_from(index).map_err(|_| FrameDecodeError::MalformedField {
        field: "delta.tool_calls.index",
    })?;
    let is_new = !state.open_tools.contains_key(&index);
    if is_new {
        state.ledger.open_tool_call(budget)?;
        state.ledger.open_block(budget)?;
        state.open_tools.insert(index, PartialToolCall::default());
    }
    let Some(call) = state.open_tools.get_mut(&index) else {
        return Err(FrameDecodeError::MalformedField {
            field: "delta.tool_calls.index",
        });
    };
    if let Some(id) = fragment.get("id").and_then(Value::as_str)
        && !id.is_empty()
    {
        call.id = Some(ToolCallId::truncating(id));
    }
    if let Some(function) = fragment.get("function").and_then(Value::as_object) {
        if let Some(name) = function.get("name").and_then(Value::as_str)
            && !name.is_empty()
        {
            call.name = Some(name.to_owned());
        }
        if let Some(arguments) = function.get("arguments").and_then(Value::as_str) {
            call.arguments.push_str(arguments);
        }
    }
    let id = call
        .id
        .clone()
        .unwrap_or_else(|| ToolCallId::truncating("<unassigned>"));
    let accumulated = call.arguments.len();
    BudgetLedger::check_tool_arguments(budget, &id, accumulated)?;
    Ok(())
}

/// The delta keys this dialect sends. Anything else is a shape the adapter was
/// not written against, which §6.4 classifies as a protocol violation.
fn check_delta_keys(delta: &Map<String, Value>) -> Result<(), FrameDecodeError> {
    for key in delta.keys() {
        if !matches!(
            key.as_str(),
            "role" | "content" | "reasoning_content" | "tool_calls"
        ) {
            return Err(FrameDecodeError::MalformedField { field: "delta" });
        }
    }
    Ok(())
}

fn absorb_delta(
    state: &mut DialectState,
    budget: &StreamBudget,
    delta: &Map<String, Value>,
) -> Result<(), FrameDecodeError> {
    check_delta_keys(delta)?;
    if let Some(content) = delta.get("content").and_then(Value::as_str)
        && !content.is_empty()
    {
        state
            .ledger
            .charge_text(budget, u64::try_from(content.len()).unwrap_or(u64::MAX))?;
        if state.open_text.is_empty() {
            state.ledger.open_block(budget)?;
        }
        state.open_text.entry(0).or_default().push_str(content);
    }
    if let Some(reasoning) = delta.get("reasoning_content").and_then(Value::as_str)
        && !reasoning.is_empty()
    {
        state
            .ledger
            .charge_reasoning(budget, u64::try_from(reasoning.len()).unwrap_or(u64::MAX))?;
        if state.open_reasoning.is_empty() {
            state.ledger.open_block(budget)?;
        }
        state
            .open_reasoning
            .entry(0)
            .or_default()
            .push_str(reasoning);
    }
    if let Some(calls) = delta.get("tool_calls") {
        let Some(calls) = calls.as_array() else {
            return Err(FrameDecodeError::MalformedField {
                field: "delta.tool_calls",
            });
        };
        for fragment in calls {
            merge_tool_fragment(state, budget, fragment)?;
        }
    }
    Ok(())
}

fn mid_stream_failure(kind: ProviderFailureKind, token: &str) -> FrameOutcome {
    let detail = RedactedDetail::new(kind, redact::<512>(token, &[]));
    FrameOutcome::Failed(Box::new(ProviderFailure::new(detail)))
}

fn decode_choice(
    state: &mut DialectState,
    budget: &StreamBudget,
    choice: &Value,
) -> Result<FrameOutcome, FrameDecodeError> {
    let Some(choice) = choice.as_object() else {
        return Err(FrameDecodeError::MalformedField { field: "choices.0" });
    };
    let outcome = state.mark_started();
    if let Some(delta) = choice.get("delta") {
        let Some(delta) = delta.as_object() else {
            return Err(FrameDecodeError::MalformedField { field: "delta" });
        };
        absorb_delta(state, budget, delta)?;
    }
    let Some(token) = choice.get("finish_reason").and_then(Value::as_str) else {
        return Ok(outcome);
    };
    let Some(resolved) = finish_token(token) else {
        return Err(FrameDecodeError::MalformedField {
            field: "finish_reason",
        });
    };
    match resolved {
        FinishToken::Stop(_) => {
            state.finish_token = Some(token.to_owned());
            Ok(outcome)
        }
        FinishToken::Failure(kind) => Ok(mid_stream_failure(kind, token)),
    }
}

fn decode_chunk(
    state: &mut DialectState,
    budget: &StreamBudget,
    chunk: &Map<String, Value>,
) -> Result<FrameOutcome, FrameDecodeError> {
    if let Some(object) = chunk.get("object").and_then(Value::as_str)
        && object != CHUNK_OBJECT
    {
        return Err(FrameDecodeError::UnknownEvent {
            event: BoundedString::truncating(object),
        });
    }
    if let Some(kind) = chunk.get("type").and_then(Value::as_str) {
        return Err(FrameDecodeError::UnknownEvent {
            event: BoundedString::truncating(kind),
        });
    }
    if let Some(error) = chunk.get("error") {
        let message = error
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or("the provider reported an error mid-stream");
        return Ok(mid_stream_failure(
            ProviderFailureKind::ServerError,
            message,
        ));
    }
    let Some(choices) = chunk.get("choices").and_then(Value::as_array) else {
        return Err(FrameDecodeError::MalformedField { field: "choices" });
    };
    let usage = chunk.get("usage").and_then(Value::as_object);
    if let Some(usage) = usage {
        merge_usage(&mut state.usage, usage);
    }
    let Some(choice) = choices.first() else {
        // The trailing `include_usage` chunk. `choices: []` is never proof that
        // the provider is generating, so it must not start the response.
        return Ok(if usage.is_some() {
            FrameOutcome::Progress
        } else {
            FrameOutcome::Ignored
        });
    };
    decode_choice(state, budget, choice)
}

fn sealed_text(text: String) -> Result<BoundedString<TEXT_MAX>, FrameDecodeError> {
    BoundedString::new(text).map_err(|_| {
        FrameDecodeError::Budget(BudgetOverrun::Text {
            limit: u64::try_from(TEXT_MAX).unwrap_or(u64::MAX),
        })
    })
}

fn sealed_reasoning(text: &str) -> Result<CanonicalBlock, FrameDecodeError> {
    let body = BoundedString::<REASON_MAX>::new(text.to_owned()).map_err(|_| {
        FrameDecodeError::Budget(BudgetOverrun::Reasoning {
            limit: u64::try_from(REASON_MAX).unwrap_or(u64::MAX),
        })
    })?;
    Ok(CanonicalBlock::Reasoning(ReasoningBlock {
        body: ReasoningBody::Text { text: body },
        // `DeepSeek`'s round-trip material *is* the reasoning text: it must be
        // echoed on the next turn when tool calls were carried, so it is
        // captured as a token with this provider's provenance.
        token: Some(ReasoningToken {
            provenance: ProviderId::Deepseek,
            bytes: Bytes::from(text.as_bytes().to_vec()),
        }),
    }))
}

fn sealed_tool(call: &PartialToolCall) -> Result<CanonicalBlock, FrameDecodeError> {
    let Some(id) = call.id.clone() else {
        return Err(FrameDecodeError::MalformedField {
            field: "tool_calls.id",
        });
    };
    let Some(name) = call.name.as_deref() else {
        return Err(FrameDecodeError::MalformedField {
            field: "tool_calls.function.name",
        });
    };
    let name = ToolName::parse(name).map_err(|_| FrameDecodeError::MalformedField {
        field: "tool_calls.function.name",
    })?;
    let text = if call.arguments.trim().is_empty() {
        "{}"
    } else {
        call.arguments.as_str()
    };
    let input = CanonicalJson::parse(text)
        .map_err(|_| FrameDecodeError::ToolArgumentsNotJson { call: id.clone() })?;
    Ok(CanonicalBlock::ToolUse { id, name, input })
}

fn seal_blocks(state: &DialectState) -> Result<Vec<CanonicalBlock>, FrameDecodeError> {
    let mut blocks: Vec<CanonicalBlock> = Vec::new();
    if let Some(reasoning) = state.open_reasoning.get(&0)
        && !reasoning.is_empty()
    {
        blocks.push(sealed_reasoning(reasoning)?);
    }
    if let Some(text) = state.open_text.get(&0)
        && !text.is_empty()
    {
        blocks.push(CanonicalBlock::Text {
            text: sealed_text(text.clone())?,
            annotations: Vec::new(),
        });
    }
    // `BTreeMap` order is the provider's own `tool_calls[].index` order, which
    // is the order the calls were opened in.
    for call in state.open_tools.values() {
        blocks.push(sealed_tool(call)?);
    }
    Ok(blocks)
}

// ---------------------------------------------------------------------------
// the adapter
// ---------------------------------------------------------------------------

impl ProviderAdapter for DeepSeekAdapter {
    fn provider(&self) -> ProviderId {
        ProviderId::Deepseek
    }

    fn build_request(
        &self,
        model: &QualifiedModel,
        request: &CanonicalModelRequest,
    ) -> Result<WireRequest, RequestBuildError> {
        build(model.entry(), RequestView::of(request))
    }

    fn new_state(&self, _model: &QualifiedModel) -> DialectState {
        DialectState::new()
    }

    fn decode(
        &self,
        state: &mut DialectState,
        event: &SseEvent<'_>,
        budget: &StreamBudget,
    ) -> Result<FrameOutcome, FrameDecodeError> {
        if let Some(name) = event.name {
            // Data-only SSE: an `event:` name is a dialect this adapter was not
            // written against.
            return Err(FrameDecodeError::UnknownEvent {
                event: BoundedString::truncating(name),
            });
        }
        state
            .ledger
            .charge_response(budget, u64::try_from(event.data.len()).unwrap_or(u64::MAX))?;
        state.ledger.count_frame();
        if event.is_done_sentinel() {
            state.terminal = true;
            return Ok(FrameOutcome::Terminal);
        }
        if event.data.iter().all(u8::is_ascii_whitespace) {
            return Ok(FrameOutcome::Ignored);
        }
        let value: Value =
            serde_json::from_slice(event.data).map_err(|_| FrameDecodeError::NotJson)?;
        let Value::Object(chunk) = value else {
            return Err(FrameDecodeError::MalformedField { field: "chunk" });
        };
        decode_chunk(state, budget, &chunk)
    }

    fn finish(&self, state: DialectState) -> Result<SealedResponse, FrameDecodeError> {
        if !state.terminal {
            return Err(FrameDecodeError::OutOfOrder {
                reason: "the stream ended without the [DONE] sentinel",
            });
        }
        let Some(token) = state.finish_token.as_deref() else {
            return Err(FrameDecodeError::OutOfOrder {
                reason: "the stream ended without a finish_reason",
            });
        };
        let Some(FinishToken::Stop(stop_reason)) = finish_token(token) else {
            return Err(FrameDecodeError::MalformedField {
                field: "finish_reason",
            });
        };
        let blocks = seal_blocks(&state)?;
        let has_tool_use = blocks
            .iter()
            .any(|block| matches!(block, CanonicalBlock::ToolUse { .. }));
        if has_tool_use != matches!(stop_reason, StopReason::ToolUse) {
            return Err(FrameDecodeError::OutOfOrder {
                reason: "tool calls and the finish reason disagree",
            });
        }
        let mut usage = state.usage;
        if usage.provider_total_tokens.is_none() && usage.completeness == UsageCompleteness::Exact {
            usage.completeness = UsageCompleteness::Absent;
        }
        Ok(SealedResponse {
            blocks,
            stop_reason,
            usage,
            provider_request_id: None,
            gateway_route: None,
        })
    }

    fn classify_http(
        &self,
        status: u16,
        headers: &HeaderView<'_>,
        body: &BoundedBody,
    ) -> ProviderFailure {
        let parsed = body.as_json();
        let error = parsed.as_ref().and_then(|value| value.get("error"));
        let code = error
            .and_then(|error| error.get("code").or_else(|| error.get("type")))
            .and_then(Value::as_str);
        let message = error
            .and_then(|error| error.get("message"))
            .and_then(Value::as_str);
        let kind = if status == 400
            && !body.is_truncated()
            && message.is_some_and(is_exact_context_overflow_message)
        {
            ProviderFailureKind::ContextOverflow
        } else {
            status_kind(status)
        };
        let text = message.or_else(|| body.as_str()).unwrap_or_default();
        let mut detail = if text.trim().is_empty() {
            RedactedDetail::internal(kind, "the provider returned no diagnostic body")
        } else {
            RedactedDetail::new(kind, redact::<512>(text, &[]))
        }
        .with_status(status);
        if let Some(code) = code {
            let code = redact::<64>(code, &[]);
            detail = detail.with_code(code.as_str());
        }
        ProviderFailure {
            detail,
            rate_limit: self.rate_limit_feedback(headers),
        }
    }

    fn rate_limit_feedback(&self, _headers: &HeaderView<'_>) -> RateLimitFeedback {
        // `DeepSeek` documents no rate-limit headers at all. Concurrency is the
        // published control and lives on the entry as `concurrency_hint`, so the
        // honest answer is the positive record that nothing was published —
        // reading a stray `Retry-After` would invent backpressure semantics the
        // provider never promised.
        RateLimitFeedback::none()
    }

    fn request_id(
        &self,
        _headers: &HeaderView<'_>,
        _state: &DialectState,
    ) -> Option<ProviderRequestId> {
        // None documented, in a header or in the body. Correlation is the AEX
        // effect id only.
        None
    }
}

#[cfg(test)]
mod tests {
    use aex_model_catalog::document::{
        CacheReadSemantics, CapabilitySet, ReasoningEncoding, ReasoningMode, ReasoningPolicy,
        StreamUsageDelivery,
    };
    use aex_model_catalog::fixture;

    use super::{
        Accept, AuthScheme, BoundedBody, BoundedString, Bytes, CHAT_COMPLETIONS_PATH,
        CanonicalBlock, CanonicalJson, CanonicalMessage, CanonicalToolDef, Capability,
        DeepSeekAdapter, DialectState, EndpointPin, FinishToken, FrameDecodeError, FrameOutcome,
        HeaderView, ModelEntry, ProviderAdapter, ProviderFailureKind, ProviderId,
        QualificationRequest, ReasoningBlock, ReasoningBody, ReasoningEffort, ReasoningReplay,
        ReasoningRequest, ReasoningToken, RequestBuildError, RequestView, Role, SealedResponse,
        StopReason, StreamBudget, StructuredOutputRequest, SystemBlock, ToolChoice, ToolName,
        ToolResultPart, UsageCompleteness, Value, build, build_body,
        build_non_streamed_qualification_request, build_qualification_request,
        decode_non_streamed_qualification_response, finish_token, status_kind,
    };
    use crate::error::RateLimitSource;
    use crate::sse::SseDecoder;

    // -----------------------------------------------------------------------
    // harness
    // -----------------------------------------------------------------------

    fn capabilities() -> CapabilitySet {
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
            Capability::ReasoningReplay,
            Capability::PromptCacheImplicit,
            Capability::StopSequences,
            Capability::Temperature,
            Capability::TopP,
            Capability::SystemInstruction,
        ])
    }

    /// The catalog entry §5.3 describes, built from the shared fixture and then
    /// narrowed to the facts this dialect actually publishes.
    fn entry() -> ModelEntry {
        let mut entry = fixture::entry(ProviderId::Deepseek, "deepseek-v4-pro", capabilities());
        entry.reasoning = ReasoningPolicy {
            mode: ReasoningMode::Optional,
            encoding: ReasoningEncoding::DeepSeekThinking,
            replay: ReasoningReplay::RequiredWithToolCalls,
            excludes_sampling: true,
        };
        entry.usage_map.cache_read_field = CacheReadSemantics::HitMissSplit;
        entry.usage_map.stream_usage_delivery = StreamUsageDelivery::TrailingChoicesEmptyChunk;
        entry.limits.max_stop_sequences = 16;
        entry.limits.max_tools = 128;
        entry.limits.temperature_milli = Some((0, 2_000));
        entry.limits.min_reasoning_tokens = None;
        entry.limits.max_reasoning_tokens = None;
        entry.concurrency_hint = 500;
        entry
    }

    fn text_block(text: &str) -> CanonicalBlock {
        CanonicalBlock::Text {
            text: fixture::bounded(text),
            annotations: Vec::new(),
        }
    }

    fn user(text: &str) -> CanonicalMessage {
        CanonicalMessage {
            role: Role::User,
            blocks: vec![text_block(text)],
        }
    }

    fn tool_name(name: &str) -> ToolName {
        ToolName::parse(name).expect("a fixture tool name is well formed")
    }

    fn lookup_tool() -> CanonicalToolDef {
        CanonicalToolDef {
            name: tool_name("lookup"),
            description: fixture::bounded("Look things up"),
            input_schema: CanonicalJson::parse("{\"type\":\"object\"}").expect("schema"),
            strict: true,
        }
    }

    /// An owned request, because [`RequestView`] borrows and a
    /// [`aex_model_catalog::QualifiedModel`] can only be minted by a loaded
    /// catalog revision.
    struct Draft {
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
    }

    impl Draft {
        fn new() -> Self {
            Self {
                system: Vec::new(),
                messages: vec![user("hello")],
                tools: Vec::new(),
                tool_choice: ToolChoice::Auto,
                parallel_tools: true,
                max_output_tokens: 4_096,
                temperature_milli: None,
                top_p_milli: None,
                stop_sequences: Vec::new(),
                reasoning: ReasoningRequest::ProviderDefault,
                structured_output: None,
            }
        }

        fn view(&self) -> RequestView<'_> {
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
            }
        }

        fn body(&self, entry: &ModelEntry) -> Result<Value, RequestBuildError> {
            build_body(entry, self.view())
        }

        fn json(&self, entry: &ModelEntry) -> String {
            serde_json::to_string(&self.body(entry).expect("the draft builds"))
                .expect("a Value serializes")
        }

        fn refusal(&self, entry: &ModelEntry) -> RequestBuildError {
            self.body(entry).expect_err("the draft must be refused")
        }
    }

    // -----------------------------------------------------------------------
    // request goldens
    // -----------------------------------------------------------------------

    #[test]
    fn the_request_body_is_the_pinned_deepseek_shape() {
        let mut draft = Draft::new();
        draft.system = vec![SystemBlock {
            text: fixture::bounded("Answer in JSON."),
            cacheable: false,
        }];
        draft.reasoning = ReasoningRequest::Enabled {
            budget_tokens: None,
            effort: Some(ReasoningEffort::High),
        };
        draft.structured_output = Some(StructuredOutputRequest::JsonObject);
        crate::golden::assert_json_eq(
            &draft.json(&entry()),
            concat!(
                r#"{"max_tokens":4096,"#,
                r#""messages":[{"content":"Answer in JSON.","role":"system"},"#,
                r#"{"content":"hello","role":"user"}],"#,
                r#""model":"deepseek-v4-pro","reasoning_effort":"high","#,
                r#""response_format":{"type":"json_object"},"stream":true,"#,
                r#""stream_options":{"include_usage":true},"#,
                r#""thinking":{"reasoning_effort":"high","type":"enabled"}}"#,
            ),
        );
    }

    #[test]
    fn qualification_builds_the_production_wire_shape_from_a_staged_entry() {
        let entry = entry();
        assert_eq!(entry.state, aex_model_catalog::document::EntryState::Staged);
        let draft = Draft::new();
        let request = build_qualification_request(
            &entry,
            QualificationRequest {
                system: &draft.system,
                messages: &draft.messages,
                tools: &draft.tools,
                tool_choice: &draft.tool_choice,
                parallel_tools: draft.parallel_tools,
                max_output_tokens: draft.max_output_tokens,
                temperature_milli: draft.temperature_milli,
                top_p_milli: draft.top_p_milli,
                stop_sequences: &draft.stop_sequences,
                reasoning: draft.reasoning,
                structured_output: draft.structured_output.as_ref(),
            },
        )
        .expect("a staged candidate uses the production request builder");
        assert_eq!(
            request.url().expect("compiled URL").as_str(),
            "https://api.deepseek.com/chat/completions"
        );
        let body: Value = serde_json::from_slice(&request.body).expect("request JSON");
        assert_eq!(body["model"], "deepseek-v4-pro");
        assert_eq!(body["stream"], true);
    }

    #[test]
    fn qualification_non_stream_parity_reuses_the_production_decoder() {
        let entry = entry();
        let draft = Draft::new();
        let request = QualificationRequest {
            system: &draft.system,
            messages: &draft.messages,
            tools: &draft.tools,
            tool_choice: &draft.tool_choice,
            parallel_tools: draft.parallel_tools,
            max_output_tokens: draft.max_output_tokens,
            temperature_milli: draft.temperature_milli,
            top_p_milli: draft.top_p_milli,
            stop_sequences: &draft.stop_sequences,
            reasoning: draft.reasoning,
            structured_output: draft.structured_output.as_ref(),
        };
        let wire = build_non_streamed_qualification_request(&entry, request)
            .expect("a staged candidate can request the parity shape");
        assert_eq!(wire.accept, Accept::Json);
        let body: Value = serde_json::from_slice(&wire.body).expect("request JSON");
        assert_eq!(body["stream"], false);
        assert!(body.get("stream_options").is_none());

        let response = BoundedBody::new(
            br#"{"object":"chat.completion","choices":[{"index":0,"message":{"role":"assistant","content":"hello"},"finish_reason":"stop"}],"usage":{"prompt_tokens":10,"prompt_cache_hit_tokens":2,"prompt_cache_miss_tokens":8,"completion_tokens":1,"completion_tokens_details":{"reasoning_tokens":0},"total_tokens":11}}"#.to_vec(),
            false,
        );
        let sealed =
            decode_non_streamed_qualification_response(&response, &StreamBudget::default())
                .expect("the non-streamed shape passes through the production decoder");
        assert_eq!(sealed.stop_reason, StopReason::EndTurn);
        assert_eq!(sealed.usage.input_tokens, 8);
        assert_eq!(sealed.usage.cache_read_input_tokens, 2);
        assert_eq!(sealed.usage.output_tokens, 1);
        assert_eq!(sealed.usage.completeness, UsageCompleteness::Exact);
    }

    #[test]
    fn the_tool_request_body_is_the_nested_function_shape() {
        let mut draft = Draft::new();
        draft.tools = vec![lookup_tool()];
        draft.tool_choice = ToolChoice::Named {
            name: tool_name("lookup"),
        };
        draft.stop_sequences = vec![fixture::bounded("END")];
        crate::golden::assert_json_eq(
            &draft.json(&entry()),
            concat!(
                r#"{"max_tokens":4096,"messages":[{"content":"hello","role":"user"}],"#,
                r#""model":"deepseek-v4-pro","stop":["END"],"stream":true,"#,
                r#""stream_options":{"include_usage":true},"#,
                r#""tool_choice":{"function":{"name":"lookup"},"type":"function"},"#,
                r#""tools":[{"function":{"description":"Look things up","name":"lookup","#,
                r#""parameters":{"type":"object"},"strict":true},"type":"function"}]}"#,
            ),
        );
    }

    #[test]
    fn sampling_is_exact_decimal_and_the_deprecated_penalties_are_never_sent() {
        let mut draft = Draft::new();
        draft.reasoning = ReasoningRequest::Disabled;
        draft.temperature_milli = Some(700);
        draft.top_p_milli = Some(900);
        let json = draft.json(&entry());
        assert!(json.contains(r#""temperature":0.7"#), "{json}");
        assert!(json.contains(r#""top_p":0.9"#), "{json}");
        assert!(json.contains(r#""thinking":{"type":"disabled"}"#), "{json}");
        assert!(!json.contains("frequency_penalty"), "{json}");
        assert!(!json.contains("presence_penalty"), "{json}");
    }

    #[test]
    fn a_whole_temperature_renders_without_a_fraction() {
        let mut draft = Draft::new();
        draft.reasoning = ReasoningRequest::Disabled;
        draft.temperature_milli = Some(2_000);
        assert!(draft.json(&entry()).contains(r#""temperature":2"#));
    }

    #[test]
    fn the_path_and_auth_are_pinned_to_the_chat_completions_surface() {
        let built = build(&entry(), Draft::new().view()).expect("builds");
        assert_eq!(built.endpoint, EndpointPin::DeepSeekApi);
        assert_eq!(built.path.as_str(), CHAT_COMPLETIONS_PATH);
        assert_eq!(built.auth, AuthScheme::BearerAuthorization);
        assert_eq!(
            built.url().expect("assembles").as_str(),
            "https://api.deepseek.com/chat/completions"
        );
        assert!(built.query.is_empty(), "no launch surface takes a query");
        assert_eq!(built.accept, Accept::TextEventStream);
    }

    #[test]
    fn the_adapter_speaks_for_deepseek_only() {
        let adapter = DeepSeekAdapter;
        assert_eq!(adapter.provider(), ProviderId::Deepseek);
        let mut foreign = entry();
        foreign.provider = ProviderId::Openai;
        assert_eq!(
            build_body(&foreign, Draft::new().view()),
            Err(RequestBuildError::Encoding {
                reason: "this adapter speaks only for deepseek",
            })
        );
    }

    // -----------------------------------------------------------------------
    // every RequestBuildError arm this dialect raises
    // -----------------------------------------------------------------------

    #[test]
    fn a_json_schema_request_is_refused_before_dispatch() {
        let mut draft = Draft::new();
        let requested = StructuredOutputRequest::JsonSchema {
            name: tool_name("answer"),
            schema: CanonicalJson::parse("{\"type\":\"object\"}").expect("schema"),
            strict: true,
        };
        draft.structured_output = Some(requested.clone());
        assert_eq!(
            draft.refusal(&entry()),
            RequestBuildError::StructuredOutputUnsupported {
                requested: Box::new(requested),
            }
        );
    }

    #[test]
    fn json_object_mode_without_the_word_json_in_the_prompt_is_refused() {
        let mut draft = Draft::new();
        draft.structured_output = Some(StructuredOutputRequest::JsonObject);
        assert_eq!(
            draft.refusal(&entry()),
            RequestBuildError::StructuredOutputPromptRequirement
        );
    }

    #[test]
    fn json_object_mode_accepts_the_word_from_a_user_turn() {
        let mut draft = Draft::new();
        draft.messages = vec![user("reply as Json please")];
        draft.structured_output = Some(StructuredOutputRequest::JsonObject);
        assert!(
            draft
                .json(&entry())
                .contains(r#""response_format":{"type":"json_object"}"#),
            "the requirement is case-insensitive and satisfied by user text"
        );
    }

    #[test]
    fn reasoning_forbids_temperature() {
        let mut draft = Draft::new();
        draft.reasoning = ReasoningRequest::Enabled {
            budget_tokens: None,
            effort: None,
        };
        draft.temperature_milli = Some(500);
        assert_eq!(
            draft.refusal(&entry()),
            RequestBuildError::SamplingWithReasoning {
                field: "temperature",
            }
        );
    }

    #[test]
    fn reasoning_forbids_top_p() {
        let mut draft = Draft::new();
        draft.reasoning = ReasoningRequest::Enabled {
            budget_tokens: None,
            effort: None,
        };
        draft.top_p_milli = Some(500);
        assert_eq!(
            draft.refusal(&entry()),
            RequestBuildError::SamplingWithReasoning { field: "top_p" }
        );
    }

    #[test]
    fn the_provider_default_still_counts_as_reasoning_on() {
        // `DeepSeek` reasons by default, so sending nothing still lands in
        // thinking mode and the sampling prohibition still applies.
        let mut draft = Draft::new();
        draft.temperature_milli = Some(500);
        assert_eq!(
            draft.refusal(&entry()),
            RequestBuildError::SamplingWithReasoning {
                field: "temperature",
            }
        );
    }

    #[test]
    fn a_temperature_outside_the_pairs_range_is_refused() {
        let mut draft = Draft::new();
        draft.reasoning = ReasoningRequest::Disabled;
        draft.temperature_milli = Some(2_001);
        assert_eq!(
            draft.refusal(&entry()),
            RequestBuildError::SamplingUnsupported {
                field: "temperature",
            }
        );
    }

    #[test]
    fn a_top_p_outside_the_pairs_range_is_refused() {
        let mut draft = Draft::new();
        draft.reasoning = ReasoningRequest::Disabled;
        draft.top_p_milli = Some(1);
        assert_eq!(
            draft.refusal(&entry()),
            RequestBuildError::SamplingUnsupported { field: "top_p" }
        );
    }

    #[test]
    fn disabling_parallel_tool_calls_is_refused_rather_than_dropped() {
        let mut draft = Draft::new();
        draft.tools = vec![lookup_tool()];
        draft.parallel_tools = false;
        assert_eq!(
            draft.refusal(&entry()),
            RequestBuildError::SamplingUnsupported {
                field: "parallel_tool_calls",
            }
        );
    }

    #[test]
    fn a_seventeenth_stop_sequence_is_refused_by_the_dialect_bound() {
        let mut draft = Draft::new();
        draft.stop_sequences = (0..17)
            .map(|n| fixture::bounded(&format!("s{n}")))
            .collect();
        assert_eq!(
            draft.refusal(&entry()),
            RequestBuildError::StopSequenceLimit { max: 16 }
        );
    }

    #[test]
    fn the_catalog_can_narrow_the_stop_sequence_bound_but_never_widen_it() {
        let mut narrow = entry();
        narrow.limits.max_stop_sequences = 4;
        let mut draft = Draft::new();
        draft.stop_sequences = (0..5).map(|n| fixture::bounded(&format!("s{n}"))).collect();
        assert_eq!(
            draft.refusal(&narrow),
            RequestBuildError::StopSequenceLimit { max: 4 }
        );

        let mut wide = entry();
        wide.limits.max_stop_sequences = 200;
        draft.stop_sequences = (0..17)
            .map(|n| fixture::bounded(&format!("s{n}")))
            .collect();
        assert_eq!(
            draft.refusal(&wide),
            RequestBuildError::StopSequenceLimit { max: 16 },
            "a document must not raise a dialect bound"
        );
    }

    #[test]
    fn a_hundred_and_twenty_ninth_tool_is_refused() {
        let mut draft = Draft::new();
        draft.tools = (0..129)
            .map(|n| CanonicalToolDef {
                name: tool_name(&format!("tool_{n}")),
                ..lookup_tool()
            })
            .collect();
        assert_eq!(
            draft.refusal(&entry()),
            RequestBuildError::ToolLimit { max: 128 }
        );
    }

    #[test]
    fn a_tool_name_outside_the_providers_grammar_is_refused() {
        let mut long = entry();
        long.tool_policy.max_name_bytes = 4;
        let mut draft = Draft::new();
        draft.tools = vec![lookup_tool()];
        assert_eq!(
            draft.refusal(&long),
            RequestBuildError::ToolNameInvalid {
                name: tool_name("lookup"),
            }
        );
    }

    #[test]
    fn an_undeclared_capability_is_refused_before_dispatch() {
        let mut bare = entry();
        bare.capabilities = CapabilitySet::from_slice(&[Capability::Streaming]);
        let mut draft = Draft::new();
        draft.tools = vec![lookup_tool()];
        assert_eq!(
            draft.refusal(&bare),
            RequestBuildError::CapabilityUnavailable {
                capability: Capability::Tools,
            }
        );
    }

    #[test]
    fn a_pair_that_does_not_stream_cannot_be_dispatched_at_all() {
        let mut bare = entry();
        bare.capabilities = CapabilitySet::EMPTY;
        assert_eq!(
            Draft::new().refusal(&bare),
            RequestBuildError::CapabilityUnavailable {
                capability: Capability::Streaming,
            }
        );
    }

    #[test]
    fn a_named_tool_choice_that_names_no_declared_tool_is_refused() {
        let mut draft = Draft::new();
        draft.tools = vec![lookup_tool()];
        draft.tool_choice = ToolChoice::Named {
            name: tool_name("absent"),
        };
        assert_eq!(
            draft.refusal(&entry()),
            RequestBuildError::ToolChoiceUnsupported {
                requested: Box::new(ToolChoice::Named {
                    name: tool_name("absent"),
                }),
            }
        );
    }

    #[test]
    fn a_required_tool_choice_with_no_tools_is_refused() {
        let mut draft = Draft::new();
        draft.tool_choice = ToolChoice::Required;
        assert_eq!(
            draft.refusal(&entry()),
            RequestBuildError::ToolChoiceUnsupported {
                requested: Box::new(ToolChoice::Required),
            }
        );
    }

    #[test]
    fn an_output_ceiling_outside_the_pairs_range_is_refused() {
        let mut draft = Draft::new();
        draft.max_output_tokens = 1_000_000;
        assert_eq!(
            draft.refusal(&entry()),
            RequestBuildError::OutputTokensOutOfRange { min: 1, max: 8_192 }
        );
    }

    #[test]
    fn a_reasoning_budget_is_refused_because_the_dialect_takes_an_effort() {
        let mut draft = Draft::new();
        draft.reasoning = ReasoningRequest::Enabled {
            budget_tokens: Some(2_048),
            effort: None,
        };
        assert_eq!(
            draft.refusal(&entry()),
            RequestBuildError::ReasoningBudgetOutOfRange { min: 0, max: 0 }
        );
    }

    #[test]
    fn reasoning_material_from_another_provider_cannot_be_replayed() {
        let mut draft = Draft::new();
        draft.messages = vec![CanonicalMessage {
            role: Role::Assistant,
            blocks: vec![CanonicalBlock::Reasoning(ReasoningBlock {
                body: ReasoningBody::Text {
                    text: fixture::bounded("thought"),
                },
                token: Some(ReasoningToken {
                    provenance: ProviderId::Anthropic,
                    bytes: Bytes::from_static(b"thought"),
                }),
            })],
        }];
        assert_eq!(
            draft.refusal(&entry()),
            RequestBuildError::ReasoningProvenanceMismatch {
                expected: ProviderId::Deepseek,
                found: ProviderId::Anthropic,
            }
        );
    }

    #[test]
    fn an_assistant_turn_with_tool_calls_must_carry_reasoning_content() {
        let mut draft = Draft::new();
        draft.messages = vec![CanonicalMessage {
            role: Role::Assistant,
            blocks: vec![CanonicalBlock::ToolUse {
                id: fixture::bounded("call_1"),
                name: tool_name("lookup"),
                input: CanonicalJson::parse("{}").expect("json"),
            }],
        }];
        assert_eq!(
            draft.refusal(&entry()),
            RequestBuildError::ReasoningTokenRequired
        );
    }

    #[test]
    fn reasoning_content_is_echoed_on_the_assistant_turn_that_carried_tool_calls() {
        let mut draft = Draft::new();
        draft.messages = vec![
            user("find it"),
            CanonicalMessage {
                role: Role::Assistant,
                blocks: vec![
                    CanonicalBlock::Reasoning(ReasoningBlock {
                        body: ReasoningBody::Text {
                            text: fixture::bounded("ignored when a token is carried"),
                        },
                        token: Some(ReasoningToken {
                            provenance: ProviderId::Deepseek,
                            bytes: Bytes::from_static(b"the echoed reasoning"),
                        }),
                    }),
                    CanonicalBlock::ToolUse {
                        id: fixture::bounded("call_1"),
                        name: tool_name("lookup"),
                        input: CanonicalJson::parse("{\"q\":\"x\"}").expect("json"),
                    },
                ],
            },
            CanonicalMessage {
                role: Role::User,
                blocks: vec![CanonicalBlock::ToolResult {
                    call: fixture::bounded("call_1"),
                    content: vec![ToolResultPart::Text {
                        text: fixture::bounded("found"),
                    }],
                    is_error: false,
                }],
            },
        ];
        let json = crate::golden::sorted(&draft.json(&entry()));
        assert!(
            json.contains(r#""reasoning_content":"the echoed reasoning""#),
            "{json}"
        );
        assert!(
            json.contains(r#""tool_calls":[{"function":{"arguments":"{\"q\":\"x\"}","name":"lookup"},"id":"call_1","type":"function"}]"#),
            "{json}"
        );
        assert!(
            json.contains(r#"{"content":"found","role":"tool","tool_call_id":"call_1"}"#),
            "{json}"
        );
    }

    #[test]
    fn a_body_over_the_pairs_bound_is_refused() {
        let mut small = entry();
        small.limits.request_body_max_bytes = 32;
        assert_eq!(
            build(&small, Draft::new().view()),
            Err(RequestBuildError::BodyTooLarge { limit: 32 })
        );
    }

    #[test]
    fn a_tool_use_block_on_a_user_turn_is_refused() {
        let mut draft = Draft::new();
        draft.messages = vec![CanonicalMessage {
            role: Role::User,
            blocks: vec![CanonicalBlock::ToolUse {
                id: fixture::bounded("call_1"),
                name: tool_name("lookup"),
                input: CanonicalJson::parse("{}").expect("json"),
            }],
        }];
        assert_eq!(
            draft.refusal(&entry()),
            RequestBuildError::Encoding {
                reason: "a user turn may carry only text and tool results",
            }
        );
    }

    // -----------------------------------------------------------------------
    // streaming harness
    // -----------------------------------------------------------------------

    /// Feeds a raw `SSE` script through the shared decoder and this dialect, so
    /// the noise rules are proved end to end rather than assumed.
    fn run(script: &str) -> Result<(Vec<FrameOutcome>, DialectState), FrameDecodeError> {
        let adapter = DeepSeekAdapter;
        let budget = StreamBudget::default();
        let mut state = DialectState::new();
        let mut decoder = SseDecoder::new(budget.max_frame_bytes);
        decoder
            .push(script.as_bytes())
            .expect("the script is valid SSE");
        let events = decoder.drain().expect("the script drains");
        decoder
            .finish()
            .expect("the script ends on a frame boundary");
        let mut outcomes = Vec::with_capacity(events.len());
        for event in &events {
            outcomes.push(adapter.decode(&mut state, &event.as_ref(), &budget)?);
        }
        Ok((outcomes, state))
    }

    fn sealed(script: &str) -> Result<SealedResponse, FrameDecodeError> {
        let (_, state) = run(script)?;
        DeepSeekAdapter.finish(state)
    }

    fn chunk(delta: &str, finish: &str) -> String {
        format!(
            "data: {{\"id\":\"x\",\"object\":\"chat.completion.chunk\",\"choices\":[{{\"index\":0,\
             \"delta\":{delta},\"finish_reason\":{finish}}}],\"usage\":null}}\n\n"
        )
    }

    const TRAILING_USAGE: &str = concat!(
        "data: {\"id\":\"x\",\"object\":\"chat.completion.chunk\",\"choices\":[],",
        "\"usage\":{\"prompt_tokens\":100,\"prompt_cache_hit_tokens\":40,",
        "\"prompt_cache_miss_tokens\":60,\"completion_tokens\":25,\"total_tokens\":125,",
        "\"completion_tokens_details\":{\"reasoning_tokens\":9}}}\n\n",
    );

    const DONE: &str = "data: [DONE]\n\n";

    fn text_stream() -> String {
        format!(
            "{}{}{TRAILING_USAGE}{DONE}",
            chunk("{\"role\":\"assistant\",\"content\":\"he\"}", "null"),
            chunk("{\"content\":\"llo\"}", "\"stop\""),
        )
    }

    // -----------------------------------------------------------------------
    // frame taxonomy
    // -----------------------------------------------------------------------

    #[test]
    fn the_first_chunk_with_a_choice_starts_the_response_exactly_once() {
        let (outcomes, _) = run(&text_stream()).expect("decodes");
        assert_eq!(
            outcomes,
            vec![
                FrameOutcome::ResponseStarted,
                FrameOutcome::Progress,
                FrameOutcome::Progress,
                FrameOutcome::Terminal,
            ]
        );
    }

    #[test]
    fn keep_alive_comments_and_blank_padding_never_reach_the_dialect() {
        let script = format!(": keep-alive\n\n\n\n{}", text_stream());
        let (outcomes, state) = run(&script).expect("decodes");
        assert_eq!(
            outcomes.first(),
            Some(&FrameOutcome::ResponseStarted),
            "a keep-alive must not be the first decoded frame"
        );
        assert_eq!(state.ledger.frames, 4, "noise is not a dialect frame");
    }

    #[test]
    fn an_empty_choices_chunk_never_starts_the_response() {
        let script = format!("{TRAILING_USAGE}{DONE}");
        let (outcomes, state) = run(&script).expect("decodes");
        assert_eq!(
            outcomes,
            vec![FrameOutcome::Progress, FrameOutcome::Terminal]
        );
        assert!(
            !state.response_started,
            "`choices: []` is not proof the provider is generating"
        );
    }

    #[test]
    fn the_done_sentinel_is_terminal_and_does_not_start_the_response() {
        let (outcomes, state) = run(DONE).expect("decodes");
        assert_eq!(outcomes, vec![FrameOutcome::Terminal]);
        assert!(state.terminal);
        assert!(!state.response_started);
    }

    #[test]
    fn a_frame_that_is_not_json_is_refused() {
        assert_eq!(
            run("data: not-json\n\n").expect_err("refused"),
            FrameDecodeError::NotJson
        );
    }

    #[test]
    fn a_foreign_dialect_object_is_a_protocol_violation() {
        let error = run("data: {\"object\":\"response.completed\"}\n\n").expect_err("refused");
        assert!(
            matches!(&error, FrameDecodeError::UnknownEvent { event } if event.as_str() == "response.completed"),
            "{error:?}"
        );
    }

    #[test]
    fn a_named_sse_event_is_a_protocol_violation_on_a_data_only_dialect() {
        let error = run("event: message_start\ndata: {}\n\n").expect_err("refused");
        assert!(
            matches!(&error, FrameDecodeError::UnknownEvent { event } if event.as_str() == "message_start"),
            "{error:?}"
        );
    }

    #[test]
    fn an_unknown_chunk_shape_is_refused_rather_than_ignored() {
        assert_eq!(
            run("data: {\"surprise\":1}\n\n").expect_err("refused"),
            FrameDecodeError::MalformedField { field: "choices" }
        );
    }

    #[test]
    fn an_unknown_delta_key_is_refused() {
        let script = chunk("{\"content\":\"hi\",\"audio\":{}}", "null");
        assert_eq!(
            run(&script).expect_err("refused"),
            FrameDecodeError::MalformedField { field: "delta" }
        );
    }

    #[test]
    fn an_unknown_finish_reason_is_refused() {
        let script = chunk("{\"content\":\"hi\"}", "\"pause_turn\"");
        assert_eq!(
            run(&script).expect_err("refused"),
            FrameDecodeError::MalformedField {
                field: "finish_reason",
            }
        );
    }

    #[test]
    fn a_stream_that_ends_without_the_sentinel_is_refused() {
        let script = format!(
            "{}{TRAILING_USAGE}",
            chunk("{\"content\":\"hi\"}", "\"stop\"")
        );
        assert_eq!(
            sealed(&script).expect_err("refused"),
            FrameDecodeError::OutOfOrder {
                reason: "the stream ended without the [DONE] sentinel",
            }
        );
    }

    #[test]
    fn a_stream_that_ends_without_a_finish_reason_is_refused() {
        let script = format!("{}{DONE}", chunk("{\"content\":\"hi\"}", "null"));
        assert_eq!(
            sealed(&script).expect_err("refused"),
            FrameDecodeError::OutOfOrder {
                reason: "the stream ended without a finish_reason",
            }
        );
    }

    // -----------------------------------------------------------------------
    // block assembly
    // -----------------------------------------------------------------------

    #[test]
    fn a_text_stream_seals_into_one_text_block() {
        let response = sealed(&text_stream()).expect("seals");
        assert_eq!(response.stop_reason, StopReason::EndTurn);
        assert_eq!(
            response.blocks,
            vec![CanonicalBlock::Text {
                text: fixture::bounded("hello"),
                annotations: Vec::new(),
            }]
        );
        assert_eq!(response.provider_request_id, None);
    }

    #[test]
    fn reasoning_content_seals_with_deepseek_provenance() {
        let script = format!(
            "{}{}{TRAILING_USAGE}{DONE}",
            chunk("{\"reasoning_content\":\"step \"}", "null"),
            chunk(
                "{\"reasoning_content\":\"two\",\"content\":\"done\"}",
                "\"stop\""
            ),
        );
        let response = sealed(&script).expect("seals");
        assert_eq!(
            response.blocks[0],
            CanonicalBlock::Reasoning(ReasoningBlock {
                body: ReasoningBody::Text {
                    text: fixture::bounded("step two"),
                },
                token: Some(ReasoningToken {
                    provenance: ProviderId::Deepseek,
                    bytes: Bytes::from_static(b"step two"),
                }),
            }),
            "the echoed material must be replayable to this provider only"
        );
    }

    #[test]
    fn tool_arguments_reassemble_from_openai_compatible_fragments() {
        let script = format!(
            "{}{}{}{TRAILING_USAGE}{DONE}",
            chunk(
                "{\"tool_calls\":[{\"index\":0,\"id\":\"call_1\",\"type\":\"function\",\
                 \"function\":{\"name\":\"lookup\",\"arguments\":\"\"}}]}",
                "null"
            ),
            chunk(
                "{\"tool_calls\":[{\"index\":0,\"function\":{\"arguments\":\"{\\\"q\\\":\"}}]}",
                "null"
            ),
            chunk(
                "{\"tool_calls\":[{\"index\":0,\"function\":{\"arguments\":\"\\\"rust\\\"}\"}}]}",
                "\"tool_calls\""
            ),
        );
        let response = sealed(&script).expect("seals");
        assert_eq!(response.stop_reason, StopReason::ToolUse);
        assert_eq!(
            response.blocks,
            vec![CanonicalBlock::ToolUse {
                id: fixture::bounded("call_1"),
                name: tool_name("lookup"),
                input: CanonicalJson::parse("{\"q\":\"rust\"}").expect("json"),
            }]
        );
    }

    #[test]
    fn a_fragment_boundary_inside_a_string_escape_reassembles() {
        // The split lands between the backslash and the `n` of a `\n` escape,
        // which is the classic place a per-fragment parse would corrupt.
        let script = format!(
            "{}{}{}{TRAILING_USAGE}{DONE}",
            chunk(
                "{\"tool_calls\":[{\"index\":0,\"id\":\"call_1\",\"type\":\"function\",\
                 \"function\":{\"name\":\"lookup\",\"arguments\":\"{\\\"q\\\":\\\"a\\\\\"}}]}",
                "null"
            ),
            chunk(
                "{\"tool_calls\":[{\"index\":0,\"function\":{\"arguments\":\"nb\\\"}\"}}]}",
                "null"
            ),
            chunk("{}", "\"tool_calls\""),
        );
        let response = sealed(&script).expect("seals");
        assert_eq!(
            response.blocks,
            vec![CanonicalBlock::ToolUse {
                id: fixture::bounded("call_1"),
                name: tool_name("lookup"),
                input: CanonicalJson::parse("{\"q\":\"a\\nb\"}").expect("json"),
            }]
        );
    }

    #[test]
    fn two_parallel_tool_calls_keep_their_own_fragment_indices() {
        let script = format!(
            "{}{}{TRAILING_USAGE}{DONE}",
            chunk(
                "{\"tool_calls\":[{\"index\":0,\"id\":\"call_a\",\"function\":{\"name\":\"lookup\",\
                 \"arguments\":\"{\\\"q\\\":1}\"}},{\"index\":1,\"id\":\"call_b\",\
                 \"function\":{\"name\":\"lookup\",\"arguments\":\"{\\\"q\\\":\"}}]}",
                "null"
            ),
            chunk(
                "{\"tool_calls\":[{\"index\":1,\"function\":{\"arguments\":\"2}\"}}]}",
                "\"tool_calls\""
            ),
        );
        let response = sealed(&script).expect("seals");
        assert_eq!(response.blocks.len(), 2);
        assert_eq!(
            response.blocks[1],
            CanonicalBlock::ToolUse {
                id: fixture::bounded("call_b"),
                name: tool_name("lookup"),
                input: CanonicalJson::parse("{\"q\":2}").expect("json"),
            }
        );
    }

    #[test]
    fn tool_arguments_that_do_not_reassemble_into_json_are_refused() {
        let script = format!(
            "{}{TRAILING_USAGE}{DONE}",
            chunk(
                "{\"tool_calls\":[{\"index\":0,\"id\":\"call_1\",\
                 \"function\":{\"name\":\"lookup\",\"arguments\":\"{\\\"q\\\":\"}}]}",
                "\"tool_calls\""
            ),
        );
        assert_eq!(
            sealed(&script).expect_err("refused"),
            FrameDecodeError::ToolArgumentsNotJson {
                call: fixture::bounded("call_1"),
            }
        );
    }

    #[test]
    fn tool_calls_with_a_non_tool_finish_reason_are_refused() {
        let script = format!(
            "{}{TRAILING_USAGE}{DONE}",
            chunk(
                "{\"tool_calls\":[{\"index\":0,\"id\":\"call_1\",\
                 \"function\":{\"name\":\"lookup\",\"arguments\":\"{}\"}}]}",
                "\"stop\""
            ),
        );
        assert_eq!(
            sealed(&script).expect_err("refused"),
            FrameDecodeError::OutOfOrder {
                reason: "tool calls and the finish reason disagree",
            }
        );
    }

    // -----------------------------------------------------------------------
    // the trailing-usage-chunk trap
    // -----------------------------------------------------------------------

    #[test]
    fn usage_comes_from_the_trailing_empty_choices_chunk() {
        let response = sealed(&text_stream()).expect("seals");
        assert_eq!(
            response.usage.input_tokens, 60,
            "the cache miss is the input"
        );
        assert_eq!(response.usage.cache_read_input_tokens, 40);
        assert_eq!(response.usage.output_tokens, 25);
        assert_eq!(response.usage.reasoning_tokens, 9);
        assert_eq!(response.usage.provider_total_tokens, Some(125));
        assert_eq!(response.usage.completeness, UsageCompleteness::Exact);
        assert!(response.usage.is_consistent());
    }

    #[test]
    fn a_mid_stream_null_usage_does_not_zero_the_tally() {
        let adapter = DeepSeekAdapter;
        let budget = StreamBudget::default();
        let mut state = DialectState::new();
        let mut decoder = SseDecoder::new(budget.max_frame_bytes);
        decoder.push(TRAILING_USAGE.as_bytes()).expect("push");
        for event in &decoder.drain().expect("drain") {
            adapter
                .decode(&mut state, &event.as_ref(), &budget)
                .expect("the usage chunk decodes");
        }
        assert_eq!(state.usage.output_tokens, 25);

        // Every content chunk carries `usage: null`; merging one would zero a
        // tally the trailing chunk already filled.
        let mut decoder = SseDecoder::new(budget.max_frame_bytes);
        decoder
            .push(chunk("{\"content\":\"more\"}", "\"stop\"").as_bytes())
            .expect("push");
        for event in &decoder.drain().expect("drain") {
            adapter
                .decode(&mut state, &event.as_ref(), &budget)
                .expect("a null-usage chunk decodes");
        }
        assert_eq!(
            state.usage.output_tokens, 25,
            "a null must not zero the tally"
        );
        assert_eq!(state.usage.provider_total_tokens, Some(125));
    }

    #[test]
    fn a_stream_with_no_usage_chunk_records_absent_rather_than_zero() {
        let script = format!("{}{DONE}", chunk("{\"content\":\"hi\"}", "\"stop\""));
        let response = sealed(&script).expect("seals");
        assert_eq!(response.usage.completeness, UsageCompleteness::Absent);
        assert_eq!(response.usage.provider_total_tokens, None);
    }

    #[test]
    fn a_partial_usage_chunk_is_recorded_as_partial() {
        let script = format!(
            "{}data: {{\"object\":\"chat.completion.chunk\",\"choices\":[],\
             \"usage\":{{\"prompt_tokens\":10,\"completion_tokens\":2,\"total_tokens\":12}}}}\n\n{DONE}",
            chunk("{\"content\":\"hi\"}", "\"stop\""),
        );
        let response = sealed(&script).expect("seals");
        assert_eq!(response.usage.input_tokens, 10);
        assert!(
            matches!(
                response.usage.completeness,
                UsageCompleteness::Partial { .. }
            ),
            "{:?}",
            response.usage.completeness
        );
    }

    // -----------------------------------------------------------------------
    // the exhaustive tables
    // -----------------------------------------------------------------------

    #[test]
    fn the_stop_mapping_table_is_exhaustive() {
        assert_eq!(
            finish_token("stop"),
            Some(FinishToken::Stop(StopReason::EndTurn))
        );
        assert_eq!(
            finish_token("tool_calls"),
            Some(FinishToken::Stop(StopReason::ToolUse))
        );
        assert_eq!(
            finish_token("length"),
            Some(FinishToken::Stop(StopReason::MaxOutputTokens))
        );
        assert_eq!(
            finish_token("content_filter"),
            Some(FinishToken::Failure(ProviderFailureKind::ContentFiltered))
        );
        assert_eq!(
            finish_token("insufficient_system_resource"),
            Some(FinishToken::Failure(
                ProviderFailureKind::InsufficientProviderResource
            ))
        );
        for absent in ["", "STOP", "sensitive", "max_tokens", "end_turn"] {
            assert_eq!(
                finish_token(absent),
                None,
                "{absent} is not a DeepSeek token"
            );
        }
    }

    #[test]
    fn a_content_filter_finish_is_a_failure_and_never_a_stop_reason() {
        let script = format!(
            "{}{DONE}",
            chunk("{\"content\":\"hi\"}", "\"content_filter\"")
        );
        let (outcomes, state) = run(&script).expect("decodes");
        let FrameOutcome::Failed(failure) = &outcomes[0] else {
            panic!("expected a mid-stream failure, got {outcomes:?}");
        };
        assert_eq!(failure.kind(), ProviderFailureKind::ContentFiltered);
        assert!(
            state.finish_token.is_none(),
            "a failure token never becomes a stop reason"
        );
    }

    #[test]
    fn an_insufficient_system_resource_finish_classes_as_transient() {
        let script = format!(
            "{}{DONE}",
            chunk("{\"content\":\"hi\"}", "\"insufficient_system_resource\"")
        );
        let (outcomes, _) = run(&script).expect("decodes");
        let FrameOutcome::Failed(failure) = &outcomes[0] else {
            panic!("expected a mid-stream failure, got {outcomes:?}");
        };
        assert_eq!(
            failure.kind(),
            ProviderFailureKind::InsufficientProviderResource
        );
        assert_eq!(
            failure.class(),
            aex_model_catalog::ProviderFailureClass::Transient
        );
    }

    #[test]
    fn the_error_mapping_table_is_exhaustive() {
        for (status, expected) in [
            (400, ProviderFailureKind::InvalidRequest),
            (401, ProviderFailureKind::Authentication),
            (402, ProviderFailureKind::Billing),
            (404, ProviderFailureKind::ModelNotFound),
            (422, ProviderFailureKind::InvalidRequest),
            (429, ProviderFailureKind::RateLimited),
            (500, ProviderFailureKind::ServerError),
            (503, ProviderFailureKind::Overloaded),
        ] {
            assert_eq!(status_kind(status), expected, "status {status}");
        }
        assert_eq!(
            status_kind(418),
            ProviderFailureKind::ServerError,
            "an undocumented status falls back rather than guessing"
        );
    }

    fn classify(status: u16, body: &[u8]) -> crate::error::ProviderFailure {
        let headers = reqwest::header::HeaderMap::new();
        DeepSeekAdapter.classify_http(
            status,
            &HeaderView::new(&headers),
            &BoundedBody::new(body.to_vec(), false),
        )
    }

    #[test]
    fn exact_numeric_400_context_diagnostic_is_typed_as_overflow() {
        let failure = classify(
            400,
            br#"{"error":{"message":"This model's maximum context length is 1000000 tokens. However, you requested 1000002 tokens (1000001 in the messages, 1 in the completion). Please reduce the length of the messages or completion.","type":"invalid_request_error"}}"#,
        );
        assert_eq!(failure.kind(), ProviderFailureKind::ContextOverflow);
    }

    #[test]
    fn context_overflow_near_misses_remain_invalid_requests() {
        for body in [
            br#"{"error":{"message":"Maximum context length exceeded","code":"context_length_exceeded"}}"#.as_slice(),
            br#"{"error":{"message":"This model's maximum context length is 1000000 tokens. However, you requested 1000002 tokens (1000000 in the messages, 1 in the completion). Please reduce the length of the messages or completion."}}"#.as_slice(),
            br#"{"error":{"message":"This model's maximum context length is 1000000 tokens. However, you requested 999999 tokens (999998 in the messages, 1 in the completion). Please reduce the length of the messages or completion."}}"#.as_slice(),
            br#"{"error":{"message":"This model's maximum context length is 1000000 tokens. However, you requested 1000002 tokens (1000001 in the messages, 1 in the completion). Please reduce the length of the messages or completion!"}}"#.as_slice(),
        ] {
            assert_eq!(classify(400, body).kind(), ProviderFailureKind::InvalidRequest);
        }

        let headers = reqwest::header::HeaderMap::new();
        let exact = br#"{"error":{"message":"This model's maximum context length is 1000000 tokens. However, you requested 1000002 tokens (1000001 in the messages, 1 in the completion). Please reduce the length of the messages or completion."}}"#;
        let truncated = DeepSeekAdapter.classify_http(
            400,
            &HeaderView::new(&headers),
            &BoundedBody::new(exact.to_vec(), true),
        );
        assert_eq!(truncated.kind(), ProviderFailureKind::InvalidRequest);
        assert_eq!(
            classify(422, exact).kind(),
            ProviderFailureKind::InvalidRequest
        );
    }

    #[test]
    fn an_unparseable_402_body_still_classifies_as_billing() {
        let failure = classify(402, b"<html>Insufficient Balance</html>");
        assert_eq!(failure.kind(), ProviderFailureKind::Billing);
        assert_eq!(
            failure.class(),
            aex_model_catalog::ProviderFailureClass::Permanent,
            "a billing failure is never retried"
        );
        assert_eq!(failure.detail.http_status, Some(402));
        assert_eq!(failure.detail.provider_code, None);
        assert!(
            failure
                .detail
                .message
                .as_str()
                .contains("Insufficient Balance"),
            "{}",
            failure.detail.message
        );
    }

    #[test]
    fn an_openai_shaped_body_is_read_opportunistically() {
        let failure = classify(
            401,
            br#"{"error":{"message":"Authentication Fails","type":"authentication_error"}}"#,
        );
        assert_eq!(failure.kind(), ProviderFailureKind::Authentication);
        assert_eq!(
            failure
                .detail
                .provider_code
                .as_ref()
                .map(BoundedString::as_str),
            Some("authentication_error")
        );
        assert_eq!(failure.detail.message.as_str(), "Authentication Fails");
    }

    #[test]
    fn an_empty_error_body_still_produces_a_diagnostic() {
        let failure = classify(503, b"");
        assert_eq!(failure.kind(), ProviderFailureKind::Overloaded);
        assert_eq!(
            failure.detail.message.as_str(),
            "the provider returned no diagnostic body"
        );
    }

    #[test]
    fn a_credential_echoed_into_an_error_body_is_redacted() {
        let key = "sk-0123456789abcdef0123456789abcdef";
        let body = format!(r#"{{"error":{{"message":"bad key {key}"}}}}"#);
        let failure = classify(401, body.as_bytes());
        assert!(
            !failure.detail.message.as_str().contains(key),
            "{}",
            failure.detail.message
        );
    }

    // -----------------------------------------------------------------------
    // the positive negatives
    // -----------------------------------------------------------------------

    #[test]
    fn deepseek_publishes_no_rate_limit_feedback() {
        // A positive record, not an omission: the provider documents no
        // `x-ratelimit-*` and no `Retry-After`, so reading a stray header would
        // invent backpressure semantics it never promised.
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert("retry-after", "30".parse().expect("value"));
        headers.insert(
            "x-ratelimit-remaining-requests",
            "0".parse().expect("value"),
        );
        let feedback = DeepSeekAdapter.rate_limit_feedback(&HeaderView::new(&headers));
        assert_eq!(feedback.source, RateLimitSource::NotProvided);
        assert!(feedback.is_absent());
        assert_eq!(feedback.retry_after, None);
        assert_eq!(feedback.requests_remaining, None);
    }

    #[test]
    fn deepseek_publishes_no_request_id() {
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert("x-request-id", "req_abc".parse().expect("value"));
        let state = DialectState::new();
        assert_eq!(
            DeepSeekAdapter.request_id(&HeaderView::new(&headers), &state),
            None
        );
    }
}
