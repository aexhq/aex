//! The `openai` dialect adapter: `OpenAI` Responses over `POST /v1/responses`
//! (plan 08 §5.1).
//!
//! # What this dialect is not
//!
//! It is **not** chat completions. There is no `messages` array, no `system`
//! member, no stop-sequence member and no `[DONE]` sentinel. The request
//! carries `input` items and `instructions`; the stream carries named `event:`
//! frames whose `data` repeats the name in `type` and numbers itself with a
//! monotonic `sequence_number`.
//!
//! # The pins AEX takes (D-18)
//!
//! `store: false` — AEX owns conversation history, so no server-side state is
//! created and `GET /v1/responses/{id}` is deliberately unavailable.
//! `stream: true` and `stream_options.include_obfuscation: false`, which
//! removes padding bytes that would otherwise be counted and billed as egress.
//! `previous_response_id`, `conversation` and `background` are never sent.
//!
//! # Why a provider-hosted tool event is a rejection
//!
//! AEX declares no hosted tool, so a `web_search_call.*`, `file_search_call.*`,
//! `code_interpreter_call*`, `image_generation_call.*`, `mcp_*` or `audio*`
//! frame can only mean the request was altered in flight. Every one of them
//! decodes to [`FrameDecodeError::UnknownEvent`], which the shared core reports
//! as a `ProtocolViolation` (D-25).
//!
//! # Cancellation
//!
//! `POST /v1/responses/{id}/cancel` is documented as valid only for
//! `background: true`, which AEX never sets, so the endpoint is never called
//! (D-16). Cancellation is a connection abort.

use core::time::Duration;

use aex_model_catalog::QualifiedModel;
use aex_model_catalog::canonical::{
    CanonicalBlock, CanonicalModelRequest, NormalizedUsage, REASON_MAX, ReasoningBlock,
    ReasoningBody, ReasoningEffort, ReasoningRequest, ReasoningToken, Role, StopReason,
    StructuredOutputRequest, TEXT_MAX, ToolChoice, ToolResultPart, UsageCompleteness, UsageField,
    UsageFieldSet,
};
use aex_model_catalog::document::{
    AdapterSourceDigest, Capability, CapabilitySet, Dialect, EndpointPin, ReasoningMode,
    SamplingSupport, SchemaEncoding, StructuredOutputPolicy, ToolEncoding,
};
use aex_model_catalog::primitives::{
    Blake3Digest, BoundedString, ProviderRequestId, ToolCallId, ToolName,
};
use aex_wire::CanonicalJson;
use aex_wire::provider::ProviderId;
use bytes::Bytes;
use serde::Serialize;
use serde_json::Value;

use crate::adapter::{
    BoundedBody, DialectState, FrameDecodeError, FrameOutcome, HeaderView, PartialToolCall,
    ProviderAdapter, RequestBuildError, SealedResponse,
};
use crate::budget::{BudgetLedger, BudgetOverrun, StreamBudget};
use crate::error::{
    ProviderFailure, ProviderFailureKind, RateLimitFeedback, RateLimitSource, RedactedDetail,
};
use crate::redact::redact;
use crate::sse::SseEvent;
use crate::transport::{Accept, AuthScheme, WireRequest};

/// The single path this dialect posts to.
const RESPONSES_PATH: &str = "/v1/responses";

/// The compiled identity of this adapter's source, which every conformance
/// receipt is bound to (D-06). Changing observable behaviour means bumping it,
/// which invalidates every receipt earned against the previous behaviour.
const SOURCE_TAG: &[u8] = b"aex-brain-provider-gateway::openai@1";

/// The `include` member that makes round-trip reasoning material arrive.
const INCLUDE_ENCRYPTED_REASONING: &str = "reasoning.encrypted_content";

/// Bytes of prompt text charged to one token when estimating context use. A
/// conservative pre-dispatch estimate, never a billing figure.
const BYTES_PER_ESTIMATED_TOKEN: usize = 4;

/// Key offset that namespaces the second member of a content family inside the
/// shared per-family maps on [`DialectState`]: refusal text beside assistant
/// text in `open_text`, reasoning summary beside reasoning text in
/// `open_reasoning`.
///
/// An `output_index` at or above it is refused rather than folded, so the two
/// namespaces provably cannot collide.
const ALTERNATE_KEY_BASE: u16 = 0x8000;

/// The finish token a `completed` response records.
const FINISH_COMPLETED: &str = "completed";

/// The finish token an output-ceiling stop records.
const FINISH_MAX_OUTPUT_TOKENS: &str = "max_output_tokens";

/// The canonical refusal bound, restated as a byte budget.
const REFUSAL_BOUND: u64 = 1024;

/// The rate-limit headers this provider publishes beyond the ones read into
/// [`RateLimitFeedback`], used to decide whether it published anything at all.
const RATE_LIMIT_HEADERS: [&str; 5] = [
    "x-ratelimit-limit-requests",
    "x-ratelimit-limit-tokens",
    "x-ratelimit-limit-project-tokens",
    "x-ratelimit-reset-requests",
    "x-ratelimit-reset-tokens",
];

/// The `OpenAI` Responses dialect adapter.
///
/// Stateless: every per-stream value lives in the [`DialectState`] the shared
/// core owns, so one adapter serves every concurrent dispatch.
#[derive(Debug, Clone, Copy, Default)]
pub struct OpenAiAdapter;

// ---------------------------------------------------------------------------
// request body
// ---------------------------------------------------------------------------

/// The `POST /v1/responses` body.
///
/// Member order here is wire order: `serde_json` writes struct members in
/// declaration order, which is what makes the request goldens byte-exact.
#[derive(Debug, Serialize)]
struct ResponsesBody<'a> {
    model: &'a str,
    input: Vec<InputItem>,
    #[serde(skip_serializing_if = "Option::is_none")]
    instructions: Option<String>,
    max_output_tokens: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<serde_json::Number>,
    #[serde(skip_serializing_if = "Option::is_none")]
    top_p: Option<serde_json::Number>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    tools: Vec<ToolDef>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_choice: Option<ToolChoiceWire>,
    #[serde(skip_serializing_if = "Option::is_none")]
    parallel_tool_calls: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    text: Option<TextConfig>,
    #[serde(skip_serializing_if = "Option::is_none")]
    reasoning: Option<ReasoningConfig>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    include: Vec<&'static str>,
    store: bool,
    stream: bool,
    stream_options: StreamOptions,
}

/// `stream_options`. `include_obfuscation` is pinned off (D-18).
#[derive(Debug, Serialize)]
struct StreamOptions {
    include_obfuscation: bool,
}

/// One member of `input`.
#[derive(Debug, Serialize)]
#[serde(untagged)]
enum InputItem {
    Message(MessageItem),
    FunctionCall(FunctionCallItem),
    FunctionCallOutput(FunctionCallOutputItem),
    Reasoning(ReasoningItem),
}

/// A conversational turn.
#[derive(Debug, Serialize)]
struct MessageItem {
    #[serde(rename = "type")]
    kind: &'static str,
    role: &'static str,
    content: Vec<ContentPart>,
}

/// One part of a message's content.
#[derive(Debug, Serialize)]
struct ContentPart {
    #[serde(rename = "type")]
    kind: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    refusal: Option<String>,
}

/// A model request to run a tool, replayed on a later turn.
#[derive(Debug, Serialize)]
struct FunctionCallItem {
    #[serde(rename = "type")]
    kind: &'static str,
    call_id: String,
    name: String,
    arguments: String,
}

/// The answer to a `function_call`.
#[derive(Debug, Serialize)]
struct FunctionCallOutputItem {
    #[serde(rename = "type")]
    kind: &'static str,
    call_id: String,
    output: String,
}

/// Round-trip reasoning material.
#[derive(Debug, Serialize)]
struct ReasoningItem {
    #[serde(rename = "type")]
    kind: &'static str,
    encrypted_content: String,
    summary: Vec<Value>,
}

/// A tool declaration. Flat, not the chat-completions nesting.
#[derive(Debug, Serialize)]
struct ToolDef {
    #[serde(rename = "type")]
    kind: &'static str,
    name: String,
    description: String,
    parameters: CanonicalJson,
    strict: bool,
}

/// `tool_choice`.
#[derive(Debug, Serialize)]
#[serde(untagged)]
enum ToolChoiceWire {
    Mode(&'static str),
    Named {
        #[serde(rename = "type")]
        kind: &'static str,
        name: String,
    },
}

/// `text`, which is where the output schema lives on this dialect.
#[derive(Debug, Serialize)]
struct TextConfig {
    format: TextFormat,
}

/// `text.format`.
#[derive(Debug, Serialize)]
#[serde(untagged)]
enum TextFormat {
    JsonObject {
        #[serde(rename = "type")]
        kind: &'static str,
    },
    JsonSchema {
        #[serde(rename = "type")]
        kind: &'static str,
        name: String,
        schema: CanonicalJson,
        strict: bool,
    },
}

/// `reasoning`.
#[derive(Debug, Serialize)]
struct ReasoningConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    effort: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    summary: Option<&'static str>,
}

// ---------------------------------------------------------------------------
// the trait
// ---------------------------------------------------------------------------

impl ProviderAdapter for OpenAiAdapter {
    fn provider(&self) -> ProviderId {
        ProviderId::Openai
    }

    fn source_digest(&self) -> AdapterSourceDigest {
        AdapterSourceDigest(Blake3Digest::of(SOURCE_TAG))
    }

    fn build_request(
        &self,
        model: &QualifiedModel,
        request: &CanonicalModelRequest,
    ) -> Result<WireRequest, RequestBuildError> {
        guard_pair(model, request)?;
        guard_shape(model, request)?;

        let body = ResponsesBody {
            model: model.model().as_str(),
            input: build_input(model, request)?,
            instructions: build_instructions(model.capabilities(), request)?,
            max_output_tokens: request.max_output_tokens,
            temperature: build_temperature(model, request)?,
            top_p: build_top_p(model, request)?,
            tools: build_tools(model, request)?,
            tool_choice: build_tool_choice(model, request)?,
            parallel_tool_calls: build_parallel(model, request)?,
            text: build_text(model, request)?,
            reasoning: build_reasoning(model, request)?,
            include: build_include(model, request),
            store: false,
            stream: true,
            stream_options: StreamOptions {
                include_obfuscation: false,
            },
        };

        let bytes = serde_json::to_vec(&body).map_err(|_| RequestBuildError::Encoding {
            reason: "the request body could not be serialized",
        })?;
        let limit = model.limits().request_body_max_bytes;
        if u32::try_from(bytes.len()).unwrap_or(u32::MAX) > limit {
            return Err(RequestBuildError::BodyTooLarge { limit });
        }

        Ok(WireRequest {
            endpoint: model.endpoint(),
            path: BoundedString::new(RESPONSES_PATH).map_err(|_| RequestBuildError::Encoding {
                reason: "the responses path exceeds its bound",
            })?,
            query: Vec::new(),
            headers: vec![(
                "content-type",
                BoundedString::new("application/json").map_err(|_| {
                    RequestBuildError::Encoding {
                        reason: "the content type exceeds its bound",
                    }
                })?,
            )],
            auth: AuthScheme::BearerAuthorization,
            body: Bytes::from(bytes),
            accept: Accept::TextEventStream,
        })
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
        let payload = event.data_str().map_err(|_| FrameDecodeError::NotJson)?;
        let frame: Value = serde_json::from_str(payload).map_err(|_| FrameDecodeError::NotJson)?;
        let kind = text_of(&frame, "type")?;

        if let Some(name) = event.name
            && name != kind
        {
            return Err(FrameDecodeError::OutOfOrder {
                reason: "the SSE event name does not match the frame type",
            });
        }
        if state.terminal {
            return Err(FrameDecodeError::OutOfOrder {
                reason: "a frame arrived after the terminal frame",
            });
        }
        if !state.response_started && kind != "response.created" {
            return Err(FrameDecodeError::OutOfOrder {
                reason: "the stream did not open with response.created",
            });
        }

        state
            .ledger
            .charge_response(budget, u64::try_from(payload.len()).unwrap_or(u64::MAX))?;
        // The strongest monotonicity statement the shared state supports: with
        // contiguous numbering the two are equal, and a rewound or reordered
        // frame falls behind the frames already decoded.
        let sequence = number_of(&frame, "sequence_number")?;
        if sequence < u64::from(state.ledger.frames) {
            return Err(FrameDecodeError::OutOfOrder {
                reason: "sequence_number did not advance",
            });
        }
        state.ledger.count_frame();

        dispatch(state, kind, &frame, sequence, budget)
    }

    fn finish(&self, state: DialectState) -> Result<SealedResponse, FrameDecodeError> {
        if !state.terminal {
            return Err(FrameDecodeError::OutOfOrder {
                reason: "the stream ended without a terminal frame",
            });
        }
        if !state.open_text.is_empty()
            || !state.open_reasoning.is_empty()
            || !state.open_tools.is_empty()
        {
            return Err(FrameDecodeError::OutOfOrder {
                reason: "the stream ended with an unclosed output item",
            });
        }

        let has_tool_use = state
            .blocks
            .iter()
            .any(|block| matches!(block, CanonicalBlock::ToolUse { .. }));
        let stop_reason = match state.finish_token.as_deref() {
            Some(FINISH_COMPLETED) if has_tool_use => StopReason::ToolUse,
            Some(FINISH_COMPLETED) => StopReason::EndTurn,
            Some(FINISH_MAX_OUTPUT_TOKENS) => StopReason::MaxOutputTokens,
            _ => {
                return Err(FrameDecodeError::OutOfOrder {
                    reason: "the terminal frame carried no usable finish token",
                });
            }
        };

        Ok(SealedResponse {
            blocks: state.blocks,
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
        let parsed = body.as_json();
        let error = parsed.as_ref().and_then(|value| value.get("error"));
        let code = error
            .and_then(|value| value.get("code"))
            .and_then(Value::as_str);
        let kind = code
            .and_then(code_kind)
            .or_else(|| {
                error
                    .and_then(|value| value.get("type"))
                    .and_then(Value::as_str)
                    .and_then(type_kind)
            })
            .unwrap_or_else(|| status_kind(status));

        let message = error
            .and_then(|value| value.get("message"))
            .and_then(Value::as_str);
        let mut detail = match message {
            Some(text) => RedactedDetail::new(kind, redact(text, &[])),
            None => RedactedDetail::internal(kind, "the provider returned no readable error body"),
        }
        .with_status(status);
        if let Some(code) = code {
            detail = detail.with_code(code);
        }

        ProviderFailure {
            detail,
            rate_limit: self.rate_limit_feedback(headers),
        }
    }

    fn rate_limit_feedback(&self, headers: &HeaderView<'_>) -> RateLimitFeedback {
        let retry_after = headers.get_u64("retry-after").map(Duration::from_secs);
        let requests_remaining = headers
            .get("x-ratelimit-remaining-requests")
            .and_then(|text| text.trim().parse::<u32>().ok());
        let account = headers.get_u64("x-ratelimit-remaining-tokens");
        let project = headers.get_u64("x-ratelimit-remaining-project-tokens");
        // Two ceilings apply at once, so the binding one is the smaller.
        let tokens_remaining = match (account, project) {
            (Some(left), Some(right)) => Some(left.min(right)),
            (left, right) => left.or(right),
        };

        let published = retry_after.is_some()
            || requests_remaining.is_some()
            || tokens_remaining.is_some()
            || RATE_LIMIT_HEADERS
                .iter()
                .any(|name| headers.get(name).is_some());

        RateLimitFeedback {
            retry_after,
            requests_remaining,
            tokens_remaining,
            // `x-ratelimit-reset-*` is a relative duration such as `6m0s` and
            // this method holds no clock, so the absolute instant is recorded
            // as absent rather than invented.
            reset_at: None,
            source: if published {
                RateLimitSource::VendorHeaders
            } else {
                RateLimitSource::NotProvided
            },
        }
    }

    fn request_id(
        &self,
        headers: &HeaderView<'_>,
        state: &DialectState,
    ) -> Option<ProviderRequestId> {
        headers
            .get("x-request-id")
            .map(ProviderRequestId::truncating)
            .or_else(|| state.request_id.clone())
    }
}

// ---------------------------------------------------------------------------
// request construction
// ---------------------------------------------------------------------------

/// Refuses a pair this module does not speak for.
fn guard_pair(
    model: &QualifiedModel,
    request: &CanonicalModelRequest,
) -> Result<(), RequestBuildError> {
    if model.provider() != ProviderId::Openai {
        return Err(RequestBuildError::Encoding {
            reason: "the pinned pair does not belong to openai",
        });
    }
    if model.dialect() != Dialect::OpenAiResponses {
        return Err(RequestBuildError::Encoding {
            reason: "the entry does not declare the openai responses dialect",
        });
    }
    if model.endpoint() != EndpointPin::OpenAiApi {
        return Err(RequestBuildError::Encoding {
            reason: "the entry does not pin the openai origin",
        });
    }
    if request.selection != *model {
        return Err(RequestBuildError::Encoding {
            reason: "the request selection is not the pinned pair",
        });
    }
    Ok(())
}

/// Refuses everything about the request shape the entry does not declare,
/// before any member is encoded.
fn guard_shape(
    model: &QualifiedModel,
    request: &CanonicalModelRequest,
) -> Result<(), RequestBuildError> {
    // This dialect has no stop-sequence member at all, so a caller sequence is
    // refused rather than dropped, whatever the entry declares.
    if !request.stop_sequences.is_empty() {
        return Err(RequestBuildError::StopSequencesUnsupported);
    }
    let capabilities = model.capabilities();
    require(capabilities, Capability::Streaming)?;

    let limits = model.limits();
    if request.max_output_tokens < limits.min_output_tokens
        || request.max_output_tokens > limits.max_output_tokens
    {
        return Err(RequestBuildError::OutputTokensOutOfRange {
            min: limits.min_output_tokens,
            max: limits.max_output_tokens,
        });
    }
    if !request.cache_breakpoints.is_empty() {
        require(capabilities, Capability::PromptCacheExplicit)?;
    }

    let estimated = estimate_prompt_tokens(request);
    if estimated > limits.context_window_tokens {
        return Err(RequestBuildError::ContextOverflowEstimated {
            limit: limits.context_window_tokens,
            estimated,
        });
    }
    Ok(())
}

/// The capability gate, stated once.
fn require(set: CapabilitySet, capability: Capability) -> Result<(), RequestBuildError> {
    if set.has(capability) {
        return Ok(());
    }
    Err(RequestBuildError::CapabilityUnavailable { capability })
}

/// A conservative pre-dispatch estimate of prompt size in tokens.
fn estimate_prompt_tokens(request: &CanonicalModelRequest) -> u32 {
    let mut bytes = 0usize;
    for block in &request.system {
        bytes = bytes.saturating_add(block.text.len());
    }
    for message in &request.messages {
        for block in &message.blocks {
            bytes = bytes.saturating_add(block_bytes(block));
        }
    }
    for tool in &request.tools {
        bytes = bytes
            .saturating_add(tool.name.as_str().len())
            .saturating_add(tool.description.len())
            .saturating_add(tool.input_schema.as_str().len());
    }
    u32::try_from(bytes.div_ceil(BYTES_PER_ESTIMATED_TOKEN)).unwrap_or(u32::MAX)
}

/// How many prompt bytes one canonical block contributes.
fn block_bytes(block: &CanonicalBlock) -> usize {
    match block {
        CanonicalBlock::Text { text, .. } => text.len(),
        CanonicalBlock::Refusal { text } => text.len(),
        CanonicalBlock::Reasoning(reasoning) => {
            let body = match &reasoning.body {
                ReasoningBody::Text { text } | ReasoningBody::Summary { text } => text.len(),
                ReasoningBody::Redacted => 0,
            };
            body.saturating_add(
                reasoning
                    .token
                    .as_ref()
                    .map_or(0, |token| token.bytes.len()),
            )
        }
        CanonicalBlock::ToolUse { id, name, input } => id
            .len()
            .saturating_add(name.as_str().len())
            .saturating_add(input.as_str().len()),
        CanonicalBlock::ToolResult { call, content, .. } => {
            content.iter().fold(call.len(), |total, part| {
                let part = match part {
                    ToolResultPart::Text { text } => text.len(),
                    ToolResultPart::Json { value } => value.as_str().len(),
                };
                total.saturating_add(part)
            })
        }
    }
}

/// The single `instructions` string. This dialect has no `system` member.
fn build_instructions(
    capabilities: CapabilitySet,
    request: &CanonicalModelRequest,
) -> Result<Option<String>, RequestBuildError> {
    if request.system.is_empty() {
        return Ok(None);
    }
    require(capabilities, Capability::SystemInstruction)?;
    let joined = request
        .system
        .iter()
        .map(|block| block.text.as_str())
        .collect::<Vec<_>>()
        .join("\n\n");
    Ok(Some(joined))
}

/// `temperature`, honoured only where the entry says the pair honours it.
fn build_temperature(
    model: &QualifiedModel,
    request: &CanonicalModelRequest,
) -> Result<Option<serde_json::Number>, RequestBuildError> {
    let Some(milli) = request.temperature_milli else {
        return Ok(None);
    };
    let honoured = matches!(
        model.entry().sampling,
        SamplingSupport::TemperatureOnly | SamplingSupport::Full
    ) && model.capabilities().has(Capability::Temperature);
    if !honoured {
        return Err(RequestBuildError::SamplingUnsupported {
            field: "temperature",
        });
    }
    if model.entry().reasoning.excludes_sampling && reasoning_requested(request) {
        return Err(RequestBuildError::SamplingWithReasoning {
            field: "temperature",
        });
    }
    decimal(milli, model.limits().temperature_milli, "temperature")
}

/// `top_p`, honoured only where the entry declares full sampling.
fn build_top_p(
    model: &QualifiedModel,
    request: &CanonicalModelRequest,
) -> Result<Option<serde_json::Number>, RequestBuildError> {
    let Some(milli) = request.top_p_milli else {
        return Ok(None);
    };
    let honoured = matches!(model.entry().sampling, SamplingSupport::Full)
        && model.capabilities().has(Capability::TopP);
    if !honoured {
        return Err(RequestBuildError::SamplingUnsupported { field: "top_p" });
    }
    if model.entry().reasoning.excludes_sampling && reasoning_requested(request) {
        return Err(RequestBuildError::SamplingWithReasoning { field: "top_p" });
    }
    decimal(milli, model.limits().top_p_milli, "top_p")
}

/// Whether the request asks for reasoning explicitly.
fn reasoning_requested(request: &CanonicalModelRequest) -> bool {
    matches!(request.reasoning, ReasoningRequest::Enabled { .. })
}

/// Renders integer milli-units as a decimal `JSON` number, in range.
///
/// The canonical vocabulary carries no floats, so the decimal text is built
/// from the integer and parsed rather than divided.
fn decimal(
    milli: u16,
    range: Option<(u16, u16)>,
    field: &'static str,
) -> Result<Option<serde_json::Number>, RequestBuildError> {
    let Some((low, high)) = range else {
        return Err(RequestBuildError::SamplingUnsupported { field });
    };
    if milli < low || milli > high {
        return Err(RequestBuildError::Encoding {
            reason: "a sampling value is outside the pair's declared range",
        });
    }
    let whole = milli / 1000;
    let fraction = milli % 1000;
    let rendered = if fraction == 0 {
        whole.to_string()
    } else {
        let digits = format!("{fraction:03}");
        format!("{whole}.{}", digits.trim_end_matches('0'))
    };
    serde_json::from_str::<serde_json::Number>(&rendered)
        .map(Some)
        .map_err(|_| RequestBuildError::Encoding {
            reason: "a sampling value could not be encoded",
        })
}

/// The flat tool declarations this dialect takes.
fn build_tools(
    model: &QualifiedModel,
    request: &CanonicalModelRequest,
) -> Result<Vec<ToolDef>, RequestBuildError> {
    if request.tools.is_empty() {
        return Ok(Vec::new());
    }
    let capabilities = model.capabilities();
    require(capabilities, Capability::Tools)?;

    let policy = &model.entry().tool_policy;
    if policy.encoding != ToolEncoding::OpenAiFlat {
        return Err(RequestBuildError::Encoding {
            reason: "the entry declares a tool encoding this dialect does not speak",
        });
    }
    let max = model.limits().max_tools;
    if u16::try_from(request.tools.len()).unwrap_or(u16::MAX) > max {
        return Err(RequestBuildError::ToolLimit { max });
    }

    let mut out = Vec::with_capacity(request.tools.len());
    for tool in &request.tools {
        let name = tool.name.as_str();
        let too_long = u16::try_from(name.len()).unwrap_or(u16::MAX) > policy.max_name_bytes;
        if too_long || !policy.name_pattern.accepts(name) {
            return Err(RequestBuildError::ToolNameInvalid {
                name: tool.name.clone(),
            });
        }
        if tool.strict {
            require(capabilities, Capability::StrictToolSchema)?;
        }
        out.push(ToolDef {
            kind: "function",
            name: name.to_owned(),
            description: tool.description.as_str().to_owned(),
            parameters: tool.input_schema.clone(),
            strict: tool.strict,
        });
    }
    Ok(out)
}

/// `tool_choice`, sent only where tools are.
fn build_tool_choice(
    model: &QualifiedModel,
    request: &CanonicalModelRequest,
) -> Result<Option<ToolChoiceWire>, RequestBuildError> {
    let unsupported = || RequestBuildError::ToolChoiceUnsupported {
        requested: Box::new(request.tool_choice.clone()),
    };
    if request.tools.is_empty() {
        return match request.tool_choice {
            ToolChoice::Auto => Ok(None),
            _ => Err(unsupported()),
        };
    }
    let capabilities = model.capabilities();
    match &request.tool_choice {
        ToolChoice::Auto => Ok(Some(ToolChoiceWire::Mode("auto"))),
        ToolChoice::Required => {
            if capabilities.has(Capability::ToolChoiceRequired) {
                Ok(Some(ToolChoiceWire::Mode("required")))
            } else {
                Err(unsupported())
            }
        }
        ToolChoice::None => {
            if capabilities.has(Capability::ToolChoiceNone) {
                Ok(Some(ToolChoiceWire::Mode("none")))
            } else {
                Err(unsupported())
            }
        }
        ToolChoice::Named { name } => {
            let declared = request.tools.iter().any(|tool| tool.name == *name);
            if !capabilities.has(Capability::ToolChoiceNamed) || !declared {
                return Err(unsupported());
            }
            Ok(Some(ToolChoiceWire::Named {
                kind: "function",
                name: name.as_str().to_owned(),
            }))
        }
    }
}

/// `parallel_tool_calls`, sent only where tools are.
fn build_parallel(
    model: &QualifiedModel,
    request: &CanonicalModelRequest,
) -> Result<Option<bool>, RequestBuildError> {
    if request.tools.is_empty() {
        return Ok(None);
    }
    if request.parallel_tools {
        require(model.capabilities(), Capability::ParallelTools)?;
    }
    Ok(Some(request.parallel_tools))
}

/// `text.format`, which is where structured output lives on this dialect.
fn build_text(
    model: &QualifiedModel,
    request: &CanonicalModelRequest,
) -> Result<Option<TextConfig>, RequestBuildError> {
    let Some(requested) = &request.structured_output else {
        return Ok(None);
    };
    require(model.capabilities(), Capability::StructuredOutput)?;
    let unsupported = || RequestBuildError::StructuredOutputUnsupported {
        requested: Box::new(requested.clone()),
    };
    let policy = model.entry().structured_output;

    match requested {
        StructuredOutputRequest::JsonObject => {
            if matches!(policy, StructuredOutputPolicy::Unsupported) {
                return Err(unsupported());
            }
            Ok(Some(TextConfig {
                format: TextFormat::JsonObject {
                    kind: "json_object",
                },
            }))
        }
        StructuredOutputRequest::JsonSchema {
            name,
            schema,
            strict,
        } => {
            if !matches!(
                policy,
                StructuredOutputPolicy::JsonSchema {
                    encoding: SchemaEncoding::OpenAiTextFormat,
                    ..
                }
            ) {
                return Err(unsupported());
            }
            Ok(Some(TextConfig {
                format: TextFormat::JsonSchema {
                    kind: "json_schema",
                    name: name.as_str().to_owned(),
                    schema: schema.clone(),
                    strict: *strict,
                },
            }))
        }
    }
}

/// `reasoning`. This dialect takes an effort ladder, never a token budget.
fn build_reasoning(
    model: &QualifiedModel,
    request: &CanonicalModelRequest,
) -> Result<Option<ReasoningConfig>, RequestBuildError> {
    let mode = model.entry().reasoning.mode;
    match request.reasoning {
        ReasoningRequest::ProviderDefault => Ok(None),
        ReasoningRequest::Disabled => match mode {
            ReasoningMode::Unsupported => Ok(None),
            ReasoningMode::Optional => Ok(Some(ReasoningConfig {
                effort: Some("none"),
                summary: None,
            })),
            ReasoningMode::AlwaysOn => Err(RequestBuildError::Encoding {
                reason: "this pair cannot disable reasoning",
            }),
        },
        ReasoningRequest::Enabled {
            budget_tokens,
            effort,
        } => {
            require(model.capabilities(), Capability::Reasoning)?;
            if matches!(mode, ReasoningMode::Unsupported) {
                return Err(RequestBuildError::CapabilityUnavailable {
                    capability: Capability::Reasoning,
                });
            }
            if budget_tokens.is_some() {
                return Err(RequestBuildError::Encoding {
                    reason: "this dialect takes a reasoning effort, not a token budget",
                });
            }
            Ok(Some(ReasoningConfig {
                effort: effort.map(effort_token),
                summary: Some("auto"),
            }))
        }
    }
}

/// The neutral effort ladder onto this provider's own set.
const fn effort_token(effort: ReasoningEffort) -> &'static str {
    match effort {
        ReasoningEffort::Minimal => "minimal",
        ReasoningEffort::Low => "low",
        ReasoningEffort::Medium => "medium",
        ReasoningEffort::High => "high",
        ReasoningEffort::Max => "max",
    }
}

/// `include`, which is the only way round-trip reasoning material arrives.
fn build_include(model: &QualifiedModel, request: &CanonicalModelRequest) -> Vec<&'static str> {
    if !model.capabilities().has(Capability::ReasoningReplay)
        || matches!(request.reasoning, ReasoningRequest::Disabled)
    {
        return Vec::new();
    }
    vec![INCLUDE_ENCRYPTED_REASONING]
}

/// The `input` item list.
fn build_input(
    model: &QualifiedModel,
    request: &CanonicalModelRequest,
) -> Result<Vec<InputItem>, RequestBuildError> {
    let mut out = Vec::new();
    for message in &request.messages {
        let has_tool_use = message
            .blocks
            .iter()
            .any(|block| matches!(block, CanonicalBlock::ToolUse { .. }));
        let mut parts: Vec<ContentPart> = Vec::new();
        for block in &message.blocks {
            match block {
                CanonicalBlock::Text { text, .. } => {
                    parts.push(text_part(message.role, text.as_str()));
                }
                CanonicalBlock::Refusal { text } => parts.push(ContentPart {
                    kind: "refusal",
                    text: None,
                    refusal: Some(text.as_str().to_owned()),
                }),
                CanonicalBlock::ToolUse { id, name, input } => {
                    flush(&mut parts, message.role, &mut out);
                    out.push(InputItem::FunctionCall(FunctionCallItem {
                        kind: "function_call",
                        call_id: id.as_str().to_owned(),
                        name: name.as_str().to_owned(),
                        arguments: input.as_str().to_owned(),
                    }));
                }
                CanonicalBlock::ToolResult {
                    call,
                    content,
                    is_error,
                } => {
                    flush(&mut parts, message.role, &mut out);
                    out.push(InputItem::FunctionCallOutput(FunctionCallOutputItem {
                        kind: "function_call_output",
                        call_id: call.as_str().to_owned(),
                        output: tool_output(content, *is_error),
                    }));
                }
                CanonicalBlock::Reasoning(reasoning) => {
                    flush(&mut parts, message.role, &mut out);
                    if let Some(item) = reasoning_item(model, reasoning, has_tool_use)? {
                        out.push(InputItem::Reasoning(item));
                    }
                }
            }
        }
        flush(&mut parts, message.role, &mut out);
    }
    Ok(out)
}

/// Closes the message item that has been accumulating content parts.
fn flush(parts: &mut Vec<ContentPart>, role: Role, out: &mut Vec<InputItem>) {
    if parts.is_empty() {
        return;
    }
    out.push(InputItem::Message(MessageItem {
        kind: "message",
        role: role_token(role),
        content: core::mem::take(parts),
    }));
}

/// The wire spelling of a role.
const fn role_token(role: Role) -> &'static str {
    match role {
        Role::User => "user",
        Role::Assistant => "assistant",
    }
}

/// A text content part, whose type depends on which side wrote it.
fn text_part(role: Role, text: &str) -> ContentPart {
    ContentPart {
        kind: match role {
            Role::User => "input_text",
            Role::Assistant => "output_text",
        },
        text: Some(text.to_owned()),
        refusal: None,
    }
}

/// Renders a tool result as this dialect's single `output` string.
///
/// `function_call_output` has no error member, so a failed tool is marked in
/// the text with a fixed prefix. That is visible to the model and to a reader
/// of the journal, which a dropped flag would not be.
fn tool_output(content: &[ToolResultPart], is_error: bool) -> String {
    let mut out = String::new();
    if is_error {
        out.push_str("error: ");
    }
    for (index, part) in content.iter().enumerate() {
        if index > 0 {
            out.push('\n');
        }
        match part {
            ToolResultPart::Text { text } => out.push_str(text.as_str()),
            ToolResultPart::Json { value } => out.push_str(value.as_str()),
        }
    }
    out
}

/// A reasoning block as an input item, where the dialect can carry one.
fn reasoning_item(
    model: &QualifiedModel,
    reasoning: &ReasoningBlock,
    has_tool_use: bool,
) -> Result<Option<ReasoningItem>, RequestBuildError> {
    if let Some(token) = &reasoning.token {
        if token.provenance != ProviderId::Openai {
            return Err(RequestBuildError::ReasoningProvenanceMismatch {
                expected: ProviderId::Openai,
                found: token.provenance,
            });
        }
        let encrypted =
            core::str::from_utf8(&token.bytes).map_err(|_| RequestBuildError::Encoding {
                reason: "reasoning round-trip material is not valid UTF-8",
            })?;
        return Ok(Some(ReasoningItem {
            kind: "reasoning",
            encrypted_content: encrypted.to_owned(),
            summary: Vec::new(),
        }));
    }
    if model.requires_reasoning_token(has_tool_use) {
        return Err(RequestBuildError::ReasoningTokenRequired);
    }
    // Without encrypted material there is no member on this dialect that can
    // carry reasoning back, and the entry does not require replay, so the block
    // has no representation rather than a silently altered one.
    Ok(None)
}

// ---------------------------------------------------------------------------
// stream decoding
// ---------------------------------------------------------------------------

/// Routes one decoded frame by its `type`.
fn dispatch(
    state: &mut DialectState,
    kind: &str,
    frame: &Value,
    sequence: u64,
    budget: &StreamBudget,
) -> Result<FrameOutcome, FrameDecodeError> {
    match kind {
        "response.created" => on_created(state, frame, sequence),
        "response.in_progress"
        | "response.queued"
        | "response.content_part.added"
        | "response.content_part.done" => Ok(FrameOutcome::Progress),
        "response.output_item.added" => on_item_added(state, frame, budget),
        "response.output_item.done" => on_item_done(state, frame),
        "response.output_text.delta" => on_text_delta(state, frame, budget),
        "response.output_text.done" => on_text_done(state, frame),
        "response.refusal.delta" => on_refusal_delta(state, frame, budget),
        "response.refusal.done" => on_refusal_done(state, frame),
        "response.function_call_arguments.delta" => on_arguments_delta(state, frame, budget),
        "response.function_call_arguments.done" => on_arguments_done(state, frame),
        "response.reasoning_text.delta" => on_reasoning_delta(state, frame, budget, false),
        "response.reasoning_text.done" => on_reasoning_done(state, frame, false),
        "response.reasoning_summary_text.delta" => on_reasoning_delta(state, frame, budget, true),
        "response.reasoning_summary_text.done" => on_reasoning_done(state, frame, true),
        "response.completed" | "response.failed" | "response.incomplete" => {
            on_terminal(state, frame)
        }
        "error" => Ok(on_error(state, frame)),
        // Every hosted-tool frame lands here. AEX declares no hosted tool, so
        // receiving one means the request was altered in flight (D-25).
        other => Err(FrameDecodeError::UnknownEvent {
            event: BoundedString::truncating(other),
        }),
    }
}

/// The one frame that proves the provider is generating.
fn on_created(
    state: &mut DialectState,
    frame: &Value,
    sequence: u64,
) -> Result<FrameOutcome, FrameDecodeError> {
    if state.response_started {
        return Err(FrameDecodeError::OutOfOrder {
            reason: "response.created arrived twice",
        });
    }
    if sequence != 0 {
        return Err(FrameDecodeError::OutOfOrder {
            reason: "response.created did not carry sequence_number 0",
        });
    }
    if let Some(id) = frame
        .get("response")
        .and_then(|response| response.get("id"))
        .and_then(Value::as_str)
    {
        state.request_id = Some(ProviderRequestId::truncating(id));
    }
    Ok(state.mark_started())
}

/// An output item opened.
fn on_item_added(
    state: &mut DialectState,
    frame: &Value,
    budget: &StreamBudget,
) -> Result<FrameOutcome, FrameDecodeError> {
    let index = output_index(frame)?;
    let item = frame
        .get("item")
        .ok_or(FrameDecodeError::MalformedField { field: "item" })?;
    match text_of(item, "type")? {
        "message" | "reasoning" => state.ledger.open_block(budget)?,
        "function_call" => {
            state.ledger.open_block(budget)?;
            state.ledger.open_tool_call(budget)?;
            let call = ToolCallId::new(text_of(item, "call_id")?).map_err(|_| {
                FrameDecodeError::MalformedField {
                    field: "item.call_id",
                }
            })?;
            state.open_tools.insert(
                index,
                PartialToolCall {
                    id: Some(call),
                    name: Some(text_of(item, "name")?.to_owned()),
                    arguments: String::new(),
                },
            );
        }
        other => {
            return Err(FrameDecodeError::UnknownEvent {
                event: BoundedString::truncating(other),
            });
        }
    }
    Ok(FrameOutcome::Progress)
}

/// An output item closed.
fn on_item_done(state: &mut DialectState, frame: &Value) -> Result<FrameOutcome, FrameDecodeError> {
    let index = output_index(frame)?;
    let item = frame
        .get("item")
        .ok_or(FrameDecodeError::MalformedField { field: "item" })?;
    match text_of(item, "type")? {
        "message" => {
            let unflushed = state.open_text.remove(&index).is_some()
                || state
                    .open_text
                    .remove(&(ALTERNATE_KEY_BASE + index))
                    .is_some();
            if unflushed {
                return Err(FrameDecodeError::OutOfOrder {
                    reason: "a message item closed with unflushed content",
                });
            }
            Ok(FrameOutcome::Progress)
        }
        "reasoning" => finish_reasoning(state, index, item),
        "function_call" => finish_tool_call(state, index, item),
        other => Err(FrameDecodeError::UnknownEvent {
            event: BoundedString::truncating(other),
        }),
    }
}

/// Emits the reasoning block, which is where the encrypted material arrives.
fn finish_reasoning(
    state: &mut DialectState,
    index: u16,
    item: &Value,
) -> Result<FrameOutcome, FrameDecodeError> {
    let text = state.open_reasoning.remove(&index);
    let summary = state.open_reasoning.remove(&(ALTERNATE_KEY_BASE + index));
    let body = match (text, summary) {
        (Some(text), _) => ReasoningBody::Text {
            text: bounded_reason(&text)?,
        },
        (None, Some(summary)) => ReasoningBody::Summary {
            text: bounded_reason(&summary)?,
        },
        (None, None) => ReasoningBody::Redacted,
    };
    let token = item
        .get("encrypted_content")
        .and_then(Value::as_str)
        .map(|encrypted| ReasoningToken {
            provenance: ProviderId::Openai,
            bytes: Bytes::copy_from_slice(encrypted.as_bytes()),
        });
    state
        .blocks
        .push(CanonicalBlock::Reasoning(ReasoningBlock { body, token }));
    Ok(FrameOutcome::Progress)
}

/// Emits the tool-use block from the reassembled fragments.
fn finish_tool_call(
    state: &mut DialectState,
    index: u16,
    item: &Value,
) -> Result<FrameOutcome, FrameDecodeError> {
    let partial = state
        .open_tools
        .remove(&index)
        .ok_or(FrameDecodeError::OutOfOrder {
            reason: "a function_call item closed without opening",
        })?;
    let id = partial.id.ok_or(FrameDecodeError::MalformedField {
        field: "item.call_id",
    })?;
    let name = partial
        .name
        .ok_or(FrameDecodeError::MalformedField { field: "item.name" })?;

    let arguments = match item.get("arguments").and_then(Value::as_str) {
        Some(closing) if partial.arguments.is_empty() => closing.to_owned(),
        Some(closing) if partial.arguments != closing => {
            return Err(FrameDecodeError::OutOfOrder {
                reason: "the reassembled tool arguments do not match the closing item",
            });
        }
        _ => partial.arguments,
    };

    let parsed: Value = serde_json::from_str(&arguments)
        .map_err(|_| FrameDecodeError::ToolArgumentsNotJson { call: id.clone() })?;
    let input = CanonicalJson::from_value(&parsed)
        .map_err(|_| FrameDecodeError::ToolArgumentsNotJson { call: id.clone() })?;
    let name = ToolName::parse(&name)
        .map_err(|_| FrameDecodeError::MalformedField { field: "item.name" })?;

    state
        .blocks
        .push(CanonicalBlock::ToolUse { id, name, input });
    Ok(FrameOutcome::Progress)
}

/// Assistant text arrived.
fn on_text_delta(
    state: &mut DialectState,
    frame: &Value,
    budget: &StreamBudget,
) -> Result<FrameOutcome, FrameDecodeError> {
    let index = output_index(frame)?;
    let delta = text_of(frame, "delta")?;
    state
        .ledger
        .charge_text(budget, u64::try_from(delta.len()).unwrap_or(u64::MAX))?;
    state.open_text.entry(index).or_default().push_str(delta);
    Ok(FrameOutcome::Progress)
}

/// Assistant text closed.
fn on_text_done(state: &mut DialectState, frame: &Value) -> Result<FrameOutcome, FrameDecodeError> {
    let index = output_index(frame)?;
    let closing = text_of(frame, "text")?;
    let accumulated = state.open_text.remove(&index).unwrap_or_default();
    if !accumulated.is_empty() && accumulated != closing {
        return Err(FrameDecodeError::OutOfOrder {
            reason: "the reassembled output text does not match the closing text",
        });
    }
    let text = if accumulated.is_empty() {
        closing.to_owned()
    } else {
        accumulated
    };
    state.blocks.push(CanonicalBlock::Text {
        text: BoundedString::new(text).map_err(|_| {
            FrameDecodeError::Budget(BudgetOverrun::Text {
                limit: u64::try_from(TEXT_MAX).unwrap_or(u64::MAX),
            })
        })?,
        annotations: Vec::new(),
    });
    Ok(FrameOutcome::Progress)
}

/// Refusal text arrived.
fn on_refusal_delta(
    state: &mut DialectState,
    frame: &Value,
    budget: &StreamBudget,
) -> Result<FrameOutcome, FrameDecodeError> {
    let index = output_index(frame)?;
    let delta = text_of(frame, "delta")?;
    state
        .ledger
        .charge_text(budget, u64::try_from(delta.len()).unwrap_or(u64::MAX))?;
    state
        .open_text
        .entry(ALTERNATE_KEY_BASE + index)
        .or_default()
        .push_str(delta);
    Ok(FrameOutcome::Progress)
}

/// Refusal text closed.
fn on_refusal_done(
    state: &mut DialectState,
    frame: &Value,
) -> Result<FrameOutcome, FrameDecodeError> {
    let index = output_index(frame)?;
    let closing = text_of(frame, "refusal")?;
    let accumulated = state
        .open_text
        .remove(&(ALTERNATE_KEY_BASE + index))
        .unwrap_or_default();
    if !accumulated.is_empty() && accumulated != closing {
        return Err(FrameDecodeError::OutOfOrder {
            reason: "the reassembled refusal does not match the closing refusal",
        });
    }
    let text = if accumulated.is_empty() {
        closing.to_owned()
    } else {
        accumulated
    };
    state.blocks.push(CanonicalBlock::Refusal {
        text: BoundedString::new(text).map_err(|_| {
            FrameDecodeError::Budget(BudgetOverrun::Text {
                limit: REFUSAL_BOUND,
            })
        })?,
    });
    Ok(FrameOutcome::Progress)
}

/// A fragment of tool arguments arrived.
fn on_arguments_delta(
    state: &mut DialectState,
    frame: &Value,
    budget: &StreamBudget,
) -> Result<FrameOutcome, FrameDecodeError> {
    let index = output_index(frame)?;
    let delta = text_of(frame, "delta")?;
    let call = state
        .open_tools
        .get_mut(&index)
        .ok_or(FrameDecodeError::OutOfOrder {
            reason: "tool arguments arrived for an item that never opened",
        })?;
    call.arguments.push_str(delta);
    let id = call
        .id
        .clone()
        .unwrap_or_else(|| ToolCallId::truncating("unknown"));
    BudgetLedger::check_tool_arguments(budget, &id, call.arguments.len())?;
    Ok(FrameOutcome::Progress)
}

/// The closing tool-argument frame, which restates the whole `JSON` string.
fn on_arguments_done(
    state: &mut DialectState,
    frame: &Value,
) -> Result<FrameOutcome, FrameDecodeError> {
    let index = output_index(frame)?;
    let closing = text_of(frame, "arguments")?;
    let call = state
        .open_tools
        .get_mut(&index)
        .ok_or(FrameDecodeError::OutOfOrder {
            reason: "tool arguments closed for an item that never opened",
        })?;
    if call.arguments.is_empty() {
        call.arguments.push_str(closing);
    } else if call.arguments != closing {
        return Err(FrameDecodeError::OutOfOrder {
            reason: "the reassembled tool arguments do not match the closing arguments",
        });
    }
    Ok(FrameOutcome::Progress)
}

/// Reasoning text or reasoning summary text arrived.
fn on_reasoning_delta(
    state: &mut DialectState,
    frame: &Value,
    budget: &StreamBudget,
    summary: bool,
) -> Result<FrameOutcome, FrameDecodeError> {
    let index = output_index(frame)?;
    let delta = text_of(frame, "delta")?;
    state
        .ledger
        .charge_reasoning(budget, u64::try_from(delta.len()).unwrap_or(u64::MAX))?;
    let key = if summary {
        ALTERNATE_KEY_BASE + index
    } else {
        index
    };
    state.open_reasoning.entry(key).or_default().push_str(delta);
    Ok(FrameOutcome::Progress)
}

/// Reasoning text or reasoning summary text closed.
///
/// The block itself is emitted at `response.output_item.done`, because that is
/// where `encrypted_content` arrives and a reasoning block carries both.
fn on_reasoning_done(
    state: &mut DialectState,
    frame: &Value,
    summary: bool,
) -> Result<FrameOutcome, FrameDecodeError> {
    let index = output_index(frame)?;
    let closing = text_of(frame, "text")?;
    let key = if summary {
        ALTERNATE_KEY_BASE + index
    } else {
        index
    };
    match state.open_reasoning.get(&key) {
        Some(accumulated) if accumulated != closing => {
            return Err(FrameDecodeError::OutOfOrder {
                reason: "the reassembled reasoning does not match the closing text",
            });
        }
        Some(_) => {}
        None => {
            state.open_reasoning.insert(key, closing.to_owned());
        }
    }
    Ok(FrameOutcome::Progress)
}

/// A terminal lifecycle frame.
fn on_terminal(state: &mut DialectState, frame: &Value) -> Result<FrameOutcome, FrameDecodeError> {
    let response = frame
        .get("response")
        .ok_or(FrameDecodeError::MalformedField { field: "response" })?;
    state.terminal = true;
    state.usage = map_usage(response.get("usage"));

    match text_of(response, "status")? {
        "completed" => {
            state.finish_token = Some(FINISH_COMPLETED.to_owned());
            Ok(FrameOutcome::Terminal)
        }
        "incomplete" => on_incomplete(state, response),
        "failed" => Ok(FrameOutcome::Failed(Box::new(failure_from(
            response.get("error"),
            ProviderFailureKind::ServerError,
        )))),
        "cancelled" => Ok(FrameOutcome::Failed(Box::new(ProviderFailure::new(
            RedactedDetail::internal(
                ProviderFailureKind::Cancelled,
                "the provider reported the response as cancelled",
            ),
        )))),
        _ => Err(FrameDecodeError::MalformedField {
            field: "response.status",
        }),
    }
}

/// `status: incomplete`, whose reason decides stop versus failure.
fn on_incomplete(
    state: &mut DialectState,
    response: &Value,
) -> Result<FrameOutcome, FrameDecodeError> {
    let reason = response
        .get("incomplete_details")
        .and_then(|details| details.get("reason"))
        .and_then(Value::as_str);
    match reason {
        Some("max_output_tokens") => {
            state.finish_token = Some(FINISH_MAX_OUTPUT_TOKENS.to_owned());
            Ok(FrameOutcome::Terminal)
        }
        // A content block is a failure, never a stop reason (D-09).
        Some("content_filter") => Ok(FrameOutcome::Failed(Box::new(ProviderFailure::new(
            RedactedDetail::internal(
                ProviderFailureKind::ContentFiltered,
                "the provider blocked the generation on content grounds",
            )
            .with_code("content_filter"),
        )))),
        _ => Err(FrameDecodeError::MalformedField {
            field: "response.incomplete_details.reason",
        }),
    }
}

/// An in-stream `error` frame, which §6.1 makes a definitive terminal error.
fn on_error(state: &mut DialectState, frame: &Value) -> FrameOutcome {
    state.terminal = true;
    FrameOutcome::Failed(Box::new(failure_from(
        Some(frame),
        ProviderFailureKind::ServerError,
    )))
}

/// Builds a failure from a `{code, message}` object, redacting the message.
fn failure_from(error: Option<&Value>, fallback: ProviderFailureKind) -> ProviderFailure {
    let code = error
        .and_then(|value| value.get("code"))
        .and_then(Value::as_str);
    let kind = code.and_then(code_kind).unwrap_or(fallback);
    let message = error
        .and_then(|value| value.get("message"))
        .and_then(Value::as_str);
    let mut detail = match message {
        Some(text) => RedactedDetail::new(kind, redact(text, &[])),
        None => RedactedDetail::internal(kind, "the provider reported an error with no message"),
    };
    if let Some(code) = code {
        detail = detail.with_code(code);
    }
    ProviderFailure::new(detail)
}

/// Maps `response.usage` onto the neutral tally.
///
/// `input_tokens` is carried through unmodified: the entry declares
/// `CacheReadSemantics::SeparateReadWrite`, so the cached counters sit beside
/// the prompt count rather than inside it. Nothing here is invented — an absent
/// member is recorded on [`UsageCompleteness`].
fn map_usage(usage: Option<&Value>) -> NormalizedUsage {
    let Some(usage) = usage else {
        return NormalizedUsage {
            completeness: UsageCompleteness::Absent,
            ..NormalizedUsage::default()
        };
    };
    let input_details = usage.get("input_tokens_details");
    let output_details = usage.get("output_tokens_details");

    let input = usage.get("input_tokens").and_then(Value::as_u64);
    let cached = input_details
        .and_then(|details| details.get("cached_tokens"))
        .and_then(Value::as_u64);
    let cache_write = input_details
        .and_then(|details| details.get("cache_write_tokens"))
        .and_then(Value::as_u64);
    let output = usage.get("output_tokens").and_then(Value::as_u64);
    let reasoning = output_details
        .and_then(|details| details.get("reasoning_tokens"))
        .and_then(Value::as_u64);
    let total = usage.get("total_tokens").and_then(Value::as_u64);

    let mut missing = UsageFieldSet::EMPTY;
    for (present, field) in [
        (input.is_some(), UsageField::InputTokens),
        (cached.is_some(), UsageField::CacheReadInputTokens),
        (cache_write.is_some(), UsageField::CacheWriteInputTokens),
        (output.is_some(), UsageField::OutputTokens),
        (reasoning.is_some(), UsageField::ReasoningTokens),
        (total.is_some(), UsageField::ProviderTotalTokens),
    ] {
        if !present {
            missing = missing.with(field);
        }
    }

    NormalizedUsage {
        input_tokens: input.unwrap_or_default(),
        cache_read_input_tokens: cached.unwrap_or_default(),
        cache_write_input_tokens: cache_write.unwrap_or_default(),
        // `reasoning_included_in_output` is true for this provider, so the
        // output count is carried through and reasoning is its subset.
        output_tokens: output.unwrap_or_default(),
        reasoning_tokens: reasoning.unwrap_or_default(),
        tool_use_prompt_tokens: 0,
        provider_total_tokens: total,
        completeness: if missing.is_empty() {
            UsageCompleteness::Exact
        } else {
            UsageCompleteness::Partial { missing }
        },
    }
}

// ---------------------------------------------------------------------------
// small readers
// ---------------------------------------------------------------------------

/// A required string member.
fn text_of<'a>(value: &'a Value, field: &'static str) -> Result<&'a str, FrameDecodeError> {
    value
        .get(field)
        .and_then(Value::as_str)
        .ok_or(FrameDecodeError::MalformedField { field })
}

/// A required unsigned-integer member.
fn number_of(value: &Value, field: &'static str) -> Result<u64, FrameDecodeError> {
    value
        .get(field)
        .and_then(Value::as_u64)
        .ok_or(FrameDecodeError::MalformedField { field })
}

/// The `output_index`, bounded so the alternate key space cannot collide.
fn output_index(frame: &Value) -> Result<u16, FrameDecodeError> {
    let raw = number_of(frame, "output_index")?;
    let index = u16::try_from(raw).map_err(|_| FrameDecodeError::MalformedField {
        field: "output_index",
    })?;
    if index >= ALTERNATE_KEY_BASE {
        return Err(FrameDecodeError::MalformedField {
            field: "output_index",
        });
    }
    Ok(index)
}

/// Bounds reasoning text to the canonical block bound.
fn bounded_reason(text: &str) -> Result<BoundedString<REASON_MAX>, FrameDecodeError> {
    BoundedString::new(text).map_err(|_| {
        FrameDecodeError::Budget(BudgetOverrun::Reasoning {
            limit: u64::try_from(REASON_MAX).unwrap_or(u64::MAX),
        })
    })
}

/// The documented `error.code` table.
fn code_kind(code: &str) -> Option<ProviderFailureKind> {
    let kind = match code {
        "credit_balance_exhausted" | "billing_hard_limit_reached" => ProviderFailureKind::Billing,
        "organization_spend_limit_exceeded" | "insufficient_quota" => ProviderFailureKind::Quota,
        "rate_limit_exceeded" => ProviderFailureKind::RateLimited,
        "previous_response_not_found" | "invalid_prompt" => ProviderFailureKind::InvalidRequest,
        "context_length_exceeded" => ProviderFailureKind::ContextOverflow,
        "model_not_found" => ProviderFailureKind::ModelNotFound,
        "invalid_api_key" | "unsupported_country_region_territory" => {
            ProviderFailureKind::Authentication
        }
        "content_filter" | "content_policy_violation" => ProviderFailureKind::ContentFiltered,
        "server_error" => ProviderFailureKind::ServerError,
        _ => return None,
    };
    Some(kind)
}

/// The documented `error.type` table, consulted only when `code` says nothing.
fn type_kind(kind: &str) -> Option<ProviderFailureKind> {
    let kind = match kind {
        "invalid_request_error" => ProviderFailureKind::InvalidRequest,
        "authentication_error" | "permission_error" => ProviderFailureKind::Authentication,
        "rate_limit_error" => ProviderFailureKind::RateLimited,
        "insufficient_quota" => ProviderFailureKind::Quota,
        "server_error" | "api_error" => ProviderFailureKind::ServerError,
        _ => return None,
    };
    Some(kind)
}

/// The documented status table, consulted last.
const fn status_kind(status: u16) -> ProviderFailureKind {
    match status {
        400 | 409 | 413 | 422 => ProviderFailureKind::InvalidRequest,
        401 | 403 => ProviderFailureKind::Authentication,
        404 => ProviderFailureKind::ModelNotFound,
        408 => ProviderFailureKind::Timeout,
        429 => ProviderFailureKind::RateLimited,
        503 => ProviderFailureKind::Overloaded,
        _ => ProviderFailureKind::ServerError,
    }
}

#[cfg(test)]
mod tests {
    use aex_model_catalog::canonical::{
        CacheBreakpoint, CanonicalMessage, CanonicalToolDef, CorrelationId, SystemBlock,
    };
    use aex_model_catalog::catalog::Catalog;
    use aex_model_catalog::document::{
        CacheMode, CachePolicy, CacheReadSemantics, CatalogDocument, ModelEntry, ReasoningEncoding,
        ReasoningPolicy, ReasoningReplay,
    };
    use aex_model_catalog::fixture;
    use aex_model_catalog::signature::{
        CatalogEnvelope, CatalogSignature, SigAlg, SigningKeyId, TrustedKey, TrustedKeys,
    };
    use aex_wire::ContentHash;
    use aex_wire::provider::ModelSelection;
    use reqwest::header::HeaderMap;

    use super::{
        BoundedBody, BoundedString, CanonicalBlock, CanonicalJson, CanonicalModelRequest,
        Capability, CapabilitySet, DialectState, FrameDecodeError, FrameOutcome, HeaderView,
        OpenAiAdapter, ProviderAdapter, ProviderFailureKind, ProviderId, QualifiedModel,
        RateLimitSource, ReasoningBlock, ReasoningBody, ReasoningEffort, ReasoningMode,
        ReasoningRequest, ReasoningToken, RequestBuildError, Role, SamplingSupport, SchemaEncoding,
        SseEvent, StopReason, StreamBudget, StructuredOutputPolicy, StructuredOutputRequest,
        ToolChoice, ToolEncoding, ToolName, ToolResultPart, UsageCompleteness, UsageField,
        UsageFieldSet,
    };

    // -----------------------------------------------------------------------
    // the pinned catalog
    //
    // A `QualifiedModel` exists only inside a loaded, signed catalog, and this
    // crate declares no signing dependency, so the key and the detached
    // signature below are pinned constants over the exact canonical bytes of
    // `test_catalog_document()`. Change that document and the pair must be
    // reminted.
    // -----------------------------------------------------------------------

    /// The publisher the pinned signature was minted for.
    const PUBLISHER: &str = "aex-catalog-test";
    /// The instant the pinned test catalog is valid from.
    const NOW_MS: i64 = 1_800_000_000_000;

    /// Every capability, every policy this dialect can honour.
    const MODEL_FULL: &str = "gpt-5.2";
    /// Text only: no tools, no reasoning, no sampling, no structured output.
    const MODEL_MINIMAL: &str = "gpt-5.2-minimal";
    /// Structured output limited to the legacy `json_object` form.
    const MODEL_JSON_OBJECT: &str = "gpt-5.2-json-object";
    /// One tool and a 64-token output ceiling.
    const MODEL_NARROW: &str = "gpt-5.2-narrow";
    /// An eight-token context window.
    const MODEL_TINY_CONTEXT: &str = "gpt-5.2-tiny-context";
    /// A 256-byte request-body bound.
    const MODEL_TINY_BODY: &str = "gpt-5.2-tiny-body";
    /// Reasoning that forbids sampling and requires round-trip material.
    const MODEL_THINKING: &str = "gpt-5.2-thinking";
    /// An entry declaring the chat-completions tool encoding.
    const MODEL_NESTED_TOOLS: &str = "gpt-5.2-nested-tools";
    /// Tools with no `tool_choice` mode declared.
    const MODEL_TOOLS_ONLY: &str = "gpt-5.2-tools-only";
    /// The pair that proves the adapter refuses another provider's model.
    const MODEL_ANTHROPIC: &str = "claude-opus-5";

    fn base_capabilities() -> CapabilitySet {
        CapabilitySet::from_slice(&[
            Capability::TextIn,
            Capability::TextOut,
            Capability::Streaming,
        ])
    }

    fn full_capabilities() -> CapabilitySet {
        CapabilitySet::from_slice(&[
            Capability::TextIn,
            Capability::TextOut,
            Capability::Streaming,
            Capability::SystemInstruction,
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
            Capability::Temperature,
            Capability::TopP,
        ])
    }

    fn tools_only_capabilities() -> CapabilitySet {
        CapabilitySet::from_slice(&[
            Capability::TextIn,
            Capability::TextOut,
            Capability::Streaming,
            Capability::SystemInstruction,
            Capability::Tools,
        ])
    }

    fn json_object_capabilities() -> CapabilitySet {
        CapabilitySet::from_slice(&[
            Capability::TextIn,
            Capability::TextOut,
            Capability::Streaming,
            Capability::StructuredOutput,
        ])
    }

    fn openai_entry(model: &str, capabilities: CapabilitySet) -> ModelEntry {
        let mut entry = fixture::entry(ProviderId::Openai, model, capabilities);
        entry.tool_policy.encoding = ToolEncoding::OpenAiFlat;
        entry.sampling = SamplingSupport::Full;
        entry.reasoning = ReasoningPolicy {
            mode: ReasoningMode::Optional,
            encoding: ReasoningEncoding::OpenAiReasoning,
            replay: ReasoningReplay::RecommendedEcho,
            excludes_sampling: false,
        };
        entry.structured_output = StructuredOutputPolicy::JsonSchema {
            encoding: SchemaEncoding::OpenAiTextFormat,
            strict_default: true,
        };
        entry.cache_policy = CachePolicy {
            mode: CacheMode::Implicit,
            max_breakpoints: 0,
        };
        entry.usage_map.cache_read_field = CacheReadSemantics::SeparateReadWrite;
        entry.limits.min_output_tokens = 16;
        entry.limits.temperature_milli = Some((0, 2_000));
        entry.limits.top_p_milli = Some((0, 1_000));
        entry
    }

    fn test_catalog_document() -> CatalogDocument {
        let mut minimal = openai_entry(MODEL_MINIMAL, base_capabilities());
        minimal.sampling = SamplingSupport::None;
        minimal.structured_output = StructuredOutputPolicy::Unsupported;
        minimal.reasoning.mode = ReasoningMode::Unsupported;
        minimal.reasoning.encoding = ReasoningEncoding::None;

        let mut json_object = openai_entry(MODEL_JSON_OBJECT, json_object_capabilities());
        json_object.structured_output = StructuredOutputPolicy::JsonObjectOnly;

        let mut narrow = openai_entry(MODEL_NARROW, full_capabilities());
        narrow.limits.max_tools = 1;
        narrow.limits.max_output_tokens = 64;

        let mut tiny_context = openai_entry(MODEL_TINY_CONTEXT, full_capabilities());
        tiny_context.limits.context_window_tokens = 8;

        let mut tiny_body = openai_entry(MODEL_TINY_BODY, full_capabilities());
        tiny_body.limits.request_body_max_bytes = 256;

        let mut thinking = openai_entry(MODEL_THINKING, full_capabilities());
        thinking.reasoning.excludes_sampling = true;
        thinking.reasoning.replay = ReasoningReplay::RequiredAlways;

        let mut nested = openai_entry(MODEL_NESTED_TOOLS, full_capabilities());
        nested.tool_policy.encoding = ToolEncoding::OpenAiNestedFunction;

        let entries = vec![
            openai_entry(MODEL_FULL, full_capabilities()),
            minimal,
            json_object,
            narrow,
            tiny_context,
            tiny_body,
            thinking,
            nested,
            openai_entry(MODEL_TOOLS_ONLY, tools_only_capabilities()),
            fixture::entry(ProviderId::Anthropic, MODEL_ANTHROPIC, base_capabilities()),
        ];
        fixture::document(
            PUBLISHER,
            1,
            entries,
            fixture::at(NOW_MS),
            fixture::adapter("fixture-adapter"),
        )
    }

    /// The public key the pinned signature was minted with.
    const PINNED_PUBLIC_KEY: [u8; 65] = [
        0x04, 0x76, 0xe1, 0xe8, 0x5f, 0xfa, 0x5c, 0x68, 0x24, 0xf5, 0xfd, 0x3a, 0xb5, 0x1a, 0x3d,
        0x84, 0x52, 0x97, 0x3c, 0x31, 0xef, 0x88, 0x54, 0x29, 0xb9, 0xe4, 0x01, 0x55, 0xb5, 0x96,
        0xc1, 0xd0, 0x7e, 0xae, 0x65, 0x28, 0xbc, 0x6d, 0x02, 0x31, 0xef, 0xb1, 0x6e, 0x26, 0x71,
        0xa9, 0x3e, 0x7b, 0xab, 0x82, 0xf3, 0x22, 0x5c, 0x1a, 0xe0, 0xf6, 0x84, 0xcc, 0xb8, 0x7d,
        0x52, 0x31, 0xe5, 0xce, 0x3d,
    ];

    /// The detached signature over `SIGNING_PREFIX` followed by the canonical
    /// bytes of [`test_catalog_document`].
    const PINNED_SIGNATURE: [u8; 71] = [
        0x30, 0x45, 0x02, 0x20, 0x12, 0xb1, 0xaa, 0xdd, 0xc5, 0x13, 0x0f, 0x19, 0x70, 0x88, 0x8e,
        0xe4, 0x2d, 0x19, 0xa1, 0xf4, 0x08, 0x7d, 0x82, 0x05, 0xfc, 0x6c, 0x19, 0x97, 0x20, 0xcc,
        0xca, 0xc6, 0x94, 0x80, 0x65, 0xee, 0x02, 0x21, 0x00, 0x88, 0xe2, 0x8e, 0x23, 0xd4, 0x6f,
        0xe6, 0xdb, 0x98, 0xae, 0x66, 0x82, 0x44, 0x61, 0x9e, 0xe8, 0xc1, 0x73, 0xd3, 0x52, 0x14,
        0x87, 0xb8, 0x49, 0x61, 0x4f, 0xdb, 0x41, 0x5a, 0xda, 0xd1, 0xb2,
    ];

    static COMPILED_KEYS: [TrustedKey; 1] = [(PUBLISHER, PINNED_PUBLIC_KEY)];

    fn catalog() -> &'static Catalog {
        static LOADED: std::sync::OnceLock<Catalog> = std::sync::OnceLock::new();
        LOADED.get_or_init(|| {
            let envelope = CatalogEnvelope {
                document: fixture::canonical_bytes(&test_catalog_document()),
                signatures: vec![CatalogSignature {
                    key_id: SigningKeyId(fixture::bounded(PUBLISHER)),
                    algorithm: SigAlg::EcdsaP256Sha256Asn1,
                    bytes: bytes::Bytes::copy_from_slice(&PINNED_SIGNATURE),
                }],
            };
            Catalog::load(
                &envelope,
                &TrustedKeys::new(&COMPILED_KEYS),
                fixture::at(NOW_MS),
                None,
                fixture::adapter("fixture-adapter"),
            )
            .expect(
                "the pinned signature covers exactly these canonical document bytes; \
                 regenerate the key and signature when the document schema or the \
                 fixture defaults change",
            )
        })
    }

    fn pair(provider: ProviderId, model: &str) -> QualifiedModel {
        catalog()
            .qualified(&ModelSelection {
                credential_id: None,
                model: model.to_owned(),
                provider,
            })
            .expect("the fixture entry resolves")
    }

    fn openai(model: &str) -> QualifiedModel {
        pair(ProviderId::Openai, model)
    }

    // -----------------------------------------------------------------------
    // request fixtures
    // -----------------------------------------------------------------------

    fn user_text(text: &str) -> CanonicalMessage {
        CanonicalMessage {
            role: Role::User,
            blocks: vec![CanonicalBlock::Text {
                text: fixture::bounded(text),
                annotations: Vec::new(),
            }],
        }
    }

    fn base_request(model: &QualifiedModel) -> CanonicalModelRequest {
        CanonicalModelRequest {
            selection: model.clone(),
            system: Vec::new(),
            messages: vec![user_text("hello")],
            tools: Vec::new(),
            tool_choice: ToolChoice::Auto,
            parallel_tools: true,
            max_output_tokens: 1024,
            temperature_milli: None,
            top_p_milli: None,
            stop_sequences: Vec::new(),
            reasoning: ReasoningRequest::ProviderDefault,
            structured_output: None,
            cache_breakpoints: Vec::new(),
            correlation: CorrelationId::from_effect([0u8; 16]),
            request_hash: ContentHash::of(b"openai-adapter-test"),
        }
    }

    fn tool_def(name: &str, strict: bool) -> CanonicalToolDef {
        CanonicalToolDef {
            name: ToolName::parse(name).expect("a valid tool name"),
            description: fixture::bounded("Look a thing up."),
            input_schema: CanonicalJson::parse(r#"{"type":"object"}"#).expect("a valid schema"),
            strict,
        }
    }

    fn body_of(model: &QualifiedModel, request: &CanonicalModelRequest) -> String {
        let wire = OpenAiAdapter
            .build_request(model, request)
            .expect("the request builds");
        String::from_utf8(wire.body.to_vec()).expect("the body is UTF-8")
    }

    fn build_error(model: &QualifiedModel, request: &CanonicalModelRequest) -> RequestBuildError {
        OpenAiAdapter
            .build_request(model, request)
            .expect_err("the request must be refused before dispatch")
    }

    // -----------------------------------------------------------------------
    // stream fixtures
    // -----------------------------------------------------------------------

    const CREATED: &str = r#"{"type":"response.created","sequence_number":0,"response":{"id":"resp_abc","status":"in_progress"}}"#;
    const USAGE: &str = r#""usage":{"input_tokens":11,"input_tokens_details":{"cached_tokens":8,"cache_write_tokens":3},"output_tokens":5,"output_tokens_details":{"reasoning_tokens":2},"total_tokens":16}"#;

    fn completed(sequence: u32) -> String {
        format!(
            r#"{{"type":"response.completed","sequence_number":{sequence},"response":{{"id":"resp_abc","status":"completed",{USAGE}}}}}"#
        )
    }

    fn decode_named(
        state: &mut DialectState,
        name: Option<&str>,
        data: &str,
    ) -> Result<FrameOutcome, FrameDecodeError> {
        OpenAiAdapter.decode(
            state,
            &SseEvent {
                name,
                data: data.as_bytes(),
                id: None,
            },
            &StreamBudget::default(),
        )
    }

    fn decode(state: &mut DialectState, data: &str) -> Result<FrameOutcome, FrameDecodeError> {
        let value: serde_json::Value = serde_json::from_str(data).expect("a test frame is JSON");
        let name = value["type"]
            .as_str()
            .expect("a test frame carries a type")
            .to_owned();
        decode_named(state, Some(&name), data)
    }

    fn play(frames: &[&str]) -> (DialectState, Result<FrameOutcome, FrameDecodeError>) {
        let mut state = DialectState::new();
        let mut last = Ok(FrameOutcome::Ignored);
        for frame in frames {
            last = decode(&mut state, frame);
            if last.is_err() {
                break;
            }
        }
        (state, last)
    }

    fn classify(status: u16, body: &str) -> ProviderFailureKind {
        let headers = HeaderMap::new();
        OpenAiAdapter
            .classify_http(
                status,
                &HeaderView::new(&headers),
                &BoundedBody::new(body.as_bytes().to_vec(), false),
            )
            .kind()
    }

    // -----------------------------------------------------------------------
    // identity
    // -----------------------------------------------------------------------

    #[test]
    fn the_adapter_speaks_for_openai_and_names_its_source() {
        assert_eq!(OpenAiAdapter.provider(), ProviderId::Openai);
        // The digest is compiled, not derived at runtime, and is stable.
        assert_eq!(OpenAiAdapter.source_digest(), OpenAiAdapter.source_digest());
    }

    #[test]
    fn the_request_targets_the_pinned_responses_endpoint_with_bearer_auth() {
        let model = openai(MODEL_FULL);
        let wire = OpenAiAdapter
            .build_request(&model, &base_request(&model))
            .expect("the request builds");
        assert_eq!(
            wire.url().expect("assembles").as_str(),
            "https://api.openai.com/v1/responses"
        );
        assert_eq!(wire.auth, super::AuthScheme::BearerAuthorization);
        assert_eq!(wire.accept, super::Accept::TextEventStream);
        assert!(wire.query.is_empty(), "this dialect sends no query");
    }

    #[test]
    fn no_beta_or_version_header_is_sent() {
        // `OpenAI-Beta` is an Assistants header; Responses takes none.
        let model = openai(MODEL_FULL);
        let wire = OpenAiAdapter
            .build_request(&model, &base_request(&model))
            .expect("the request builds");
        let names: Vec<&str> = wire.headers.iter().map(|(name, _)| *name).collect();
        assert_eq!(names, vec!["content-type"]);
    }

    // -----------------------------------------------------------------------
    // request goldens
    // -----------------------------------------------------------------------

    #[test]
    fn a_text_request_serializes_to_the_pinned_body() {
        let model = openai(MODEL_FULL);
        let body = body_of(&model, &base_request(&model));
        assert_eq!(
            body,
            concat!(
                r#"{"model":"gpt-5.2","#,
                r#""input":[{"type":"message","role":"user","content":[{"type":"input_text","text":"hello"}]}],"#,
                r#""max_output_tokens":1024,"#,
                r#""include":["reasoning.encrypted_content"],"#,
                r#""store":false,"stream":true,"stream_options":{"include_obfuscation":false}}"#,
            )
        );
        assert!(
            !body.contains("\"stop\""),
            "this dialect has no stop member"
        );
        assert!(!body.contains("previous_response_id"));
        assert!(!body.contains("background"));
        assert!(!body.contains("conversation"));
    }

    #[test]
    fn a_tool_request_serializes_to_the_pinned_body() {
        let model = openai(MODEL_FULL);
        let mut request = base_request(&model);
        request.system = vec![SystemBlock {
            text: fixture::bounded("You are terse."),
            cacheable: false,
        }];
        request.tools = vec![tool_def("lookup", true)];
        request.tool_choice = ToolChoice::Named {
            name: ToolName::parse("lookup").expect("name"),
        };
        assert_eq!(
            body_of(&model, &request),
            concat!(
                r#"{"model":"gpt-5.2","#,
                r#""input":[{"type":"message","role":"user","content":[{"type":"input_text","text":"hello"}]}],"#,
                r#""instructions":"You are terse.","max_output_tokens":1024,"#,
                r#""tools":[{"type":"function","name":"lookup","description":"Look a thing up.","parameters":{"type":"object"},"strict":true}],"#,
                r#""tool_choice":{"type":"function","name":"lookup"},"parallel_tool_calls":true,"#,
                r#""include":["reasoning.encrypted_content"],"#,
                r#""store":false,"stream":true,"stream_options":{"include_obfuscation":false}}"#,
            )
        );
    }

    #[test]
    fn a_structured_output_request_serializes_to_the_pinned_body() {
        let model = openai(MODEL_FULL);
        let mut request = base_request(&model);
        request.structured_output = Some(StructuredOutputRequest::JsonSchema {
            name: ToolName::parse("answer").expect("name"),
            schema: CanonicalJson::parse(r#"{"type":"object"}"#).expect("schema"),
            strict: true,
        });
        assert_eq!(
            body_of(&model, &request),
            concat!(
                r#"{"model":"gpt-5.2","#,
                r#""input":[{"type":"message","role":"user","content":[{"type":"input_text","text":"hello"}]}],"#,
                r#""max_output_tokens":1024,"#,
                r#""text":{"format":{"type":"json_schema","name":"answer","schema":{"type":"object"},"strict":true}},"#,
                r#""include":["reasoning.encrypted_content"],"#,
                r#""store":false,"stream":true,"stream_options":{"include_obfuscation":false}}"#,
            )
        );
    }

    #[test]
    fn a_reasoning_request_serializes_to_the_pinned_body() {
        let model = openai(MODEL_FULL);
        let mut request = base_request(&model);
        request.reasoning = ReasoningRequest::Enabled {
            budget_tokens: None,
            effort: Some(ReasoningEffort::High),
        };
        request.temperature_milli = Some(700);
        request.top_p_milli = Some(950);
        assert_eq!(
            body_of(&model, &request),
            concat!(
                r#"{"model":"gpt-5.2","#,
                r#""input":[{"type":"message","role":"user","content":[{"type":"input_text","text":"hello"}]}],"#,
                r#""max_output_tokens":1024,"temperature":0.7,"top_p":0.95,"#,
                r#""reasoning":{"effort":"high","summary":"auto"},"#,
                r#""include":["reasoning.encrypted_content"],"#,
                r#""store":false,"stream":true,"stream_options":{"include_obfuscation":false}}"#,
            )
        );
    }

    #[test]
    fn a_json_object_request_uses_the_legacy_format() {
        let model = openai(MODEL_JSON_OBJECT);
        let mut request = base_request(&model);
        request.structured_output = Some(StructuredOutputRequest::JsonObject);
        let body = body_of(&model, &request);
        assert!(
            body.contains(r#""text":{"format":{"type":"json_object"}}"#),
            "{body}"
        );
        assert!(
            !body.contains(r#""include":"#),
            "a pair without reasoning replay asks for no replay material: {body}"
        );
    }

    #[test]
    fn a_tool_round_trip_serializes_call_and_output_items() {
        let model = openai(MODEL_FULL);
        let mut request = base_request(&model);
        request.tools = vec![tool_def("lookup", false)];
        request.messages.push(CanonicalMessage {
            role: Role::Assistant,
            blocks: vec![CanonicalBlock::ToolUse {
                id: fixture::bounded("call_abc"),
                name: ToolName::parse("lookup").expect("name"),
                input: CanonicalJson::parse(r#"{"q":"x"}"#).expect("input"),
            }],
        });
        request.messages.push(CanonicalMessage {
            role: Role::User,
            blocks: vec![CanonicalBlock::ToolResult {
                call: fixture::bounded("call_abc"),
                content: vec![ToolResultPart::Text {
                    text: fixture::bounded("42"),
                }],
                is_error: false,
            }],
        });
        let body = body_of(&model, &request);
        assert!(
            body.contains(
                r#"{"type":"function_call","call_id":"call_abc","name":"lookup","arguments":"{\"q\":\"x\"}"}"#
            ),
            "{body}"
        );
        assert!(
            body.contains(r#"{"type":"function_call_output","call_id":"call_abc","output":"42"}"#),
            "{body}"
        );
    }

    #[test]
    fn a_failed_tool_result_is_marked_rather_than_dropped() {
        let model = openai(MODEL_FULL);
        let mut request = base_request(&model);
        request.messages.push(CanonicalMessage {
            role: Role::User,
            blocks: vec![CanonicalBlock::ToolResult {
                call: fixture::bounded("call_abc"),
                content: vec![ToolResultPart::Text {
                    text: fixture::bounded("boom"),
                }],
                is_error: true,
            }],
        });
        assert!(
            body_of(&model, &request).contains(r#""output":"error: boom""#),
            "the error flag must survive into the one member this dialect has"
        );
    }

    #[test]
    fn reasoning_replay_material_is_sent_as_a_reasoning_item() {
        let model = openai(MODEL_FULL);
        let mut request = base_request(&model);
        request.messages.push(CanonicalMessage {
            role: Role::Assistant,
            blocks: vec![CanonicalBlock::Reasoning(ReasoningBlock {
                body: ReasoningBody::Redacted,
                token: Some(ReasoningToken {
                    provenance: ProviderId::Openai,
                    bytes: bytes::Bytes::from_static(b"gAAAA"),
                }),
            })],
        });
        assert!(
            body_of(&model, &request)
                .contains(r#"{"type":"reasoning","encrypted_content":"gAAAA","summary":[]}"#),
        );
    }

    #[test]
    fn an_assistant_refusal_replays_as_a_refusal_part() {
        let model = openai(MODEL_FULL);
        let mut request = base_request(&model);
        request.messages.push(CanonicalMessage {
            role: Role::Assistant,
            blocks: vec![CanonicalBlock::Refusal {
                text: fixture::bounded("I cannot help with that"),
            }],
        });
        assert!(
            body_of(&model, &request)
                .contains(r#"{"type":"refusal","refusal":"I cannot help with that"}"#),
        );
    }

    #[test]
    fn disabling_reasoning_sends_the_none_effort_and_asks_for_no_replay_material() {
        let model = openai(MODEL_FULL);
        let mut request = base_request(&model);
        request.reasoning = ReasoningRequest::Disabled;
        let body = body_of(&model, &request);
        assert!(body.contains(r#""reasoning":{"effort":"none"}"#), "{body}");
        assert!(!body.contains(r#""include":"#), "{body}");
    }

    // -----------------------------------------------------------------------
    // the capability gate
    // -----------------------------------------------------------------------

    #[test]
    fn stop_sequences_are_refused_before_dispatch() {
        // The whole point of the openai row: there is no member to put them in.
        let model = openai(MODEL_FULL);
        let mut request = base_request(&model);
        request.stop_sequences = vec![fixture::bounded("STOP")];
        assert_eq!(
            build_error(&model, &request),
            RequestBuildError::StopSequencesUnsupported
        );
    }

    #[test]
    fn an_undeclared_capability_is_refused_before_dispatch() {
        let model = openai(MODEL_MINIMAL);

        let mut tools = base_request(&model);
        tools.tools = vec![tool_def("lookup", false)];
        assert_eq!(
            build_error(&model, &tools),
            RequestBuildError::CapabilityUnavailable {
                capability: Capability::Tools
            }
        );

        let mut system = base_request(&model);
        system.system = vec![SystemBlock {
            text: fixture::bounded("be terse"),
            cacheable: false,
        }];
        assert_eq!(
            build_error(&model, &system),
            RequestBuildError::CapabilityUnavailable {
                capability: Capability::SystemInstruction
            }
        );

        let mut schema = base_request(&model);
        schema.structured_output = Some(StructuredOutputRequest::JsonObject);
        assert_eq!(
            build_error(&model, &schema),
            RequestBuildError::CapabilityUnavailable {
                capability: Capability::StructuredOutput
            }
        );

        let mut reasoning = base_request(&model);
        reasoning.reasoning = ReasoningRequest::Enabled {
            budget_tokens: None,
            effort: Some(ReasoningEffort::Low),
        };
        assert_eq!(
            build_error(&model, &reasoning),
            RequestBuildError::CapabilityUnavailable {
                capability: Capability::Reasoning
            }
        );
    }

    #[test]
    fn explicit_cache_breakpoints_are_refused_on_an_implicitly_cached_pair() {
        let model = openai(MODEL_FULL);
        let mut request = base_request(&model);
        request.cache_breakpoints = vec![CacheBreakpoint::AfterSystem];
        assert_eq!(
            build_error(&model, &request),
            RequestBuildError::CapabilityUnavailable {
                capability: Capability::PromptCacheExplicit
            }
        );
    }

    #[test]
    fn an_unsupported_tool_choice_is_refused_before_dispatch() {
        let model = openai(MODEL_TOOLS_ONLY);
        for choice in [
            ToolChoice::Required,
            ToolChoice::None,
            ToolChoice::Named {
                name: ToolName::parse("lookup").expect("name"),
            },
        ] {
            let mut request = base_request(&model);
            request.tools = vec![tool_def("lookup", false)];
            request.tool_choice = choice.clone();
            assert_eq!(
                build_error(&model, &request),
                RequestBuildError::ToolChoiceUnsupported {
                    requested: Box::new(choice)
                }
            );
        }
    }

    #[test]
    fn a_named_tool_that_was_never_declared_is_refused() {
        let model = openai(MODEL_FULL);
        let mut request = base_request(&model);
        request.tools = vec![tool_def("lookup", false)];
        request.tool_choice = ToolChoice::Named {
            name: ToolName::parse("other").expect("name"),
        };
        assert!(matches!(
            build_error(&model, &request),
            RequestBuildError::ToolChoiceUnsupported { .. }
        ));
    }

    #[test]
    fn a_tool_choice_without_tools_is_refused() {
        let model = openai(MODEL_FULL);
        let mut request = base_request(&model);
        request.tool_choice = ToolChoice::Required;
        assert!(matches!(
            build_error(&model, &request),
            RequestBuildError::ToolChoiceUnsupported { .. }
        ));
    }

    #[test]
    fn an_unsupported_structured_output_form_is_refused_before_dispatch() {
        let model = openai(MODEL_JSON_OBJECT);
        let mut request = base_request(&model);
        let requested = StructuredOutputRequest::JsonSchema {
            name: ToolName::parse("answer").expect("name"),
            schema: CanonicalJson::parse(r#"{"type":"object"}"#).expect("schema"),
            strict: true,
        };
        request.structured_output = Some(requested.clone());
        assert_eq!(
            build_error(&model, &request),
            RequestBuildError::StructuredOutputUnsupported {
                requested: Box::new(requested)
            }
        );
    }

    #[test]
    fn an_output_ceiling_outside_the_declared_range_is_refused() {
        let model = openai(MODEL_NARROW);
        for ceiling in [15u32, 65u32] {
            let mut request = base_request(&model);
            request.max_output_tokens = ceiling;
            assert_eq!(
                build_error(&model, &request),
                RequestBuildError::OutputTokensOutOfRange { min: 16, max: 64 }
            );
        }
    }

    #[test]
    fn more_tools_than_the_pair_declares_is_refused() {
        let model = openai(MODEL_NARROW);
        let mut request = base_request(&model);
        request.max_output_tokens = 32;
        request.tools = vec![tool_def("one", false), tool_def("two", false)];
        assert_eq!(
            build_error(&model, &request),
            RequestBuildError::ToolLimit { max: 1 }
        );
    }

    #[test]
    fn a_tool_name_outside_the_provider_grammar_is_refused() {
        let model = openai(MODEL_FULL);
        let mut request = base_request(&model);
        // The workspace grammar accepts a dot; the openai function-name
        // grammar does not, so the narrowing must fire here.
        request.tools = vec![tool_def("my.tool", false)];
        assert_eq!(
            build_error(&model, &request),
            RequestBuildError::ToolNameInvalid {
                name: ToolName::parse("my.tool").expect("name")
            }
        );
    }

    #[test]
    fn a_strict_tool_needs_the_strict_schema_capability() {
        let model = openai(MODEL_TOOLS_ONLY);
        let mut request = base_request(&model);
        request.tools = vec![tool_def("lookup", true)];
        request.parallel_tools = false;
        assert_eq!(
            build_error(&model, &request),
            RequestBuildError::CapabilityUnavailable {
                capability: Capability::StrictToolSchema
            }
        );
    }

    #[test]
    fn parallel_tool_calls_need_the_parallel_capability() {
        let model = openai(MODEL_TOOLS_ONLY);
        let mut request = base_request(&model);
        request.tools = vec![tool_def("lookup", false)];
        request.parallel_tools = true;
        assert_eq!(
            build_error(&model, &request),
            RequestBuildError::CapabilityUnavailable {
                capability: Capability::ParallelTools
            }
        );
    }

    #[test]
    fn an_unhonoured_sampling_field_is_refused() {
        let model = openai(MODEL_MINIMAL);
        let mut temperature = base_request(&model);
        temperature.temperature_milli = Some(500);
        assert_eq!(
            build_error(&model, &temperature),
            RequestBuildError::SamplingUnsupported {
                field: "temperature"
            }
        );

        let mut top_p = base_request(&model);
        top_p.top_p_milli = Some(500);
        assert_eq!(
            build_error(&model, &top_p),
            RequestBuildError::SamplingUnsupported { field: "top_p" }
        );
    }

    #[test]
    fn a_sampling_value_outside_the_declared_range_is_refused() {
        let model = openai(MODEL_FULL);
        let mut request = base_request(&model);
        request.temperature_milli = Some(2_500);
        assert!(matches!(
            build_error(&model, &request),
            RequestBuildError::Encoding { .. }
        ));
    }

    #[test]
    fn sampling_is_refused_where_reasoning_forbids_it() {
        let model = openai(MODEL_THINKING);
        let mut request = base_request(&model);
        request.reasoning = ReasoningRequest::Enabled {
            budget_tokens: None,
            effort: Some(ReasoningEffort::Medium),
        };
        request.temperature_milli = Some(500);
        assert_eq!(
            build_error(&model, &request),
            RequestBuildError::SamplingWithReasoning {
                field: "temperature"
            }
        );
    }

    #[test]
    fn a_reasoning_token_budget_is_refused_because_this_dialect_has_no_such_member() {
        let model = openai(MODEL_FULL);
        let mut request = base_request(&model);
        request.reasoning = ReasoningRequest::Enabled {
            budget_tokens: Some(2_048),
            effort: None,
        };
        assert!(matches!(
            build_error(&model, &request),
            RequestBuildError::Encoding { .. }
        ));
    }

    #[test]
    fn reasoning_material_from_another_provider_is_refused() {
        let model = openai(MODEL_FULL);
        let mut request = base_request(&model);
        request.messages.push(CanonicalMessage {
            role: Role::Assistant,
            blocks: vec![CanonicalBlock::Reasoning(ReasoningBlock {
                body: ReasoningBody::Redacted,
                token: Some(ReasoningToken {
                    provenance: ProviderId::Anthropic,
                    bytes: bytes::Bytes::from_static(b"sig"),
                }),
            })],
        });
        assert_eq!(
            build_error(&model, &request),
            RequestBuildError::ReasoningProvenanceMismatch {
                expected: ProviderId::Openai,
                found: ProviderId::Anthropic,
            }
        );
    }

    #[test]
    fn a_pair_requiring_replay_material_refuses_a_reasoning_block_without_it() {
        let model = openai(MODEL_THINKING);
        let mut request = base_request(&model);
        request.messages.push(CanonicalMessage {
            role: Role::Assistant,
            blocks: vec![CanonicalBlock::Reasoning(ReasoningBlock {
                body: ReasoningBody::Text {
                    text: fixture::bounded("thinking"),
                },
                token: None,
            })],
        });
        assert_eq!(
            build_error(&model, &request),
            RequestBuildError::ReasoningTokenRequired
        );
    }

    #[test]
    fn an_estimated_context_overflow_is_refused() {
        let model = openai(MODEL_TINY_CONTEXT);
        let mut request = base_request(&model);
        request.messages = vec![user_text(&"a".repeat(400))];
        assert_eq!(
            build_error(&model, &request),
            RequestBuildError::ContextOverflowEstimated {
                limit: 8,
                estimated: 100,
            }
        );
    }

    #[test]
    fn an_over_large_body_is_refused() {
        let model = openai(MODEL_TINY_BODY);
        let mut request = base_request(&model);
        request.messages = vec![user_text(&"a".repeat(400))];
        assert_eq!(
            build_error(&model, &request),
            RequestBuildError::BodyTooLarge { limit: 256 }
        );
    }

    #[test]
    fn a_pair_from_another_provider_is_refused() {
        let model = pair(ProviderId::Anthropic, MODEL_ANTHROPIC);
        assert!(matches!(
            build_error(&model, &base_request(&model)),
            RequestBuildError::Encoding { .. }
        ));
    }

    #[test]
    fn a_chat_completions_tool_encoding_is_refused() {
        let model = openai(MODEL_NESTED_TOOLS);
        let mut request = base_request(&model);
        request.tools = vec![tool_def("lookup", false)];
        assert!(matches!(
            build_error(&model, &request),
            RequestBuildError::Encoding { .. }
        ));
    }

    #[test]
    fn a_request_whose_selection_is_not_the_pinned_pair_is_refused() {
        let model = openai(MODEL_FULL);
        let other = openai(MODEL_MINIMAL);
        assert!(matches!(
            build_error(&model, &base_request(&other)),
            RequestBuildError::Encoding { .. }
        ));
    }

    // -----------------------------------------------------------------------
    // frame taxonomy
    // -----------------------------------------------------------------------

    #[test]
    fn the_first_frame_starts_the_response_and_captures_the_response_id() {
        let mut state = DialectState::new();
        assert_eq!(
            decode(&mut state, CREATED),
            Ok(FrameOutcome::ResponseStarted)
        );
        assert_eq!(
            state.request_id.as_ref().map(BoundedString::as_str),
            Some("resp_abc")
        );
        assert_eq!(
            decode(
                &mut state,
                r#"{"type":"response.in_progress","sequence_number":1,"response":{"id":"resp_abc","status":"in_progress"}}"#
            ),
            Ok(FrameOutcome::Progress)
        );
    }

    #[test]
    fn a_stream_that_does_not_open_with_response_created_is_out_of_order() {
        let mut state = DialectState::new();
        assert_eq!(
            decode(
                &mut state,
                r#"{"type":"response.output_text.delta","sequence_number":0,"output_index":0,"delta":"x"}"#
            ),
            Err(FrameDecodeError::OutOfOrder {
                reason: "the stream did not open with response.created"
            })
        );
    }

    #[test]
    fn the_text_taxonomy_decodes_into_one_sealed_text_block() {
        let done = completed(8);
        let (state, last) = play(&[
            CREATED,
            r#"{"type":"response.output_item.added","sequence_number":1,"output_index":0,"item":{"type":"message","role":"assistant"}}"#,
            r#"{"type":"response.content_part.added","sequence_number":2,"output_index":0,"content_index":0,"part":{"type":"output_text","text":""}}"#,
            r#"{"type":"response.output_text.delta","sequence_number":3,"output_index":0,"content_index":0,"item_id":"msg_1","delta":"he"}"#,
            r#"{"type":"response.output_text.delta","sequence_number":4,"output_index":0,"content_index":0,"item_id":"msg_1","delta":"llo"}"#,
            r#"{"type":"response.output_text.done","sequence_number":5,"output_index":0,"content_index":0,"item_id":"msg_1","text":"hello"}"#,
            r#"{"type":"response.content_part.done","sequence_number":6,"output_index":0,"content_index":0,"part":{"type":"output_text","text":"hello"}}"#,
            r#"{"type":"response.output_item.done","sequence_number":7,"output_index":0,"item":{"type":"message","role":"assistant"}}"#,
            &done,
        ]);
        assert_eq!(last, Ok(FrameOutcome::Terminal));
        let sealed = OpenAiAdapter.finish(state).expect("the stream seals");
        assert_eq!(sealed.stop_reason, StopReason::EndTurn);
        assert_eq!(
            sealed.blocks,
            vec![CanonicalBlock::Text {
                text: fixture::bounded("hello"),
                annotations: Vec::new(),
            }]
        );
        assert_eq!(
            sealed
                .provider_request_id
                .as_ref()
                .map(BoundedString::as_str),
            Some("resp_abc")
        );
    }

    #[test]
    fn the_refusal_taxonomy_decodes_into_a_refusal_block() {
        let done = completed(6);
        let (state, last) = play(&[
            CREATED,
            r#"{"type":"response.output_item.added","sequence_number":1,"output_index":0,"item":{"type":"message","role":"assistant"}}"#,
            r#"{"type":"response.refusal.delta","sequence_number":2,"output_index":0,"content_index":0,"item_id":"msg_1","delta":"I can"}"#,
            r#"{"type":"response.refusal.delta","sequence_number":3,"output_index":0,"content_index":0,"item_id":"msg_1","delta":"not"}"#,
            r#"{"type":"response.refusal.done","sequence_number":4,"output_index":0,"content_index":0,"item_id":"msg_1","refusal":"I cannot"}"#,
            r#"{"type":"response.output_item.done","sequence_number":5,"output_index":0,"item":{"type":"message","role":"assistant"}}"#,
            &done,
        ]);
        assert_eq!(last, Ok(FrameOutcome::Terminal));
        let sealed = OpenAiAdapter.finish(state).expect("the stream seals");
        assert_eq!(
            sealed.blocks,
            vec![CanonicalBlock::Refusal {
                text: fixture::bounded("I cannot"),
            }]
        );
    }

    #[test]
    fn a_provider_hosted_tool_event_is_a_protocol_violation() {
        // AEX declares no hosted tool, so any of these means the request was
        // altered in flight (D-25).
        for kind in [
            "response.web_search_call.in_progress",
            "response.file_search_call.searching",
            "response.code_interpreter_call.interpreting",
            "response.image_generation_call.generating",
            "response.mcp_call.in_progress",
            "response.mcp_list_tools.completed",
            "response.audio.delta",
        ] {
            let mut state = DialectState::new();
            decode(&mut state, CREATED).expect("the stream opens");
            let frame = format!(
                r#"{{"type":"{kind}","sequence_number":1,"output_index":0,"item_id":"x"}}"#
            );
            let error = decode(&mut state, &frame).expect_err("a hosted-tool frame is refused");
            match error {
                FrameDecodeError::UnknownEvent { event } => assert_eq!(event.as_str(), kind),
                other => panic!("{kind} produced {other:?}"),
            }
        }
    }

    #[test]
    fn a_hosted_tool_output_item_is_a_protocol_violation() {
        let mut state = DialectState::new();
        decode(&mut state, CREATED).expect("the stream opens");
        let error = decode(
            &mut state,
            r#"{"type":"response.output_item.added","sequence_number":1,"output_index":0,"item":{"type":"web_search_call","id":"ws_1"}}"#,
        )
        .expect_err("a hosted-tool item is refused");
        assert!(matches!(error, FrameDecodeError::UnknownEvent { .. }));
    }

    #[test]
    fn an_sse_event_name_that_disagrees_with_the_frame_type_is_out_of_order() {
        let mut state = DialectState::new();
        assert_eq!(
            decode_named(&mut state, Some("response.completed"), CREATED),
            Err(FrameDecodeError::OutOfOrder {
                reason: "the SSE event name does not match the frame type"
            })
        );
    }

    #[test]
    fn a_frame_that_is_not_json_is_rejected() {
        let mut state = DialectState::new();
        assert_eq!(
            decode_named(&mut state, None, "not json at all"),
            Err(FrameDecodeError::NotJson)
        );
    }

    #[test]
    fn a_sequence_number_that_goes_backwards_is_rejected() {
        let mut state = DialectState::new();
        decode(&mut state, CREATED).expect("the stream opens");
        decode(
            &mut state,
            r#"{"type":"response.in_progress","sequence_number":1,"response":{"id":"resp_abc","status":"in_progress"}}"#,
        )
        .expect("second frame");
        assert_eq!(
            decode(
                &mut state,
                r#"{"type":"response.in_progress","sequence_number":1,"response":{"id":"resp_abc","status":"in_progress"}}"#
            ),
            Err(FrameDecodeError::OutOfOrder {
                reason: "sequence_number did not advance"
            })
        );
    }

    #[test]
    fn a_frame_after_the_terminal_frame_is_rejected() {
        let done = completed(1);
        let (mut state, last) = play(&[CREATED, &done]);
        assert_eq!(last, Ok(FrameOutcome::Terminal));
        assert_eq!(
            decode(
                &mut state,
                r#"{"type":"response.in_progress","sequence_number":2,"response":{"id":"resp_abc","status":"in_progress"}}"#
            ),
            Err(FrameDecodeError::OutOfOrder {
                reason: "a frame arrived after the terminal frame"
            })
        );
    }

    #[test]
    fn a_stream_that_ends_without_a_terminal_frame_is_rejected() {
        let (state, _) = play(&[CREATED]);
        assert_eq!(
            OpenAiAdapter.finish(state),
            Err(FrameDecodeError::OutOfOrder {
                reason: "the stream ended without a terminal frame"
            })
        );
    }

    #[test]
    fn a_stream_that_ends_with_an_unclosed_item_is_rejected() {
        let done = completed(3);
        let (state, last) = play(&[
            CREATED,
            r#"{"type":"response.output_item.added","sequence_number":1,"output_index":0,"item":{"type":"message","role":"assistant"}}"#,
            r#"{"type":"response.output_text.delta","sequence_number":2,"output_index":0,"content_index":0,"item_id":"msg_1","delta":"he"}"#,
            &done,
        ]);
        assert_eq!(last, Ok(FrameOutcome::Terminal));
        assert_eq!(
            OpenAiAdapter.finish(state),
            Err(FrameDecodeError::OutOfOrder {
                reason: "the stream ended with an unclosed output item"
            })
        );
    }

    // -----------------------------------------------------------------------
    // tool-argument reassembly
    // -----------------------------------------------------------------------

    /// The three tool frames, split so that the fragment boundary falls inside
    /// the `\"` escape of the argument `JSON`.
    const TOOL_OPEN: &str = r#"{"type":"response.output_item.added","sequence_number":1,"output_index":0,"item":{"type":"function_call","id":"fc_1","call_id":"call_abc","name":"lookup","arguments":""}}"#;
    const TOOL_FRAGMENT_A: &str = r#"{"type":"response.function_call_arguments.delta","sequence_number":2,"output_index":0,"item_id":"fc_1","delta":"{\"q\":\"a\\"}"#;
    const TOOL_FRAGMENT_B: &str = r#"{"type":"response.function_call_arguments.delta","sequence_number":3,"output_index":0,"item_id":"fc_1","delta":"\"b\"}"}"#;
    const TOOL_ARGUMENTS_DONE: &str = r#"{"type":"response.function_call_arguments.done","sequence_number":4,"output_index":0,"item_id":"fc_1","arguments":"{\"q\":\"a\\\"b\"}"}"#;
    const TOOL_ITEM_DONE: &str = r#"{"type":"response.output_item.done","sequence_number":5,"output_index":0,"item":{"type":"function_call","id":"fc_1","call_id":"call_abc","name":"lookup","arguments":"{\"q\":\"a\\\"b\"}"}}"#;

    #[test]
    fn tool_arguments_reassemble_across_a_fragment_boundary_inside_a_string_escape() {
        let done = completed(6);
        let (state, last) = play(&[
            CREATED,
            TOOL_OPEN,
            TOOL_FRAGMENT_A,
            TOOL_FRAGMENT_B,
            TOOL_ARGUMENTS_DONE,
            TOOL_ITEM_DONE,
            &done,
        ]);
        assert_eq!(last, Ok(FrameOutcome::Terminal));
        let sealed = OpenAiAdapter.finish(state).expect("the stream seals");
        assert_eq!(sealed.stop_reason, StopReason::ToolUse);
        assert_eq!(
            sealed.blocks,
            vec![CanonicalBlock::ToolUse {
                id: fixture::bounded("call_abc"),
                name: ToolName::parse("lookup").expect("name"),
                input: CanonicalJson::parse(r#"{"q":"a\"b"}"#).expect("input"),
            }]
        );
    }

    #[test]
    fn tool_argument_fragments_that_disagree_with_the_closing_frame_are_rejected() {
        let (_, last) = play(&[
            CREATED,
            TOOL_OPEN,
            TOOL_FRAGMENT_A,
            r#"{"type":"response.function_call_arguments.done","sequence_number":3,"output_index":0,"item_id":"fc_1","arguments":"{\"q\":\"different\"}"}"#,
        ]);
        assert_eq!(
            last,
            Err(FrameDecodeError::OutOfOrder {
                reason: "the reassembled tool arguments do not match the closing arguments"
            })
        );
    }

    #[test]
    fn tool_arguments_that_are_not_json_are_rejected() {
        let (_, last) = play(&[
            CREATED,
            TOOL_OPEN,
            r#"{"type":"response.function_call_arguments.delta","sequence_number":2,"output_index":0,"item_id":"fc_1","delta":"{ not json"}"#,
            r#"{"type":"response.function_call_arguments.done","sequence_number":3,"output_index":0,"item_id":"fc_1","arguments":"{ not json"}"#,
            r#"{"type":"response.output_item.done","sequence_number":4,"output_index":0,"item":{"type":"function_call","id":"fc_1","call_id":"call_abc","name":"lookup","arguments":"{ not json"}}"#,
        ]);
        assert_eq!(
            last,
            Err(FrameDecodeError::ToolArgumentsNotJson {
                call: fixture::bounded("call_abc")
            })
        );
    }

    #[test]
    fn tool_arguments_for_an_item_that_never_opened_are_rejected() {
        let (_, last) = play(&[
            CREATED,
            r#"{"type":"response.function_call_arguments.delta","sequence_number":1,"output_index":3,"item_id":"fc_9","delta":"{}"}"#,
        ]);
        assert_eq!(
            last,
            Err(FrameDecodeError::OutOfOrder {
                reason: "tool arguments arrived for an item that never opened"
            })
        );
    }

    // -----------------------------------------------------------------------
    // reasoning capture
    // -----------------------------------------------------------------------

    #[test]
    fn reasoning_text_and_encrypted_material_are_captured_together() {
        let done = completed(6);
        let (state, last) = play(&[
            CREATED,
            r#"{"type":"response.output_item.added","sequence_number":1,"output_index":0,"item":{"type":"reasoning","id":"rs_1"}}"#,
            r#"{"type":"response.reasoning_text.delta","sequence_number":2,"output_index":0,"item_id":"rs_1","content_index":0,"delta":"thin"}"#,
            r#"{"type":"response.reasoning_text.delta","sequence_number":3,"output_index":0,"item_id":"rs_1","content_index":0,"delta":"king"}"#,
            r#"{"type":"response.reasoning_text.done","sequence_number":4,"output_index":0,"item_id":"rs_1","content_index":0,"text":"thinking"}"#,
            r#"{"type":"response.output_item.done","sequence_number":5,"output_index":0,"item":{"type":"reasoning","id":"rs_1","encrypted_content":"gAAAA","summary":[]}}"#,
            &done,
        ]);
        assert_eq!(last, Ok(FrameOutcome::Terminal));
        let sealed = OpenAiAdapter.finish(state).expect("the stream seals");
        assert_eq!(
            sealed.blocks,
            vec![CanonicalBlock::Reasoning(ReasoningBlock {
                body: ReasoningBody::Text {
                    text: fixture::bounded("thinking"),
                },
                token: Some(ReasoningToken {
                    provenance: ProviderId::Openai,
                    bytes: bytes::Bytes::from_static(b"gAAAA"),
                }),
            })]
        );
    }

    #[test]
    fn a_reasoning_summary_is_captured_as_a_summary_body() {
        let done = completed(5);
        let (state, last) = play(&[
            CREATED,
            r#"{"type":"response.output_item.added","sequence_number":1,"output_index":0,"item":{"type":"reasoning","id":"rs_1"}}"#,
            r#"{"type":"response.reasoning_summary_text.delta","sequence_number":2,"output_index":0,"item_id":"rs_1","summary_index":0,"delta":"short"}"#,
            r#"{"type":"response.reasoning_summary_text.done","sequence_number":3,"output_index":0,"item_id":"rs_1","summary_index":0,"text":"short"}"#,
            r#"{"type":"response.output_item.done","sequence_number":4,"output_index":0,"item":{"type":"reasoning","id":"rs_1","summary":[]}}"#,
            &done,
        ]);
        assert_eq!(last, Ok(FrameOutcome::Terminal));
        let sealed = OpenAiAdapter.finish(state).expect("the stream seals");
        assert_eq!(
            sealed.blocks,
            vec![CanonicalBlock::Reasoning(ReasoningBlock {
                body: ReasoningBody::Summary {
                    text: fixture::bounded("short"),
                },
                token: None,
            })]
        );
    }

    // -----------------------------------------------------------------------
    // usage
    // -----------------------------------------------------------------------

    #[test]
    fn the_usage_mapping_follows_the_openai_members() {
        let done = completed(1);
        let (state, _) = play(&[CREATED, &done]);
        let usage = state.usage;
        assert_eq!(usage.input_tokens, 11);
        assert_eq!(usage.cache_read_input_tokens, 8);
        assert_eq!(usage.cache_write_input_tokens, 3);
        assert_eq!(usage.output_tokens, 5);
        assert_eq!(usage.reasoning_tokens, 2);
        assert_eq!(usage.provider_total_tokens, Some(16));
        assert_eq!(usage.completeness, UsageCompleteness::Exact);
        assert!(
            usage.is_consistent(),
            "reasoning is a subset of output for this provider"
        );
    }

    #[test]
    fn a_partial_usage_payload_records_exactly_what_was_missing() {
        let (state, _) = play(&[
            CREATED,
            r#"{"type":"response.completed","sequence_number":1,"response":{"id":"resp_abc","status":"completed","usage":{"input_tokens":11,"output_tokens":5,"total_tokens":16}}}"#,
        ]);
        let expected = UsageFieldSet::EMPTY
            .with(UsageField::CacheReadInputTokens)
            .with(UsageField::CacheWriteInputTokens)
            .with(UsageField::ReasoningTokens);
        assert_eq!(
            state.usage.completeness,
            UsageCompleteness::Partial { missing: expected }
        );
    }

    #[test]
    fn absent_usage_is_recorded_as_absent_rather_than_invented() {
        let (state, _) = play(&[
            CREATED,
            r#"{"type":"response.completed","sequence_number":1,"response":{"id":"resp_abc","status":"completed"}}"#,
        ]);
        assert_eq!(state.usage.completeness, UsageCompleteness::Absent);
        assert_eq!(state.usage.input_tokens, 0);
        assert_eq!(state.usage.provider_total_tokens, None);
    }

    // -----------------------------------------------------------------------
    // stop and failure mapping
    // -----------------------------------------------------------------------

    #[test]
    fn the_stop_mapping_table_is_exhaustive() {
        // completed with no tool call.
        let done = completed(1);
        let (state, outcome) = play(&[CREATED, &done]);
        assert_eq!(outcome, Ok(FrameOutcome::Terminal));
        assert_eq!(
            OpenAiAdapter.finish(state).expect("seals").stop_reason,
            StopReason::EndTurn
        );

        // completed with a function_call in the output.
        let done = completed(6);
        let (state, outcome) = play(&[
            CREATED,
            TOOL_OPEN,
            TOOL_FRAGMENT_A,
            TOOL_FRAGMENT_B,
            TOOL_ARGUMENTS_DONE,
            TOOL_ITEM_DONE,
            &done,
        ]);
        assert_eq!(outcome, Ok(FrameOutcome::Terminal));
        assert_eq!(
            OpenAiAdapter.finish(state).expect("seals").stop_reason,
            StopReason::ToolUse
        );

        // incomplete / max_output_tokens.
        let (state, outcome) = play(&[
            CREATED,
            r#"{"type":"response.incomplete","sequence_number":1,"response":{"id":"resp_abc","status":"incomplete","incomplete_details":{"reason":"max_output_tokens"}}}"#,
        ]);
        assert_eq!(outcome, Ok(FrameOutcome::Terminal));
        assert_eq!(
            OpenAiAdapter.finish(state).expect("seals").stop_reason,
            StopReason::MaxOutputTokens
        );

        // incomplete / content_filter is a failure, never a stop reason.
        let (_, outcome) = play(&[
            CREATED,
            r#"{"type":"response.incomplete","sequence_number":1,"response":{"id":"resp_abc","status":"incomplete","incomplete_details":{"reason":"content_filter"}}}"#,
        ]);
        assert_failed(&outcome, ProviderFailureKind::ContentFiltered);

        // failed, classified from response.error.
        let (_, outcome) = play(&[
            CREATED,
            r#"{"type":"response.failed","sequence_number":1,"response":{"id":"resp_abc","status":"failed","error":{"code":"rate_limit_exceeded","message":"slow down"}}}"#,
        ]);
        assert_failed(&outcome, ProviderFailureKind::RateLimited);

        // cancelled.
        let (_, outcome) = play(&[
            CREATED,
            r#"{"type":"response.incomplete","sequence_number":1,"response":{"id":"resp_abc","status":"cancelled"}}"#,
        ]);
        assert_failed(&outcome, ProviderFailureKind::Cancelled);

        // an unknown status is a protocol violation, not a guess.
        let (_, outcome) = play(&[
            CREATED,
            r#"{"type":"response.completed","sequence_number":1,"response":{"id":"resp_abc","status":"partially_done"}}"#,
        ]);
        assert_eq!(
            outcome,
            Err(FrameDecodeError::MalformedField {
                field: "response.status"
            })
        );
    }

    fn assert_failed(
        outcome: &Result<FrameOutcome, FrameDecodeError>,
        expected: ProviderFailureKind,
    ) {
        match outcome {
            Ok(FrameOutcome::Failed(failure)) => assert_eq!(failure.kind(), expected),
            other => panic!("expected a typed failure, got {other:?}"),
        }
    }

    #[test]
    fn an_in_stream_error_frame_is_a_typed_failure_and_ends_the_stream() {
        let (state, outcome) = play(&[
            CREATED,
            r#"{"type":"error","sequence_number":1,"code":"server_error","message":"upstream fell over","param":null}"#,
        ]);
        assert_failed(&outcome, ProviderFailureKind::ServerError);
        assert!(state.terminal, "an error frame ends the stream");
    }

    #[test]
    fn a_provider_message_is_redacted_before_it_reaches_a_detail() {
        let key = "sk-0123456789abcdef0123456789abcdef";
        let frame = format!(
            r#"{{"type":"error","sequence_number":1,"code":"invalid_prompt","message":"bad key {key}"}}"#
        );
        let (_, outcome) = play(&[CREATED, &frame]);
        match outcome {
            Ok(FrameOutcome::Failed(failure)) => {
                assert!(
                    !failure.detail.message.as_str().contains(key),
                    "{}",
                    failure.detail.message
                );
                assert_eq!(failure.kind(), ProviderFailureKind::InvalidRequest);
            }
            other => panic!("expected a typed failure, got {other:?}"),
        }
    }

    #[test]
    fn the_error_mapping_table_is_exhaustive() {
        // Status rules.
        for (status, expected) in [
            (400u16, ProviderFailureKind::InvalidRequest),
            (401, ProviderFailureKind::Authentication),
            (403, ProviderFailureKind::Authentication),
            (404, ProviderFailureKind::ModelNotFound),
            (408, ProviderFailureKind::Timeout),
            (409, ProviderFailureKind::InvalidRequest),
            (413, ProviderFailureKind::InvalidRequest),
            (422, ProviderFailureKind::InvalidRequest),
            (429, ProviderFailureKind::RateLimited),
            (500, ProviderFailureKind::ServerError),
            (502, ProviderFailureKind::ServerError),
            (503, ProviderFailureKind::Overloaded),
            (504, ProviderFailureKind::ServerError),
        ] {
            let body = r#"{"error":{"code":null,"message":"x","param":null,"type":"unknown"}}"#;
            assert_eq!(classify(status, body), expected, "status {status}");
        }

        // A documented code always wins over the status.
        for (code, expected) in [
            ("credit_balance_exhausted", ProviderFailureKind::Billing),
            (
                "organization_spend_limit_exceeded",
                ProviderFailureKind::Quota,
            ),
            ("insufficient_quota", ProviderFailureKind::Quota),
            ("rate_limit_exceeded", ProviderFailureKind::RateLimited),
            (
                "previous_response_not_found",
                ProviderFailureKind::InvalidRequest,
            ),
            ("invalid_prompt", ProviderFailureKind::InvalidRequest),
            (
                "context_length_exceeded",
                ProviderFailureKind::ContextOverflow,
            ),
            ("model_not_found", ProviderFailureKind::ModelNotFound),
            ("invalid_api_key", ProviderFailureKind::Authentication),
            ("server_error", ProviderFailureKind::ServerError),
        ] {
            let body = format!(
                r#"{{"error":{{"code":"{code}","message":"x","param":null,"type":"invalid_request_error"}}}}"#
            );
            assert_eq!(classify(429, &body), expected, "code {code}");
        }

        // `type` is consulted only when `code` is null.
        for (kind, expected) in [
            ("invalid_request_error", ProviderFailureKind::InvalidRequest),
            ("authentication_error", ProviderFailureKind::Authentication),
            ("permission_error", ProviderFailureKind::Authentication),
            ("rate_limit_error", ProviderFailureKind::RateLimited),
            ("api_error", ProviderFailureKind::ServerError),
        ] {
            let body = format!(
                r#"{{"error":{{"code":null,"message":"x","param":null,"type":"{kind}"}}}}"#
            );
            assert_eq!(classify(500, &body), expected, "type {kind}");
        }
    }

    #[test]
    fn an_unparsable_error_body_still_classifies_by_status() {
        let headers = HeaderMap::new();
        let failure = OpenAiAdapter.classify_http(
            503,
            &HeaderView::new(&headers),
            &BoundedBody::new(b"{\"error\":".to_vec(), true),
        );
        assert_eq!(failure.kind(), ProviderFailureKind::Overloaded);
        assert_eq!(failure.detail.http_status, Some(503));
        assert_eq!(failure.detail.provider_code, None);
    }

    #[test]
    fn an_error_body_message_is_redacted_and_the_code_is_preserved() {
        let headers = HeaderMap::new();
        let key = "sk-proj-abcdefghij0123456789abcdefghij";
        let body = format!(
            r#"{{"error":{{"code":"invalid_api_key","message":"Incorrect API key provided: {key}","param":null,"type":"invalid_request_error"}}}}"#
        );
        let failure = OpenAiAdapter.classify_http(
            401,
            &HeaderView::new(&headers),
            &BoundedBody::new(body.into_bytes(), false),
        );
        assert_eq!(failure.kind(), ProviderFailureKind::Authentication);
        assert!(!failure.detail.message.as_str().contains(key));
        assert_eq!(
            failure
                .detail
                .provider_code
                .as_ref()
                .map(BoundedString::as_str),
            Some("invalid_api_key")
        );
    }

    // -----------------------------------------------------------------------
    // rate limits and request id
    // -----------------------------------------------------------------------

    #[test]
    fn rate_limit_feedback_comes_from_the_vendor_headers() {
        let mut headers = HeaderMap::new();
        headers.insert("retry-after", "12".parse().expect("value"));
        headers.insert(
            "x-ratelimit-remaining-requests",
            "42".parse().expect("value"),
        );
        headers.insert(
            "x-ratelimit-remaining-tokens",
            "900".parse().expect("value"),
        );
        headers.insert(
            "x-ratelimit-remaining-project-tokens",
            "100".parse().expect("value"),
        );
        headers.insert("x-ratelimit-reset-requests", "6m0s".parse().expect("value"));

        let feedback = OpenAiAdapter.rate_limit_feedback(&HeaderView::new(&headers));
        assert_eq!(feedback.source, RateLimitSource::VendorHeaders);
        assert_eq!(
            feedback.retry_after,
            Some(core::time::Duration::from_secs(12))
        );
        assert_eq!(feedback.requests_remaining, Some(42));
        assert_eq!(
            feedback.tokens_remaining,
            Some(100),
            "the binding ceiling is the smaller of the two"
        );
        assert_eq!(
            feedback.reset_at, None,
            "the reset header is relative and this method holds no clock"
        );
    }

    #[test]
    fn absent_rate_limit_headers_are_a_positive_record_of_absence() {
        let headers = HeaderMap::new();
        let feedback = OpenAiAdapter.rate_limit_feedback(&HeaderView::new(&headers));
        assert!(feedback.is_absent());
        assert_eq!(feedback.source, RateLimitSource::NotProvided);
    }

    #[test]
    fn the_request_id_prefers_the_response_header_over_the_body() {
        let (state, _) = play(&[CREATED]);
        let mut headers = HeaderMap::new();
        headers.insert("x-request-id", "req_from_header".parse().expect("value"));
        assert_eq!(
            OpenAiAdapter
                .request_id(&HeaderView::new(&headers), &state)
                .as_ref()
                .map(BoundedString::as_str),
            Some("req_from_header")
        );

        let empty = HeaderMap::new();
        assert_eq!(
            OpenAiAdapter
                .request_id(&HeaderView::new(&empty), &state)
                .as_ref()
                .map(BoundedString::as_str),
            Some("resp_abc"),
            "the response id is the fallback, since store:false makes it unfetchable later"
        );
    }

    // -----------------------------------------------------------------------
    // bounds
    // -----------------------------------------------------------------------

    #[test]
    fn the_tool_call_bound_is_enforced_while_decoding() {
        let budget = StreamBudget {
            max_tool_calls: 1,
            ..StreamBudget::default()
        };
        let mut state = DialectState::new();
        OpenAiAdapter
            .decode(
                &mut state,
                &SseEvent {
                    name: Some("response.created"),
                    data: CREATED.as_bytes(),
                    id: None,
                },
                &budget,
            )
            .expect("the stream opens");
        OpenAiAdapter
            .decode(
                &mut state,
                &SseEvent {
                    name: Some("response.output_item.added"),
                    data: TOOL_OPEN.as_bytes(),
                    id: None,
                },
                &budget,
            )
            .expect("the first tool call fits");
        let second = r#"{"type":"response.output_item.added","sequence_number":2,"output_index":1,"item":{"type":"function_call","id":"fc_2","call_id":"call_def","name":"lookup","arguments":""}}"#;
        let error = OpenAiAdapter
            .decode(
                &mut state,
                &SseEvent {
                    name: Some("response.output_item.added"),
                    data: second.as_bytes(),
                    id: None,
                },
                &budget,
            )
            .expect_err("the second tool call crosses the bound");
        assert!(matches!(error, FrameDecodeError::Budget(_)));
    }

    #[test]
    fn an_output_index_in_the_alternate_key_space_is_refused() {
        let mut state = DialectState::new();
        decode(&mut state, CREATED).expect("the stream opens");
        assert_eq!(
            decode(
                &mut state,
                r#"{"type":"response.output_text.delta","sequence_number":1,"output_index":32768,"delta":"x"}"#
            ),
            Err(FrameDecodeError::MalformedField {
                field: "output_index"
            })
        );
    }
}
