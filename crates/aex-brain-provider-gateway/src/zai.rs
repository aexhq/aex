//! The `zai` dialect adapter: Z.AI's general chat-completions API (plan 08 §5.4).
//!
//! # What is compiled rather than configured
//!
//! The origin is [`EndpointPin::ZaiPaasV4`] and the path is
//! [`CHAT_COMPLETIONS_PATH`]. The Coding-Plan base and the Anthropic-compatible
//! base are launch exclusions: hitting the general endpoint with a Coding-Plan
//! key silently bills pay-as-you-go, so neither is reachable from any document.
//!
//! # The mid-stream trap
//!
//! During `SSE` this API returns **no** error code. The reason arrives only in
//! `finish_reason`, so `sensitive`, `model_context_window_exceeded` and
//! `network_error` are decoded as terminal **failures** rather than stop
//! reasons. Because they can only arrive after a chunk carrying `choices[0]`
//! has already moved the state to `response_started`, the dispatch proof for
//! all three is `ResponseStarted`.
//!
//! # Correlation
//!
//! Z.AI publishes no request-id header. The only handle it offers is the
//! `request_id` the caller sends, which the response echoes, so `build_request`
//! pins it to the derived non-secret `CorrelationId` (D-23).

use aex_model_catalog::QualifiedModel;
use aex_model_catalog::canonical::{
    CacheBreakpoint, CanonicalBlock, CanonicalMessage, CanonicalModelRequest, CanonicalToolDef,
    CorrelationId, NormalizedUsage, REASON_MAX, ReasoningBlock, ReasoningBody, ReasoningEffort,
    ReasoningRequest, ReasoningToken, Role, StopReason, StructuredOutputRequest, SystemBlock,
    TEXT_MAX, ToolChoice, ToolResultPart, UsageCompleteness, UsageField, UsageFieldSet,
};
use aex_model_catalog::document::{
    AdapterSourceDigest, Capability, EndpointPin, ModelEntry, SamplingSupport,
};
use aex_model_catalog::primitives::{
    Blake3Digest, BoundedString, ProviderRequestId, ToolCallId, ToolName,
};
use aex_wire::CanonicalJson;
use aex_wire::provider::ProviderId;
use serde_json::{Value, json};

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

/// The one path this dialect posts to.
pub const CHAT_COMPLETIONS_PATH: &str = "/paas/v4/chat/completions";

/// The non-secret language header every Z.AI request carries.
pub const ACCEPT_LANGUAGE: (&str, &str) = ("accept-language", "en-US,en");

/// Z.AI accepts at most four stop sequences.
pub const MAX_STOP_SEQUENCES: u8 = 4;

/// Z.AI's temperature range is 0.0–1.0, half the `OpenAI` range.
pub const MAX_TEMPERATURE_MILLI: u16 = 1_000;

/// Z.AI's `top_p` range is 0.01–1.0.
pub const TOP_P_MILLI_RANGE: (u16, u16) = (10, 1_000);

/// Z.AI's `max_tokens` range is 1–131072.
pub const OUTPUT_TOKEN_RANGE: (u32, u32) = (1, 131_072);

/// Z.AI's `request_id` must be 6–64 characters.
const REQUEST_ID_BYTES: (usize, usize) = (6, 64);

/// The compiled tag this module's conformance receipts are bound to.
const SOURCE_TAG: &[u8] = b"aex-brain-provider-gateway::zai@1";

/// The Z.AI chat-completions adapter.
///
/// A unit struct: the dialect is entirely compiled, and everything that varies
/// per pair arrives on the [`QualifiedModel`]'s catalog entry.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ZaiAdapter;

impl ProviderAdapter for ZaiAdapter {
    fn provider(&self) -> ProviderId {
        ProviderId::Zai
    }

    fn source_digest(&self) -> AdapterSourceDigest {
        AdapterSourceDigest(Blake3Digest::of(SOURCE_TAG))
    }

    fn build_request(
        &self,
        model: &QualifiedModel,
        request: &CanonicalModelRequest,
    ) -> Result<WireRequest, RequestBuildError> {
        build(model.entry(), &RequestView::of(request))
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
        decode_event(state, event, budget)
    }

    fn finish(&self, state: DialectState) -> Result<SealedResponse, FrameDecodeError> {
        seal_state(state)
    }

    fn classify_http(
        &self,
        status: u16,
        _headers: &HeaderView<'_>,
        body: &BoundedBody,
    ) -> ProviderFailure {
        classify(status, body)
    }

    fn rate_limit_feedback(&self, _headers: &HeaderView<'_>) -> RateLimitFeedback {
        // Z.AI's rate-limit table is behind a login-gated console and the API
        // publishes no `x-ratelimit-*` and no `Retry-After`. The absence is a
        // positive record, not an unobserved value.
        RateLimitFeedback::none()
    }

    fn request_id(
        &self,
        _headers: &HeaderView<'_>,
        state: &DialectState,
    ) -> Option<ProviderRequestId> {
        state.request_id.clone()
    }
}

// ---------------------------------------------------------------------------
// request building
// ---------------------------------------------------------------------------

/// A decoder state that starts out recording "no usage observed".
///
/// [`NormalizedUsage`]'s own default is `Exact`, which would claim a complete
/// tally for a stream that never carried one. Usage is never invented, so the
/// fresh state says `Absent` until a chunk overwrites it.
fn fresh_state() -> DialectState {
    DialectState {
        usage: NormalizedUsage {
            completeness: UsageCompleteness::Absent,
            ..NormalizedUsage::default()
        },
        ..DialectState::default()
    }
}

/// The members of a [`CanonicalModelRequest`] this dialect reads.
///
/// Borrowing them behind one view keeps [`build`] a pure function of data a
/// test can construct, which is what lets every capability gate below be
/// exercised without a loaded catalog revision.
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
    cache_breakpoints: &'a [CacheBreakpoint],
    correlation: &'a CorrelationId,
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
            cache_breakpoints: &request.cache_breakpoints,
            correlation: &request.correlation,
        }
    }
}

/// Builds the wire request. Pure: no I/O, no clock, no credential.
fn build(entry: &ModelEntry, view: &RequestView<'_>) -> Result<WireRequest, RequestBuildError> {
    check_capabilities(entry, view)?;
    check_bounds(entry, view)?;

    let mut body = serde_json::Map::new();
    body.insert(
        "model".to_owned(),
        Value::String(entry.model.as_str().to_owned()),
    );
    body.insert("messages".to_owned(), Value::Array(build_messages(view)?));
    body.insert("stream".to_owned(), Value::Bool(true));
    // Prior `reasoning_content` is replayed rather than cleared, which is what
    // makes `ReasoningReplay::RecommendedEcho` mean anything on this dialect.
    body.insert("clear_thinking".to_owned(), Value::Bool(false));
    body.insert("max_tokens".to_owned(), Value::from(view.max_output_tokens));
    body.insert(
        "request_id".to_owned(),
        Value::String(view.correlation.as_str().to_owned()),
    );

    sampling_fields(entry, view, &mut body)?;
    reasoning_fields(view, &mut body)?;

    if !view.stop_sequences.is_empty() {
        let stops = view
            .stop_sequences
            .iter()
            .map(|stop| Value::String(stop.as_str().to_owned()))
            .collect();
        body.insert("stop".to_owned(), Value::Array(stops));
    }
    if !view.tools.is_empty() {
        body.insert("tools".to_owned(), Value::Array(build_tools(entry, view)?));
        body.insert("tool_stream".to_owned(), Value::Bool(true));
        body.insert("tool_choice".to_owned(), Value::String("auto".to_owned()));
    }
    if matches!(
        view.structured_output,
        Some(StructuredOutputRequest::JsonObject)
    ) {
        body.insert(
            "response_format".to_owned(),
            json!({ "type": "json_object" }),
        );
    }

    finish_request(entry, &Value::Object(body))
}

/// Serializes, bounds and wraps the body.
fn finish_request(entry: &ModelEntry, body: &Value) -> Result<WireRequest, RequestBuildError> {
    let bytes = serde_json::to_vec(body).map_err(|_| RequestBuildError::Encoding {
        reason: "the request body could not be serialized",
    })?;
    let limit = entry.limits.request_body_max_bytes;
    if bytes.len() > usize::try_from(limit).unwrap_or(usize::MAX) {
        return Err(RequestBuildError::BodyTooLarge { limit });
    }
    let path =
        BoundedString::new(CHAT_COMPLETIONS_PATH).map_err(|_| RequestBuildError::Encoding {
            reason: "the compiled path exceeds its bound",
        })?;
    let language =
        BoundedString::new(ACCEPT_LANGUAGE.1).map_err(|_| RequestBuildError::Encoding {
            reason: "the compiled language header exceeds its bound",
        })?;
    Ok(WireRequest {
        endpoint: EndpointPin::ZaiPaasV4,
        path,
        query: Vec::new(),
        headers: vec![(ACCEPT_LANGUAGE.0, language)],
        auth: AuthScheme::BearerAuthorization,
        body: bytes::Bytes::from(bytes),
        accept: Accept::TextEventStream,
    })
}

/// Every gate that depends on what the entry declares.
fn check_capabilities(entry: &ModelEntry, view: &RequestView<'_>) -> Result<(), RequestBuildError> {
    let mut required = vec![Capability::Streaming];
    if !view.tools.is_empty() {
        required.push(Capability::Tools);
    }
    if view.tools.iter().any(|tool| tool.strict) {
        required.push(Capability::StrictToolSchema);
    }
    if view.temperature_milli.is_some() {
        required.push(Capability::Temperature);
    }
    if view.top_p_milli.is_some() {
        required.push(Capability::TopP);
    }
    if !view.stop_sequences.is_empty() {
        required.push(Capability::StopSequences);
    }
    if view.structured_output.is_some() {
        required.push(Capability::StructuredOutput);
    }
    if matches!(view.reasoning, ReasoningRequest::Enabled { .. }) {
        required.push(Capability::Reasoning);
    }
    if !view.cache_breakpoints.is_empty() {
        required.push(Capability::PromptCacheExplicit);
    }
    for capability in required {
        if !entry.capabilities.has(capability) {
            return Err(RequestBuildError::CapabilityUnavailable { capability });
        }
    }

    // Only `auto` is supported. `required`, a named tool and `none` are refused
    // before a socket exists rather than silently downgraded.
    if !matches!(view.tool_choice, ToolChoice::Auto) {
        return Err(RequestBuildError::ToolChoiceUnsupported {
            requested: Box::new(view.tool_choice.clone()),
        });
    }
    if let Some(request @ StructuredOutputRequest::JsonSchema { .. }) = view.structured_output {
        return Err(RequestBuildError::StructuredOutputUnsupported {
            requested: Box::new(request.clone()),
        });
    }
    if !view.parallel_tools && !view.tools.is_empty() {
        return Err(RequestBuildError::Encoding {
            reason: "Z.AI has no switch that disables parallel tool calls",
        });
    }
    Ok(())
}

/// Every numeric gate, narrowed to the tighter of the provider fact and the
/// entry's own bound.
fn check_bounds(entry: &ModelEntry, view: &RequestView<'_>) -> Result<(), RequestBuildError> {
    let stops = entry.limits.max_stop_sequences.min(MAX_STOP_SEQUENCES);
    if view.stop_sequences.len() > usize::from(stops) {
        return Err(RequestBuildError::StopSequenceLimit { max: stops });
    }
    if view.tools.len() > usize::from(entry.limits.max_tools) {
        return Err(RequestBuildError::ToolLimit {
            max: entry.limits.max_tools,
        });
    }
    let min = entry.limits.min_output_tokens.max(OUTPUT_TOKEN_RANGE.0);
    let max = entry.limits.max_output_tokens.min(OUTPUT_TOKEN_RANGE.1);
    if view.max_output_tokens < min || view.max_output_tokens > max {
        return Err(RequestBuildError::OutputTokensOutOfRange { min, max });
    }
    let rendered = view.correlation.as_str().len();
    if rendered < REQUEST_ID_BYTES.0 || rendered > REQUEST_ID_BYTES.1 {
        return Err(RequestBuildError::Encoding {
            reason: "the correlation id is outside Z.AI's 6..=64-byte request_id bound",
        });
    }
    Ok(())
}

/// `temperature`, `top_p` and the `do_sample` switch they imply.
fn sampling_fields(
    entry: &ModelEntry,
    view: &RequestView<'_>,
    body: &mut serde_json::Map<String, Value>,
) -> Result<(), RequestBuildError> {
    let reasoning_on = !matches!(view.reasoning, ReasoningRequest::Disabled);
    let mut sampled = false;
    if let Some(temperature) = view.temperature_milli {
        if matches!(entry.sampling, SamplingSupport::None) {
            return Err(RequestBuildError::SamplingUnsupported {
                field: "temperature",
            });
        }
        if entry.reasoning.excludes_sampling && reasoning_on {
            return Err(RequestBuildError::SamplingWithReasoning {
                field: "temperature",
            });
        }
        let (low, high) = entry
            .limits
            .temperature_milli
            .unwrap_or((0, MAX_TEMPERATURE_MILLI));
        if temperature < low || temperature > high.min(MAX_TEMPERATURE_MILLI) {
            return Err(RequestBuildError::SamplingUnsupported {
                field: "temperature",
            });
        }
        body.insert("temperature".to_owned(), milli_to_number(temperature)?);
        sampled = true;
    }
    if let Some(top_p) = view.top_p_milli {
        if !matches!(entry.sampling, SamplingSupport::Full) {
            return Err(RequestBuildError::SamplingUnsupported { field: "top_p" });
        }
        if entry.reasoning.excludes_sampling && reasoning_on {
            return Err(RequestBuildError::SamplingWithReasoning { field: "top_p" });
        }
        let (low, high) = entry.limits.top_p_milli.unwrap_or(TOP_P_MILLI_RANGE);
        if top_p < low.max(TOP_P_MILLI_RANGE.0) || top_p > high.min(TOP_P_MILLI_RANGE.1) {
            return Err(RequestBuildError::SamplingUnsupported { field: "top_p" });
        }
        body.insert("top_p".to_owned(), milli_to_number(top_p)?);
        sampled = true;
    }
    if sampled {
        body.insert("do_sample".to_owned(), Value::Bool(true));
    }
    Ok(())
}

/// The `thinking` object and the model-gated effort rung.
fn reasoning_fields(
    view: &RequestView<'_>,
    body: &mut serde_json::Map<String, Value>,
) -> Result<(), RequestBuildError> {
    match view.reasoning {
        // Thinking is on by default; sending nothing is how "take the provider
        // default" is expressed.
        ReasoningRequest::ProviderDefault => {}
        ReasoningRequest::Disabled => {
            body.insert("thinking".to_owned(), json!({ "type": "disabled" }));
        }
        ReasoningRequest::Enabled {
            budget_tokens,
            effort,
        } => {
            if budget_tokens.is_some() {
                // Z.AI's `thinking` object takes no token budget at all, so a
                // requested one is refused rather than silently dropped.
                return Err(RequestBuildError::ReasoningBudgetOutOfRange { min: 0, max: 0 });
            }
            body.insert("thinking".to_owned(), json!({ "type": "enabled" }));
            if let Some(effort) = effort {
                body.insert(
                    "reasoning_effort".to_owned(),
                    Value::String(effort_token(effort).to_owned()),
                );
            }
        }
    }
    Ok(())
}

/// The neutral effort ladder onto Z.AI's rungs.
///
/// Z.AI publishes no effort enum, so the ladder collapses onto the
/// `OpenAI`-compatible four and the top rung folds to `high` rather than
/// inventing a value the server would reject as an invalid parameter.
const fn effort_token(effort: ReasoningEffort) -> &'static str {
    match effort {
        ReasoningEffort::Minimal => "minimal",
        ReasoningEffort::Low => "low",
        ReasoningEffort::Medium => "medium",
        ReasoningEffort::High | ReasoningEffort::Max => "high",
    }
}

/// Integer milli-units onto the float the wire takes.
fn milli_to_number(milli: u16) -> Result<Value, RequestBuildError> {
    serde_json::Number::from_f64(f64::from(milli) / 1000.0)
        .map(Value::Number)
        .ok_or(RequestBuildError::Encoding {
            reason: "a sampling value is not representable as JSON",
        })
}

/// The `messages` array, system blocks first.
fn build_messages(view: &RequestView<'_>) -> Result<Vec<Value>, RequestBuildError> {
    let mut out = Vec::with_capacity(view.messages.len() + 1);
    if !view.system.is_empty() {
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
            Role::User => push_user(&message.blocks, &mut out)?,
            Role::Assistant => out.push(assistant_message(&message.blocks)?),
        }
    }
    if out.is_empty() {
        return Err(RequestBuildError::Encoding {
            reason: "a chat request must carry at least one message",
        });
    }
    Ok(out)
}

/// A user turn: tool results become their own `tool`-role messages, in front of
/// whatever prose the same turn carried, because they answer the assistant turn
/// before them.
fn push_user(blocks: &[CanonicalBlock], out: &mut Vec<Value>) -> Result<(), RequestBuildError> {
    let mut text = String::new();
    for block in blocks {
        match block {
            CanonicalBlock::ToolResult { call, content, .. } => {
                // Z.AI's tool message has no error flag. AEX does not
                // synthesize prose for `is_error`, because inventing text would
                // alter model-visible history.
                out.push(json!({
                    "role": "tool",
                    "tool_call_id": call.as_str(),
                    "content": tool_result_text(content),
                }));
            }
            CanonicalBlock::Text { text: body, .. } => push_joined(&mut text, body.as_str()),
            CanonicalBlock::Reasoning(_)
            | CanonicalBlock::ToolUse { .. }
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

/// An assistant turn, replaying `reasoning_content` and `tool_calls`.
fn assistant_message(blocks: &[CanonicalBlock]) -> Result<Value, RequestBuildError> {
    let mut text = String::new();
    let mut reasoning = String::new();
    let mut calls = Vec::new();
    for block in blocks {
        match block {
            CanonicalBlock::Text { text: body, .. } => push_joined(&mut text, body.as_str()),
            CanonicalBlock::Refusal { text: body } => push_joined(&mut text, body.as_str()),
            CanonicalBlock::Reasoning(reason) => {
                if let Some(token) = &reason.token
                    && token.provenance != ProviderId::Zai
                {
                    return Err(RequestBuildError::ReasoningProvenanceMismatch {
                        expected: ProviderId::Zai,
                        found: token.provenance,
                    });
                }
                match &reason.body {
                    ReasoningBody::Text { text: body } | ReasoningBody::Summary { text: body } => {
                        push_joined(&mut reasoning, body.as_str());
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
    let mut message = serde_json::Map::new();
    message.insert("role".to_owned(), Value::String("assistant".to_owned()));
    message.insert("content".to_owned(), Value::String(text));
    if !reasoning.is_empty() {
        message.insert("reasoning_content".to_owned(), Value::String(reasoning));
    }
    if !calls.is_empty() {
        message.insert("tool_calls".to_owned(), Value::Array(calls));
    }
    Ok(Value::Object(message))
}

/// The `OpenAI`-nested function shape.
fn build_tools(
    entry: &ModelEntry,
    view: &RequestView<'_>,
) -> Result<Vec<Value>, RequestBuildError> {
    let mut out = Vec::with_capacity(view.tools.len());
    for tool in view.tools {
        let name = tool.name.as_str();
        if name.len() > usize::from(entry.tool_policy.max_name_bytes)
            || !entry.tool_policy.name_pattern.accepts(name)
        {
            return Err(RequestBuildError::ToolNameInvalid {
                name: tool.name.clone(),
            });
        }
        let mut function = serde_json::Map::new();
        function.insert("name".to_owned(), Value::String(name.to_owned()));
        function.insert(
            "description".to_owned(),
            Value::String(tool.description.as_str().to_owned()),
        );
        function.insert("parameters".to_owned(), tool.input_schema.to_value());
        if tool.strict {
            function.insert("strict".to_owned(), Value::Bool(true));
        }
        out.push(json!({ "type": "function", "function": Value::Object(function) }));
    }
    Ok(out)
}

fn tool_result_text(parts: &[ToolResultPart]) -> String {
    let mut out = String::new();
    for part in parts {
        match part {
            ToolResultPart::Text { text } => push_joined(&mut out, text.as_str()),
            ToolResultPart::Json { value } => push_joined(&mut out, value.as_str()),
        }
    }
    out
}

fn push_joined(target: &mut String, piece: &str) {
    if !target.is_empty() {
        target.push('\n');
    }
    target.push_str(piece);
}

// ---------------------------------------------------------------------------
// stream decoding
// ---------------------------------------------------------------------------

/// What one `finish_reason` token means.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ZaiFinish {
    /// A terminal stop reason.
    Stop(StopReason),
    /// A terminal failure. Z.AI publishes no mid-stream error code, so these
    /// three tokens are the whole of its in-stream failure vocabulary.
    Failure(ProviderFailureKind),
}

/// The complete finish-token table (plan 08 §5.4).
fn resolve_finish(token: &str) -> Option<ZaiFinish> {
    let resolved = match token {
        "stop" => ZaiFinish::Stop(StopReason::EndTurn),
        "tool_calls" => ZaiFinish::Stop(StopReason::ToolUse),
        "length" => ZaiFinish::Stop(StopReason::MaxOutputTokens),
        "sensitive" => ZaiFinish::Failure(ProviderFailureKind::ContentFiltered),
        "model_context_window_exceeded" => ZaiFinish::Failure(ProviderFailureKind::ContextOverflow),
        "network_error" => ZaiFinish::Failure(ProviderFailureKind::Transport),
        _ => return None,
    };
    Some(resolved)
}

/// What one `choices[]` element did.
enum ChoiceOutcome {
    /// The choice is still open.
    Open,
    /// The choice carried a terminal stop reason.
    Terminal,
    /// The choice carried one of the three failing finish tokens.
    Failed(Box<ProviderFailure>),
}

/// Decodes one data-only frame.
fn decode_event(
    state: &mut DialectState,
    event: &SseEvent<'_>,
    budget: &StreamBudget,
) -> Result<FrameOutcome, FrameDecodeError> {
    if let Some(name) = event.name {
        // This dialect is data-only. A named event means the response is not
        // the one this adapter asked for.
        return Err(FrameDecodeError::UnknownEvent {
            event: BoundedString::truncating(name),
        });
    }
    if event.data.is_empty() {
        return Ok(FrameOutcome::Ignored);
    }
    if event.is_done_sentinel() {
        return if state.terminal {
            Ok(FrameOutcome::Ignored)
        } else {
            Err(FrameDecodeError::OutOfOrder {
                reason: "the [DONE] sentinel arrived before any finish_reason",
            })
        };
    }
    state
        .ledger
        .charge_response(budget, u64::try_from(event.data.len()).unwrap_or(u64::MAX))?;
    state.ledger.count_frame();
    if state.terminal {
        return Err(FrameDecodeError::OutOfOrder {
            reason: "a chunk arrived after the finish_reason chunk",
        });
    }

    let value: Value = serde_json::from_slice(event.data).map_err(|_| FrameDecodeError::NotJson)?;
    let Some(chunk) = value.as_object() else {
        return Err(FrameDecodeError::NotJson);
    };

    if let Some(echoed) = chunk.get("request_id").and_then(Value::as_str) {
        state.request_id =
            Some(
                ProviderRequestId::new(echoed).map_err(|_| FrameDecodeError::MalformedField {
                    field: "request_id",
                })?,
            );
    }
    if let Some(usage) = chunk.get("usage").filter(|usage| !usage.is_null()) {
        state.usage = normalize_usage(usage)?;
    }

    let Some(choices) = chunk.get("choices").and_then(Value::as_array) else {
        return Err(FrameDecodeError::MalformedField { field: "choices" });
    };
    if choices.is_empty() {
        // Only a chunk carrying `choices[0]` proves the provider is generating.
        return Ok(FrameOutcome::Ignored);
    }

    let started = state.mark_started();
    let mut terminal = false;
    for choice in choices {
        match decode_choice(state, budget, choice)? {
            ChoiceOutcome::Open => {}
            ChoiceOutcome::Terminal => terminal = true,
            ChoiceOutcome::Failed(failure) => return Ok(FrameOutcome::Failed(failure)),
        }
    }
    if terminal {
        return Ok(FrameOutcome::Terminal);
    }
    Ok(started)
}

/// Decodes one `choices[]` element.
fn decode_choice(
    state: &mut DialectState,
    budget: &StreamBudget,
    choice: &Value,
) -> Result<ChoiceOutcome, FrameDecodeError> {
    let Some(choice) = choice.as_object() else {
        return Err(FrameDecodeError::MalformedField { field: "choices[]" });
    };
    let index = match choice.get("index").filter(|index| !index.is_null()) {
        None => 0u16,
        Some(index) => index
            .as_u64()
            .and_then(|index| u16::try_from(index).ok())
            .ok_or(FrameDecodeError::MalformedField {
                field: "choices[].index",
            })?,
    };

    if let Some(delta) = choice.get("delta").filter(|delta| !delta.is_null()) {
        let Some(delta) = delta.as_object() else {
            return Err(FrameDecodeError::MalformedField {
                field: "choices[].delta",
            });
        };
        decode_text(state, budget, index, delta)?;
        if let Some(calls) = delta.get("tool_calls").filter(|calls| !calls.is_null()) {
            let Some(calls) = calls.as_array() else {
                return Err(FrameDecodeError::MalformedField {
                    field: "choices[].delta.tool_calls",
                });
            };
            for call in calls {
                decode_tool_call(state, budget, call)?;
            }
        }
    }

    let Some(finish) = choice.get("finish_reason").filter(|token| !token.is_null()) else {
        return Ok(ChoiceOutcome::Open);
    };
    let Some(token) = finish.as_str() else {
        return Err(FrameDecodeError::MalformedField {
            field: "choices[].finish_reason",
        });
    };
    let Some(resolved) = resolve_finish(token) else {
        return Err(FrameDecodeError::MalformedField {
            field: "choices[].finish_reason",
        });
    };
    state.finish_token = Some(token.to_owned());
    state.terminal = true;
    match resolved {
        ZaiFinish::Stop(_) => Ok(ChoiceOutcome::Terminal),
        ZaiFinish::Failure(kind) => Ok(ChoiceOutcome::Failed(Box::new(failure_from_finish(
            kind, token,
        )))),
    }
}

/// `content` and `reasoning_content` accumulation.
fn decode_text(
    state: &mut DialectState,
    budget: &StreamBudget,
    index: u16,
    delta: &serde_json::Map<String, Value>,
) -> Result<(), FrameDecodeError> {
    if let Some(content) = delta.get("content").filter(|content| !content.is_null()) {
        let text = content.as_str().ok_or(FrameDecodeError::MalformedField {
            field: "choices[].delta.content",
        })?;
        if !text.is_empty() {
            if !state.open_text.contains_key(&index) {
                state.ledger.open_block(budget)?;
            }
            state
                .ledger
                .charge_text(budget, u64::try_from(text.len()).unwrap_or(u64::MAX))?;
            state.open_text.entry(index).or_default().push_str(text);
        }
    }
    if let Some(thought) = delta
        .get("reasoning_content")
        .filter(|thought| !thought.is_null())
    {
        let text = thought.as_str().ok_or(FrameDecodeError::MalformedField {
            field: "choices[].delta.reasoning_content",
        })?;
        if !text.is_empty() {
            if !state.open_reasoning.contains_key(&index) {
                state.ledger.open_block(budget)?;
            }
            state
                .ledger
                .charge_reasoning(budget, u64::try_from(text.len()).unwrap_or(u64::MAX))?;
            state
                .open_reasoning
                .entry(index)
                .or_default()
                .push_str(text);
        }
    }
    Ok(())
}

/// One `tool_calls[]` fragment, reassembled by its own `index`.
fn decode_tool_call(
    state: &mut DialectState,
    budget: &StreamBudget,
    call: &Value,
) -> Result<(), FrameDecodeError> {
    let Some(call) = call.as_object() else {
        return Err(FrameDecodeError::MalformedField {
            field: "choices[].delta.tool_calls[]",
        });
    };
    if let Some(kind) = call.get("type").and_then(Value::as_str)
        && kind != "function"
    {
        // `web_search` and `retrieval` exist upstream and are never emitted, so
        // receiving one means the request was altered in flight (D-25).
        return Err(FrameDecodeError::UnknownEvent {
            event: BoundedString::truncating(kind),
        });
    }
    let slot = call
        .get("index")
        .and_then(Value::as_u64)
        .and_then(|slot| u16::try_from(slot).ok())
        .ok_or(FrameDecodeError::MalformedField {
            field: "choices[].delta.tool_calls[].index",
        })?;
    if !state.open_tools.contains_key(&slot) {
        state.ledger.open_tool_call(budget)?;
        state.ledger.open_block(budget)?;
    }
    let partial = state.open_tools.entry(slot).or_default();
    if let Some(id) = call.get("id").and_then(Value::as_str)
        && !id.is_empty()
        && partial.id.is_none()
    {
        partial.id = Some(
            ToolCallId::new(id).map_err(|_| FrameDecodeError::MalformedField {
                field: "choices[].delta.tool_calls[].id",
            })?,
        );
    }
    let Some(function) = call.get("function").filter(|value| !value.is_null()) else {
        return Ok(());
    };
    let Some(function) = function.as_object() else {
        return Err(FrameDecodeError::MalformedField {
            field: "choices[].delta.tool_calls[].function",
        });
    };
    if let Some(name) = function.get("name").and_then(Value::as_str)
        && !name.is_empty()
        && partial.name.is_none()
    {
        partial.name = Some(name.to_owned());
    }
    let Some(arguments) = function
        .get("arguments")
        .filter(|arguments| !arguments.is_null())
    else {
        return Ok(());
    };
    match arguments {
        // The guide's shape: a fragment of a JSON string.
        Value::String(fragment) => partial.arguments.push_str(fragment),
        // The response schema's shape: the whole object at once.
        whole => {
            if !partial.arguments.is_empty() {
                return Err(FrameDecodeError::OutOfOrder {
                    reason: "whole tool arguments arrived after a fragment",
                });
            }
            partial.arguments =
                serde_json::to_string(whole).map_err(|_| FrameDecodeError::MalformedField {
                    field: "choices[].delta.tool_calls[].function.arguments",
                })?;
        }
    }
    let named = partial
        .id
        .clone()
        .unwrap_or_else(|| BoundedString::truncating(""));
    BudgetLedger::check_tool_arguments(budget, &named, partial.arguments.len())?;
    Ok(())
}

/// Z.AI's usage block onto the neutral tally.
///
/// `prompt_tokens_details.cached_tokens` is a **subset** of `prompt_tokens`, so
/// the uncached remainder is what `input_tokens` carries.
fn normalize_usage(value: &Value) -> Result<NormalizedUsage, FrameDecodeError> {
    let Some(usage) = value.as_object() else {
        return Err(FrameDecodeError::MalformedField { field: "usage" });
    };
    let prompt = read_count(usage, "prompt_tokens")?;
    let completion = read_count(usage, "completion_tokens")?;
    let total = read_count(usage, "total_tokens")?;
    let cached = usage
        .get("prompt_tokens_details")
        .and_then(Value::as_object)
        .and_then(|details| details.get("cached_tokens"))
        .and_then(Value::as_u64)
        .unwrap_or(0);

    let mut missing = UsageFieldSet::EMPTY;
    if prompt.is_none() {
        missing = missing.with(UsageField::InputTokens);
    }
    if completion.is_none() {
        missing = missing.with(UsageField::OutputTokens);
    }
    if total.is_none() {
        missing = missing.with(UsageField::ProviderTotalTokens);
    }
    let prompt = prompt.unwrap_or(0);
    let cached = cached.min(prompt);
    Ok(NormalizedUsage {
        input_tokens: prompt.saturating_sub(cached),
        cache_read_input_tokens: cached,
        cache_write_input_tokens: 0,
        output_tokens: completion.unwrap_or(0),
        reasoning_tokens: 0,
        tool_use_prompt_tokens: 0,
        provider_total_tokens: total,
        completeness: if missing.is_empty() {
            UsageCompleteness::Exact
        } else {
            UsageCompleteness::Partial { missing }
        },
    })
}

fn read_count(
    usage: &serde_json::Map<String, Value>,
    field: &'static str,
) -> Result<Option<u64>, FrameDecodeError> {
    match usage.get(field).filter(|count| !count.is_null()) {
        None => Ok(None),
        Some(count) => count
            .as_u64()
            .map(Some)
            .ok_or(FrameDecodeError::MalformedField { field }),
    }
}

/// The failure one of the three failing finish tokens carries.
fn failure_from_finish(kind: ProviderFailureKind, token: &str) -> ProviderFailure {
    let message = redact::<512>(
        &format!("the stream ended with finish_reason `{token}`"),
        &[],
    );
    ProviderFailure::new(
        RedactedDetail::new(kind, message).with_code(redact::<64>(token, &[]).as_str()),
    )
}

/// Assembles the canonical block set: reasoning, then prose, then tool calls.
fn seal_state(state: DialectState) -> Result<SealedResponse, FrameDecodeError> {
    let DialectState {
        open_text,
        open_reasoning,
        open_tools,
        finish_token,
        usage,
        request_id,
        terminal,
        ..
    } = state;
    if !terminal {
        return Err(FrameDecodeError::OutOfOrder {
            reason: "the stream ended without a finish_reason",
        });
    }
    let Some(token) = finish_token else {
        return Err(FrameDecodeError::OutOfOrder {
            reason: "the stream ended without a finish_reason",
        });
    };
    let stop_reason = match resolve_finish(&token) {
        Some(ZaiFinish::Stop(reason)) => reason,
        Some(ZaiFinish::Failure(_)) => {
            return Err(FrameDecodeError::OutOfOrder {
                reason: "a failing finish_reason cannot seal a response",
            });
        }
        None => {
            return Err(FrameDecodeError::MalformedField {
                field: "choices[].finish_reason",
            });
        }
    };

    let mut blocks = Vec::new();
    for text in open_reasoning.into_values() {
        if text.is_empty() {
            continue;
        }
        blocks.push(reasoning_block(text)?);
    }
    for text in open_text.into_values() {
        if text.is_empty() {
            continue;
        }
        let body = BoundedString::new(text).map_err(|_| {
            FrameDecodeError::Budget(BudgetOverrun::Text {
                limit: u64::try_from(TEXT_MAX).unwrap_or(u64::MAX),
            })
        })?;
        blocks.push(CanonicalBlock::Text {
            text: body,
            annotations: Vec::new(),
        });
    }
    for call in open_tools.into_values() {
        blocks.push(tool_use_block(call)?);
    }

    Ok(SealedResponse {
        blocks,
        stop_reason,
        usage,
        provider_request_id: request_id,
    })
}

/// Z.AI's round-trip material is the reasoning text itself, which is what
/// `clear_thinking: false` replays on the next turn.
fn reasoning_block(text: String) -> Result<CanonicalBlock, FrameDecodeError> {
    let body = BoundedString::new(text.as_str()).map_err(|_| {
        FrameDecodeError::Budget(BudgetOverrun::Reasoning {
            limit: u64::try_from(REASON_MAX).unwrap_or(u64::MAX),
        })
    })?;
    Ok(CanonicalBlock::Reasoning(ReasoningBlock {
        body: ReasoningBody::Text { text: body },
        token: Some(ReasoningToken {
            provenance: ProviderId::Zai,
            bytes: bytes::Bytes::from(text.into_bytes()),
        }),
    }))
}

fn tool_use_block(call: PartialToolCall) -> Result<CanonicalBlock, FrameDecodeError> {
    let Some(id) = call.id else {
        return Err(FrameDecodeError::MalformedField {
            field: "choices[].delta.tool_calls[].id",
        });
    };
    let Some(name) = call.name else {
        return Err(FrameDecodeError::MalformedField {
            field: "choices[].delta.tool_calls[].function.name",
        });
    };
    let name = ToolName::parse(&name).map_err(|_| FrameDecodeError::MalformedField {
        field: "choices[].delta.tool_calls[].function.name",
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

// ---------------------------------------------------------------------------
// HTTP failures
// ---------------------------------------------------------------------------

/// Classifies a non-2xx response.
///
/// The documented body is `{"error":{"code":"1214","message":"…"}}` — the code
/// is a **string**. A numeric code is tolerated as well, because reading it as
/// absent would fall back to the status and reclassify a permanent quota
/// failure as a transient server error, which §6.4 forbids retrying.
fn classify(status: u16, body: &BoundedBody) -> ProviderFailure {
    let parsed = body.as_json();
    let error = parsed
        .as_ref()
        .and_then(|value| value.get("error"))
        .and_then(Value::as_object);
    let code = error
        .and_then(|error| error.get("code"))
        .and_then(code_text);
    let message = error
        .and_then(|error| error.get("message"))
        .and_then(Value::as_str);

    let kind = code
        .as_deref()
        .and_then(kind_for_code)
        .unwrap_or_else(|| kind_for_status(status));
    let rendered = message.map_or_else(
        || BoundedString::truncating("the provider returned no readable error body"),
        |message| redact::<512>(message, &[]),
    );
    let mut detail = RedactedDetail::new(kind, rendered).with_status(status);
    if let Some(code) = &code {
        detail = detail.with_code(redact::<64>(code, &[]).as_str());
    }
    let rate_limit = if code.as_deref().is_some_and(is_backpressure_code) {
        RateLimitFeedback {
            source: RateLimitSource::ErrorBody,
            ..RateLimitFeedback::none()
        }
    } else {
        RateLimitFeedback::none()
    };
    ProviderFailure { detail, rate_limit }
}

fn code_text(value: &Value) -> Option<String> {
    match value {
        Value::String(code) => Some(code.clone()),
        Value::Number(code) => Some(code.to_string()),
        _ => None,
    }
}

/// The documented Z.AI code table.
fn kind_for_code(code: &str) -> Option<ProviderFailureKind> {
    let kind = match code {
        "1000" | "1001" | "1003" | "1220" => ProviderFailureKind::Authentication,
        "1113" => ProviderFailureKind::Billing,
        "1210" | "1212" | "1213" | "1214" | "1215" => ProviderFailureKind::InvalidRequest,
        "1211" => ProviderFailureKind::ModelNotFound,
        "1261" => ProviderFailureKind::ContextOverflow,
        "1301" => ProviderFailureKind::ContentFiltered,
        "1302" => ProviderFailureKind::RateLimited,
        "1305" => ProviderFailureKind::Overloaded,
        other => match other.parse::<u32>() {
            Ok(numeric) if (1308..=1321).contains(&numeric) => ProviderFailureKind::Quota,
            _ => return None,
        },
    };
    Some(kind)
}

/// Whether the code is the only backpressure signal this provider publishes.
fn is_backpressure_code(code: &str) -> bool {
    matches!(code, "1302" | "1305")
        || matches!(code.parse::<u32>(), Ok(numeric) if (1308..=1321).contains(&numeric))
}

fn kind_for_status(status: u16) -> ProviderFailureKind {
    match status {
        401 | 403 => ProviderFailureKind::Authentication,
        402 => ProviderFailureKind::Billing,
        404 => ProviderFailureKind::ModelNotFound,
        429 => ProviderFailureKind::RateLimited,
        503 => ProviderFailureKind::Overloaded,
        504 => ProviderFailureKind::Timeout,
        500..=599 => ProviderFailureKind::ServerError,
        _ => ProviderFailureKind::InvalidRequest,
    }
}

#[cfg(test)]
mod tests {
    use aex_model_catalog::canonical::{
        CacheBreakpoint, CanonicalBlock, CanonicalMessage, CanonicalToolDef, CorrelationId,
        ReasoningBlock, ReasoningBody, ReasoningEffort, ReasoningRequest, ReasoningToken, Role,
        StopReason, StructuredOutputRequest, SystemBlock, ToolChoice, ToolResultPart,
        UsageCompleteness, UsageField, UsageFieldSet,
    };
    use aex_model_catalog::document::{
        Capability, CapabilitySet, EndpointPin, ModelEntry, ReasoningEncoding, ReasoningMode,
        ReasoningPolicy, ReasoningReplay, SamplingSupport, StopReasonMap, StreamUsageDelivery,
        StructuredOutputPolicy,
    };
    use aex_model_catalog::fixture;
    use aex_model_catalog::primitives::{BoundedString, ToolCallId, ToolName};
    use aex_wire::CanonicalJson;
    use aex_wire::provider::ProviderId;
    use serde_json::{Value, json};

    use super::{
        ACCEPT_LANGUAGE, CHAT_COMPLETIONS_PATH, RequestView, ZaiAdapter, ZaiFinish, build,
        fresh_state, is_backpressure_code, kind_for_code, kind_for_status, resolve_finish,
    };
    use crate::adapter::{
        BoundedBody, DialectState, FrameDecodeError, FrameOutcome, HeaderView, ProviderAdapter,
        RequestBuildError,
    };

    use crate::budget::StreamBudget;
    use crate::error::{ProviderFailureKind, RateLimitSource};
    use crate::sse::SseEvent;
    use crate::transport::{Accept, AuthScheme, WireRequest};

    // -----------------------------------------------------------------------
    // harness
    //
    // `QualifiedModel::new` is crate-private to `aex-model-catalog` and
    // `Catalog::load` demands a real P-256 signature this crate has no
    // dependency to produce, so the tests drive the pure builder through the
    // same `ModelEntry` a qualified pair would carry. `build_request` is a
    // one-line delegation onto it.
    // -----------------------------------------------------------------------

    /// Everything a Z.AI pair can declare.
    fn full_capabilities() -> CapabilitySet {
        CapabilitySet::from_slice(&[
            Capability::TextIn,
            Capability::TextOut,
            Capability::Streaming,
            Capability::Tools,
            Capability::ParallelTools,
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

    /// The launch entry shape for `zai` / `glm-5.2`.
    fn entry() -> ModelEntry {
        let mut entry = fixture::entry(ProviderId::Zai, "glm-5.2", full_capabilities());
        entry.sampling = SamplingSupport::Full;
        entry.structured_output = StructuredOutputPolicy::JsonObjectOnly;
        entry.tool_policy.requires_stream_opt_in = true;
        entry.reasoning = ReasoningPolicy {
            mode: ReasoningMode::Optional,
            encoding: ReasoningEncoding::ZaiThinking,
            replay: ReasoningReplay::RecommendedEcho,
            excludes_sampling: false,
        };
        entry.usage_map.stream_usage_delivery = StreamUsageDelivery::LastContentChunk;
        entry.stop_reason_map = zai_stop_map();
        entry
    }

    /// The catalog rendering of the same table [`resolve_finish`] compiles.
    fn zai_stop_map() -> StopReasonMap {
        use aex_model_catalog::document::StopFailureMapping;
        StopReasonMap {
            end_turn: vec![fixture::bounded("stop")],
            tool_use: vec![fixture::bounded("tool_calls")],
            max_output_tokens: vec![fixture::bounded("length")],
            stop_sequence: Vec::new(),
            refusal: Vec::new(),
            failure: vec![
                StopFailureMapping {
                    token: fixture::bounded("sensitive"),
                    kind: ProviderFailureKind::ContentFiltered,
                },
                StopFailureMapping {
                    token: fixture::bounded("model_context_window_exceeded"),
                    kind: ProviderFailureKind::ContextOverflow,
                },
                StopFailureMapping {
                    token: fixture::bounded("network_error"),
                    kind: ProviderFailureKind::Transport,
                },
            ],
        }
    }

    fn correlation() -> CorrelationId {
        CorrelationId::from_effect([0xab; 16])
    }

    fn text_block(text: &str) -> CanonicalBlock {
        CanonicalBlock::Text {
            text: BoundedString::truncating(text),
            annotations: Vec::new(),
        }
    }

    fn user(text: &str) -> CanonicalMessage {
        CanonicalMessage {
            role: Role::User,
            blocks: vec![text_block(text)],
        }
    }

    fn tool(name: &str) -> CanonicalToolDef {
        CanonicalToolDef {
            name: ToolName::parse(name).expect("a tool name"),
            description: BoundedString::truncating("looks something up"),
            input_schema: CanonicalJson::parse(r#"{"type":"object"}"#).expect("a schema"),
            strict: false,
        }
    }

    /// Owned storage for a [`RequestView`].
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
        cache_breakpoints: Vec<CacheBreakpoint>,
        correlation: CorrelationId,
    }

    impl Draft {
        fn new() -> Self {
            Self {
                system: Vec::new(),
                messages: vec![user("hello")],
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
                correlation: correlation(),
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
                cache_breakpoints: &self.cache_breakpoints,
                correlation: &self.correlation,
            }
        }

        fn build(&self) -> Result<WireRequest, RequestBuildError> {
            build(&entry(), &self.view())
        }

        fn build_with(&self, entry: &ModelEntry) -> Result<WireRequest, RequestBuildError> {
            build(entry, &self.view())
        }

        fn body(&self) -> Value {
            let request = self.build().expect("the draft builds");
            serde_json::from_slice(&request.body).expect("the body is JSON")
        }
    }

    // -----------------------------------------------------------------------
    // request goldens
    // -----------------------------------------------------------------------

    #[test]
    fn the_request_targets_the_compiled_general_endpoint_and_path() {
        let request = Draft::new().build().expect("builds");
        assert_eq!(request.endpoint, EndpointPin::ZaiPaasV4);
        assert_eq!(request.path.as_str(), CHAT_COMPLETIONS_PATH);
        assert_eq!(request.auth, AuthScheme::BearerAuthorization);
        assert_eq!(request.accept, Accept::TextEventStream);
        assert!(
            request.query.is_empty(),
            "no query parameter is compiled in"
        );
        let url = request.url().expect("assembles");
        assert_eq!(url.host_str(), Some("api.z.ai"));
        assert!(
            !url.as_str().contains("coding/paas") && !url.as_str().contains("api/anthropic"),
            "the launch exclusions must be unreachable: {url}"
        );
    }

    #[test]
    fn the_only_non_secret_header_is_the_pinned_accept_language() {
        let request = Draft::new().build().expect("builds");
        assert_eq!(request.headers.len(), 1);
        assert_eq!(request.headers[0].0, ACCEPT_LANGUAGE.0);
        assert_eq!(request.headers[0].1.as_str(), "en-US,en");
    }

    #[test]
    fn the_request_id_is_the_derived_correlation_id() {
        let draft = Draft::new();
        let body = draft.body();
        assert_eq!(
            body["request_id"].as_str(),
            Some(draft.correlation.as_str()),
            "request_id is the only correlation handle this provider offers"
        );
        assert!(draft.correlation.as_str().starts_with("aex-"));
    }

    #[test]
    fn the_stream_is_always_on_and_clear_thinking_is_pinned_false() {
        let body = Draft::new().body();
        assert_eq!(body["stream"], json!(true));
        assert_eq!(
            body["clear_thinking"],
            json!(false),
            "prior reasoning_content must be replayed, not cleared"
        );
        assert_eq!(body["model"], json!("glm-5.2"));
        assert_eq!(body["max_tokens"], json!(1_024));
    }

    #[test]
    fn stream_options_is_never_sent() {
        let mut draft = Draft::new();
        draft.tools.push(tool("lookup"));
        let body = draft.body();
        assert!(
            body.get("stream_options").is_none(),
            "usage arrives on the last content chunk, so the flag is never sent"
        );
        assert!(body.get("user_id").is_none(), "no caller identity is sent");
    }

    #[test]
    fn tools_are_nested_with_tool_stream_enabled_and_tool_choice_auto() {
        let mut draft = Draft::new();
        draft.tools.push(tool("lookup"));
        let body = draft.body();
        assert_eq!(body["tool_stream"], json!(true));
        assert_eq!(body["tool_choice"], json!("auto"));
        assert_eq!(body["tools"][0]["type"], json!("function"));
        assert_eq!(body["tools"][0]["function"]["name"], json!("lookup"));
        assert_eq!(
            body["tools"][0]["function"]["parameters"],
            json!({ "type": "object" })
        );
    }

    #[test]
    fn a_request_without_tools_sends_neither_tool_stream_nor_tool_choice() {
        let body = Draft::new().body();
        assert!(body.get("tools").is_none());
        assert!(body.get("tool_stream").is_none());
        assert!(body.get("tool_choice").is_none());
    }

    #[test]
    fn sampling_is_sent_in_provider_units_alongside_do_sample() {
        let mut draft = Draft::new();
        draft.temperature_milli = Some(600);
        draft.top_p_milli = Some(950);
        let body = draft.body();
        assert_eq!(body["temperature"], json!(0.6));
        assert_eq!(body["top_p"], json!(0.95));
        assert_eq!(body["do_sample"], json!(true));
    }

    #[test]
    fn an_unsampled_request_omits_do_sample_rather_than_disabling_sampling() {
        let body = Draft::new().body();
        assert!(body.get("do_sample").is_none());
        assert!(body.get("temperature").is_none());
        assert!(body.get("top_p").is_none());
    }

    #[test]
    fn thinking_is_omitted_by_default_and_explicit_otherwise() {
        assert!(Draft::new().body().get("thinking").is_none());

        let mut off = Draft::new();
        off.reasoning = ReasoningRequest::Disabled;
        assert_eq!(off.body()["thinking"], json!({ "type": "disabled" }));

        let mut on = Draft::new();
        on.reasoning = ReasoningRequest::Enabled {
            budget_tokens: None,
            effort: Some(ReasoningEffort::Max),
        };
        let body = on.body();
        assert_eq!(body["thinking"], json!({ "type": "enabled" }));
        assert_eq!(body["reasoning_effort"], json!("high"));
    }

    #[test]
    fn a_prior_assistant_turn_replays_reasoning_content_and_tool_calls() {
        let mut draft = Draft::new();
        draft.messages.push(CanonicalMessage {
            role: Role::Assistant,
            blocks: vec![
                CanonicalBlock::Reasoning(ReasoningBlock {
                    body: ReasoningBody::Text {
                        text: BoundedString::truncating("weighing options"),
                    },
                    token: Some(ReasoningToken {
                        provenance: ProviderId::Zai,
                        bytes: bytes::Bytes::from_static(b"weighing options"),
                    }),
                }),
                CanonicalBlock::ToolUse {
                    id: ToolCallId::new("call_1").expect("id"),
                    name: ToolName::parse("lookup").expect("name"),
                    input: CanonicalJson::parse(r#"{"q":"aex"}"#).expect("input"),
                },
            ],
        });
        let body = draft.body();
        let turn = &body["messages"][1];
        assert_eq!(turn["role"], json!("assistant"));
        assert_eq!(turn["reasoning_content"], json!("weighing options"));
        assert_eq!(turn["tool_calls"][0]["id"], json!("call_1"));
        assert_eq!(turn["tool_calls"][0]["type"], json!("function"));
        assert_eq!(
            turn["tool_calls"][0]["function"]["arguments"],
            json!(r#"{"q":"aex"}"#),
            "the request side carries arguments as a JSON string"
        );
    }

    #[test]
    fn a_tool_result_becomes_a_tool_role_message_ahead_of_the_user_prose() {
        let mut draft = Draft::new();
        draft.messages = vec![CanonicalMessage {
            role: Role::User,
            blocks: vec![
                CanonicalBlock::ToolResult {
                    call: ToolCallId::new("call_1").expect("id"),
                    content: vec![ToolResultPart::Text {
                        text: BoundedString::truncating("42"),
                    }],
                    is_error: false,
                },
                text_block("and now summarize"),
            ],
        }];
        let body = draft.body();
        assert_eq!(body["messages"][0]["role"], json!("tool"));
        assert_eq!(body["messages"][0]["tool_call_id"], json!("call_1"));
        assert_eq!(body["messages"][0]["content"], json!("42"));
        assert_eq!(body["messages"][1]["role"], json!("user"));
    }

    #[test]
    fn system_blocks_join_into_one_system_message() {
        let mut draft = Draft::new();
        draft.system = vec![
            SystemBlock {
                text: BoundedString::truncating("be terse"),
                cacheable: true,
            },
            SystemBlock {
                text: BoundedString::truncating("cite sources"),
                cacheable: false,
            },
        ];
        let body = draft.body();
        assert_eq!(body["messages"][0]["role"], json!("system"));
        assert_eq!(
            body["messages"][0]["content"],
            json!("be terse\n\ncite sources")
        );
    }

    #[test]
    fn a_json_object_request_becomes_the_only_supported_response_format() {
        let mut draft = Draft::new();
        draft.structured_output = Some(StructuredOutputRequest::JsonObject);
        assert_eq!(
            draft.body()["response_format"],
            json!({ "type": "json_object" })
        );
    }

    #[test]
    fn stop_sequences_are_sent_verbatim() {
        let mut draft = Draft::new();
        draft.stop_sequences = vec![fixture::bounded("</done>"), fixture::bounded("STOP")];
        assert_eq!(draft.body()["stop"], json!(["</done>", "STOP"]));
    }

    #[test]
    fn the_adapter_speaks_for_zai_and_pins_its_own_source_digest() {
        assert_eq!(ZaiAdapter.provider(), ProviderId::Zai);
        assert_eq!(ZaiAdapter.source_digest(), ZaiAdapter.source_digest());
        assert_ne!(
            ZaiAdapter.source_digest(),
            fixture::adapter("fixture-adapter"),
            "the compiled tag is the adapter's own, not the fixture's"
        );
    }

    // -----------------------------------------------------------------------
    // capability gates — every one fires before a socket exists
    // -----------------------------------------------------------------------

    #[test]
    fn tool_choice_required_named_and_none_are_all_refused() {
        for requested in [
            ToolChoice::Required,
            ToolChoice::None,
            ToolChoice::Named {
                name: ToolName::parse("lookup").expect("name"),
            },
        ] {
            let mut draft = Draft::new();
            draft.tool_choice = requested.clone();
            let error = draft.build().expect_err("only `auto` is supported");
            assert_eq!(
                error,
                RequestBuildError::ToolChoiceUnsupported {
                    requested: Box::new(requested)
                }
            );
        }
    }

    #[test]
    fn a_temperature_above_one_is_refused() {
        let mut draft = Draft::new();
        draft.temperature_milli = Some(1_001);
        assert_eq!(
            draft.build().expect_err("Z.AI's ceiling is 1.0"),
            RequestBuildError::SamplingUnsupported {
                field: "temperature"
            }
        );

        let mut at_the_ceiling = Draft::new();
        at_the_ceiling.temperature_milli = Some(1_000);
        at_the_ceiling.build().expect("exactly 1.0 is accepted");
    }

    #[test]
    fn a_top_p_outside_the_provider_range_is_refused() {
        for milli in [9u16, 1_001] {
            let mut draft = Draft::new();
            draft.top_p_milli = Some(milli);
            assert_eq!(
                draft.build().expect_err("0.01..=1.0 only"),
                RequestBuildError::SamplingUnsupported { field: "top_p" }
            );
        }
    }

    #[test]
    fn more_than_four_stop_sequences_are_refused() {
        let mut draft = Draft::new();
        draft.stop_sequences = (0..5).map(|n| fixture::bounded(&format!("s{n}"))).collect();
        assert_eq!(
            draft.build().expect_err("four is the bound"),
            RequestBuildError::StopSequenceLimit { max: 4 }
        );

        let mut exactly_four = Draft::new();
        exactly_four.stop_sequences = (0..4).map(|n| fixture::bounded(&format!("s{n}"))).collect();
        exactly_four.build().expect("four fits");
    }

    #[test]
    fn a_json_schema_request_is_refused_because_only_json_object_exists() {
        let requested = StructuredOutputRequest::JsonSchema {
            name: aex_wire::ResourceName::parse("Answer").expect("name"),
            schema: CanonicalJson::parse(r#"{"type":"object"}"#).expect("schema"),
            strict: true,
        };
        let mut draft = Draft::new();
        draft.structured_output = Some(requested.clone());
        assert_eq!(
            draft.build().expect_err("json_object only"),
            RequestBuildError::StructuredOutputUnsupported {
                requested: Box::new(requested)
            }
        );
    }

    /// One capability paired with the draft edit that needs it.
    type CapabilityCase = (Capability, Box<dyn Fn(&mut Draft)>);

    #[test]
    fn an_undeclared_capability_is_refused_before_dispatch() {
        let cases: Vec<CapabilityCase> = vec![
            (
                Capability::Tools,
                Box::new(|draft: &mut Draft| draft.tools.push(tool("lookup"))),
            ),
            (
                Capability::Temperature,
                Box::new(|draft: &mut Draft| draft.temperature_milli = Some(500)),
            ),
            (
                Capability::TopP,
                Box::new(|draft: &mut Draft| draft.top_p_milli = Some(500)),
            ),
            (
                Capability::StopSequences,
                Box::new(|draft: &mut Draft| draft.stop_sequences.push(fixture::bounded("x"))),
            ),
            (
                Capability::StructuredOutput,
                Box::new(|draft: &mut Draft| {
                    draft.structured_output = Some(StructuredOutputRequest::JsonObject);
                }),
            ),
            (
                Capability::Reasoning,
                Box::new(|draft: &mut Draft| {
                    draft.reasoning = ReasoningRequest::Enabled {
                        budget_tokens: None,
                        effort: None,
                    };
                }),
            ),
            (
                Capability::PromptCacheExplicit,
                Box::new(|draft: &mut Draft| {
                    draft.cache_breakpoints.push(CacheBreakpoint::AfterSystem);
                }),
            ),
        ];
        for (capability, apply) in cases {
            let mut entry = entry();
            entry.capabilities = full_capabilities();
            let mut narrowed = CapabilitySet::EMPTY;
            for declared in full_capabilities().declared() {
                if declared != capability {
                    narrowed = narrowed.with(declared);
                }
            }
            entry.capabilities = narrowed;
            let mut draft = Draft::new();
            apply(&mut draft);
            assert_eq!(
                draft.build_with(&entry).expect_err("undeclared"),
                RequestBuildError::CapabilityUnavailable { capability },
                "{capability:?} was not gated"
            );
        }
    }

    #[test]
    fn a_pair_that_does_not_declare_streaming_cannot_be_dispatched() {
        let mut entry = entry();
        let mut narrowed = CapabilitySet::EMPTY;
        for declared in full_capabilities().declared() {
            if declared != Capability::Streaming {
                narrowed = narrowed.with(declared);
            }
        }
        entry.capabilities = narrowed;
        assert_eq!(
            Draft::new().build_with(&entry).expect_err("no streaming"),
            RequestBuildError::CapabilityUnavailable {
                capability: Capability::Streaming
            }
        );
    }

    #[test]
    fn a_tool_name_outside_the_grammar_is_refused() {
        let mut draft = Draft::new();
        let mut bad = tool("lookup");
        bad.name = ToolName::parse("look.up").expect("the workspace grammar allows a dot");
        draft.tools.push(bad.clone());
        assert_eq!(
            draft.build().expect_err("the provider grammar has no dot"),
            RequestBuildError::ToolNameInvalid { name: bad.name }
        );
    }

    #[test]
    fn more_tools_than_the_entry_declares_are_refused() {
        let mut entry = entry();
        entry.limits.max_tools = 1;
        let mut draft = Draft::new();
        draft.tools = vec![tool("lookup"), tool("search")];
        assert_eq!(
            draft.build_with(&entry).expect_err("over the tool bound"),
            RequestBuildError::ToolLimit { max: 1 }
        );
    }

    #[test]
    fn an_output_ceiling_outside_the_range_is_refused() {
        let mut draft = Draft::new();
        draft.max_output_tokens = 0;
        assert_eq!(
            draft.build().expect_err("Z.AI's floor is 1"),
            RequestBuildError::OutputTokensOutOfRange { min: 1, max: 8_192 }
        );

        let mut over = Draft::new();
        over.max_output_tokens = 8_193;
        assert_eq!(
            over.build().expect_err("over the entry ceiling"),
            RequestBuildError::OutputTokensOutOfRange { min: 1, max: 8_192 }
        );
    }

    #[test]
    fn a_reasoning_budget_is_refused_because_zai_takes_none() {
        let mut draft = Draft::new();
        draft.reasoning = ReasoningRequest::Enabled {
            budget_tokens: Some(2_048),
            effort: None,
        };
        assert_eq!(
            draft.build().expect_err("no budget field exists"),
            RequestBuildError::ReasoningBudgetOutOfRange { min: 0, max: 0 }
        );
    }

    #[test]
    fn reasoning_material_from_another_provider_is_refused() {
        let mut draft = Draft::new();
        draft.messages.push(CanonicalMessage {
            role: Role::Assistant,
            blocks: vec![CanonicalBlock::Reasoning(ReasoningBlock {
                body: ReasoningBody::Text {
                    text: BoundedString::truncating("borrowed thought"),
                },
                token: Some(ReasoningToken {
                    provenance: ProviderId::Deepseek,
                    bytes: bytes::Bytes::from_static(b"borrowed thought"),
                }),
            })],
        });
        assert_eq!(
            draft.build().expect_err("provenance is enforced"),
            RequestBuildError::ReasoningProvenanceMismatch {
                expected: ProviderId::Zai,
                found: ProviderId::Deepseek,
            }
        );
    }

    #[test]
    fn a_body_over_the_entry_bound_is_refused() {
        let mut entry = entry();
        entry.limits.request_body_max_bytes = 32;
        assert_eq!(
            Draft::new().build_with(&entry).expect_err("over the bound"),
            RequestBuildError::BodyTooLarge { limit: 32 }
        );
    }

    #[test]
    fn a_correlation_id_outside_the_request_id_bound_is_refused() {
        let mut draft = Draft::new();
        draft.correlation = CorrelationId(BoundedString::truncating("aex"));
        assert_eq!(
            draft.build().expect_err("request_id is 6..=64 characters"),
            RequestBuildError::Encoding {
                reason: "the correlation id is outside Z.AI's 6..=64-byte request_id bound"
            }
        );
    }

    #[test]
    fn disabling_parallel_tool_calls_is_refused_because_zai_has_no_switch() {
        let mut draft = Draft::new();
        draft.parallel_tools = false;
        draft.tools.push(tool("lookup"));
        assert_eq!(
            draft.build().expect_err("nothing to encode it with"),
            RequestBuildError::Encoding {
                reason: "Z.AI has no switch that disables parallel tool calls"
            }
        );
    }

    #[test]
    fn reasoning_forbids_sampling_when_the_entry_says_so() {
        let mut entry = entry();
        entry.reasoning.excludes_sampling = true;
        let mut draft = Draft::new();
        draft.reasoning = ReasoningRequest::Enabled {
            budget_tokens: None,
            effort: None,
        };
        draft.temperature_milli = Some(500);
        assert_eq!(
            draft
                .build_with(&entry)
                .expect_err("thinking excludes sampling"),
            RequestBuildError::SamplingWithReasoning {
                field: "temperature"
            }
        );
    }

    #[test]
    fn sampling_on_a_pair_that_honours_none_is_refused() {
        let mut entry = entry();
        entry.sampling = SamplingSupport::None;
        let mut draft = Draft::new();
        draft.temperature_milli = Some(500);
        assert_eq!(
            draft.build_with(&entry).expect_err("the pair honours none"),
            RequestBuildError::SamplingUnsupported {
                field: "temperature"
            }
        );

        let mut temperature_only = entry.clone();
        temperature_only.sampling = SamplingSupport::TemperatureOnly;
        let mut top_p = Draft::new();
        top_p.top_p_milli = Some(500);
        assert_eq!(
            top_p
                .build_with(&temperature_only)
                .expect_err("top_p is not honoured"),
            RequestBuildError::SamplingUnsupported { field: "top_p" }
        );
    }

    #[test]
    fn a_block_a_turn_cannot_carry_is_refused_rather_than_dropped() {
        let mut draft = Draft::new();
        draft.messages = vec![CanonicalMessage {
            role: Role::User,
            blocks: vec![CanonicalBlock::ToolUse {
                id: ToolCallId::new("call_1").expect("id"),
                name: ToolName::parse("lookup").expect("name"),
                input: CanonicalJson::parse("{}").expect("input"),
            }],
        }];
        assert_eq!(
            draft.build().expect_err("a user turn cannot call a tool"),
            RequestBuildError::Encoding {
                reason: "a user turn may carry only text and tool results"
            }
        );
    }

    // -----------------------------------------------------------------------
    // frame taxonomy
    // -----------------------------------------------------------------------

    fn feed(state: &mut DialectState, chunk: &Value) -> Result<FrameOutcome, FrameDecodeError> {
        let payload = serde_json::to_vec(chunk).expect("a chunk serializes");
        feed_raw(state, &payload)
    }

    fn feed_raw(
        state: &mut DialectState,
        payload: &[u8],
    ) -> Result<FrameOutcome, FrameDecodeError> {
        ZaiAdapter.decode(
            state,
            &SseEvent {
                name: None,
                data: payload,
                id: None,
            },
            &StreamBudget::default(),
        )
    }

    fn content_chunk(text: &str) -> Value {
        json!({
            "id": "chat-1",
            "created": 1,
            "model": "glm-5.2",
            "choices": [{ "index": 0, "delta": { "role": "assistant", "content": text } }],
        })
    }

    fn finish_chunk(reason: &str) -> Value {
        json!({
            "id": "chat-1",
            "model": "glm-5.2",
            "choices": [{ "index": 0, "delta": {}, "finish_reason": reason }],
        })
    }

    #[test]
    fn the_first_chunk_with_a_choice_starts_the_response_exactly_once() {
        let mut state = fresh_state();
        assert_eq!(
            feed(&mut state, &content_chunk("he")).expect("first"),
            FrameOutcome::ResponseStarted
        );
        assert_eq!(
            feed(&mut state, &content_chunk("llo")).expect("second"),
            FrameOutcome::Progress
        );
        assert!(state.response_started);
    }

    #[test]
    fn a_chunk_with_no_choices_is_ignored_and_does_not_start_the_response() {
        let mut state = fresh_state();
        let chunk = json!({ "id": "chat-1", "model": "glm-5.2", "choices": [] });
        assert_eq!(
            feed(&mut state, &chunk).expect("empty"),
            FrameOutcome::Ignored
        );
        assert!(
            !state.response_started,
            "only a chunk carrying choices[0] proves the provider is generating"
        );
    }

    #[test]
    fn a_named_event_is_not_a_frame_this_data_only_dialect_sends() {
        let mut state = fresh_state();
        let error = ZaiAdapter
            .decode(
                &mut state,
                &SseEvent {
                    name: Some("message_start"),
                    data: b"{}",
                    id: None,
                },
                &StreamBudget::default(),
            )
            .expect_err("named events belong to another dialect");
        assert!(matches!(error, FrameDecodeError::UnknownEvent { .. }));
    }

    #[test]
    fn a_non_json_frame_is_refused() {
        let mut state = fresh_state();
        assert_eq!(
            feed_raw(&mut state, b"not json").expect_err("garbage"),
            FrameDecodeError::NotJson
        );
        let mut other = fresh_state();
        assert_eq!(
            feed_raw(&mut other, b"[1,2,3]").expect_err("an array is not a chunk"),
            FrameDecodeError::NotJson
        );
    }

    #[test]
    fn a_chunk_without_choices_at_all_is_malformed() {
        let mut state = fresh_state();
        assert_eq!(
            feed(&mut state, &json!({ "id": "chat-1" })).expect_err("choices is required"),
            FrameDecodeError::MalformedField { field: "choices" }
        );
    }

    #[test]
    fn the_done_sentinel_before_any_finish_reason_is_out_of_order() {
        let mut state = fresh_state();
        feed(&mut state, &content_chunk("hi")).expect("content");
        assert_eq!(
            feed_raw(&mut state, b"[DONE]").expect_err("no finish_reason arrived"),
            FrameDecodeError::OutOfOrder {
                reason: "the [DONE] sentinel arrived before any finish_reason"
            }
        );
    }

    #[test]
    fn the_done_sentinel_after_the_terminal_chunk_is_ignored() {
        let mut state = fresh_state();
        feed(&mut state, &content_chunk("hi")).expect("content");
        assert_eq!(
            feed(&mut state, &finish_chunk("stop")).expect("terminal"),
            FrameOutcome::Terminal
        );
        assert_eq!(
            feed_raw(&mut state, b"[DONE]").expect("the sentinel closes the stream"),
            FrameOutcome::Ignored
        );
    }

    #[test]
    fn a_chunk_after_the_terminal_chunk_is_out_of_order() {
        let mut state = fresh_state();
        feed(&mut state, &finish_chunk("stop")).expect("terminal");
        assert_eq!(
            feed(&mut state, &content_chunk("more")).expect_err("the turn is over"),
            FrameDecodeError::OutOfOrder {
                reason: "a chunk arrived after the finish_reason chunk"
            }
        );
    }

    #[test]
    fn an_upstream_provider_hosted_tool_type_is_a_protocol_violation() {
        for kind in ["web_search", "retrieval"] {
            let mut state = fresh_state();
            let chunk = json!({
                "choices": [{
                    "index": 0,
                    "delta": { "tool_calls": [{ "index": 0, "type": kind, "id": "c1" }] },
                }],
            });
            let error = feed(&mut state, &chunk).expect_err("these tools are never sent");
            assert!(
                matches!(&error, FrameDecodeError::UnknownEvent { event } if event.as_str() == kind),
                "{kind} was not refused: {error:?}"
            );
        }
    }

    #[test]
    fn reasoning_and_content_deltas_accumulate_into_separate_blocks() {
        let mut state = fresh_state();
        let thinking = json!({
            "choices": [{ "index": 0, "delta": { "reasoning_content": "weighing " } }],
        });
        feed(&mut state, &thinking).expect("reasoning");
        let more = json!({
            "choices": [{ "index": 0, "delta": { "reasoning_content": "options" } }],
        });
        feed(&mut state, &more).expect("more reasoning");
        feed(&mut state, &content_chunk("answer")).expect("content");
        assert_eq!(
            state.open_reasoning.get(&0).map(String::as_str),
            Some("weighing options")
        );
        assert_eq!(state.open_text.get(&0).map(String::as_str), Some("answer"));
    }

    #[test]
    fn the_echoed_request_id_is_the_only_correlation_handle() {
        let mut state = fresh_state();
        let chunk = json!({
            "request_id": "aex-abababababababababababababababab",
            "choices": [{ "index": 0, "delta": { "content": "hi" } }],
        });
        feed(&mut state, &chunk).expect("chunk");
        let headers = reqwest::header::HeaderMap::new();
        assert_eq!(
            ZaiAdapter
                .request_id(&HeaderView::new(&headers), &state)
                .as_ref()
                .map(BoundedString::as_str),
            Some("aex-abababababababababababababababab")
        );
    }

    #[test]
    fn a_stream_that_echoes_no_request_id_reports_none() {
        let mut state = fresh_state();
        feed(&mut state, &content_chunk("hi")).expect("chunk");
        let headers = reqwest::header::HeaderMap::new();
        assert!(
            ZaiAdapter
                .request_id(&HeaderView::new(&headers), &state)
                .is_none(),
            "no header carries one, so absence is the honest answer"
        );
    }

    // -----------------------------------------------------------------------
    // tool arguments
    // -----------------------------------------------------------------------

    fn tool_fragment(arguments: &Value) -> Value {
        json!({
            "choices": [{
                "index": 0,
                "delta": {
                    "tool_calls": [{
                        "index": 0,
                        "id": "call_1",
                        "type": "function",
                        "function": { "name": "lookup", "arguments": arguments },
                    }],
                },
            }],
        })
    }

    #[test]
    fn tool_arguments_arriving_as_a_string_normalize_to_canonical_json() {
        let mut state = fresh_state();
        feed(&mut state, &tool_fragment(&json!(r#"{"q":"aex"}"#))).expect("string shape");
        feed(&mut state, &finish_chunk("tool_calls")).expect("terminal");
        let sealed = ZaiAdapter.finish(state).expect("seals");
        assert_eq!(sealed.stop_reason, StopReason::ToolUse);
        let CanonicalBlock::ToolUse { id, name, input } = &sealed.blocks[0] else {
            panic!("expected a tool-use block, got {:?}", sealed.blocks[0]);
        };
        assert_eq!(id.as_str(), "call_1");
        assert_eq!(name.as_str(), "lookup");
        assert_eq!(input.as_str(), r#"{"q":"aex"}"#);
    }

    #[test]
    fn tool_arguments_arriving_as_an_object_normalize_to_canonical_json() {
        let mut state = fresh_state();
        feed(&mut state, &tool_fragment(&json!({ "q": "aex" }))).expect("object shape");
        feed(&mut state, &finish_chunk("tool_calls")).expect("terminal");
        let sealed = ZaiAdapter.finish(state).expect("seals");
        let CanonicalBlock::ToolUse { input, .. } = &sealed.blocks[0] else {
            panic!("expected a tool-use block");
        };
        assert_eq!(
            input.as_str(),
            r#"{"q":"aex"}"#,
            "both wire shapes reach the same canonical value"
        );
    }

    #[test]
    fn tool_argument_fragments_reassemble_across_a_string_escape_boundary() {
        let mut state = fresh_state();
        // The split falls between the backslash and the `n` of a `\n` escape,
        // so neither fragment is valid JSON on its own.
        for fragment in [r#"{"path":"a\"#, r#"nb"}"#] {
            feed(&mut state, &tool_fragment(&json!(fragment))).expect("fragment");
        }
        feed(&mut state, &finish_chunk("tool_calls")).expect("terminal");
        let sealed = ZaiAdapter.finish(state).expect("seals");
        let CanonicalBlock::ToolUse { input, .. } = &sealed.blocks[0] else {
            panic!("expected a tool-use block");
        };
        assert_eq!(
            input.to_value(),
            json!({ "path": "a\nb" }),
            "the escape survived the fragment boundary"
        );
    }

    #[test]
    fn a_whole_object_after_a_fragment_is_out_of_order() {
        let mut state = fresh_state();
        feed(&mut state, &tool_fragment(&json!(r#"{"q":"#))).expect("fragment");
        assert_eq!(
            feed(&mut state, &tool_fragment(&json!({ "q": "aex" }))).expect_err("mixed shapes"),
            FrameDecodeError::OutOfOrder {
                reason: "whole tool arguments arrived after a fragment"
            }
        );
    }

    #[test]
    fn tool_arguments_that_never_become_json_are_refused() {
        let mut state = fresh_state();
        feed(&mut state, &tool_fragment(&json!(r#"{"q":"#))).expect("fragment");
        feed(&mut state, &finish_chunk("tool_calls")).expect("terminal");
        let error = ZaiAdapter.finish(state).expect_err("half a document");
        assert!(matches!(
            error,
            FrameDecodeError::ToolArgumentsNotJson { .. }
        ));
    }

    #[test]
    fn a_tool_call_fragment_without_an_index_cannot_be_reassembled() {
        let mut state = fresh_state();
        let chunk = json!({
            "choices": [{
                "index": 0,
                "delta": { "tool_calls": [{ "id": "call_1", "type": "function" }] },
            }],
        });
        assert_eq!(
            feed(&mut state, &chunk).expect_err("index drives reassembly"),
            FrameDecodeError::MalformedField {
                field: "choices[].delta.tool_calls[].index"
            }
        );
    }

    #[test]
    fn two_tool_calls_reassemble_independently_by_index() {
        let mut state = fresh_state();
        let chunk = json!({
            "choices": [{
                "index": 0,
                "delta": { "tool_calls": [
                    { "index": 0, "id": "call_a", "type": "function",
                      "function": { "name": "lookup", "arguments": "{\"q\":" } },
                    { "index": 1, "id": "call_b", "type": "function",
                      "function": { "name": "search", "arguments": "{\"n\":" } },
                ] },
            }],
        });
        feed(&mut state, &chunk).expect("first fragments");
        let tail = json!({
            "choices": [{
                "index": 0,
                "delta": { "tool_calls": [
                    { "index": 1, "function": { "arguments": "2}" } },
                    { "index": 0, "function": { "arguments": "\"aex\"}" } },
                ] },
            }],
        });
        feed(&mut state, &tail).expect("second fragments");
        feed(&mut state, &finish_chunk("tool_calls")).expect("terminal");
        let sealed = ZaiAdapter.finish(state).expect("seals");
        assert_eq!(sealed.blocks.len(), 2);
        let CanonicalBlock::ToolUse { id, input, .. } = &sealed.blocks[0] else {
            panic!("expected a tool-use block");
        };
        assert_eq!(id.as_str(), "call_a");
        assert_eq!(input.as_str(), r#"{"q":"aex"}"#);
    }

    // -----------------------------------------------------------------------
    // the last-content-chunk usage trap
    // -----------------------------------------------------------------------

    #[test]
    fn usage_arrives_on_the_last_content_chunk_rather_than_a_trailing_one() {
        let mut state = fresh_state();
        feed(&mut state, &content_chunk("hello")).expect("content");
        assert_eq!(
            state.usage.completeness,
            UsageCompleteness::Absent,
            "no chunk before the last one carries usage"
        );
        let last = json!({
            "choices": [{ "index": 0, "delta": { "content": " there" }, "finish_reason": "stop" }],
            "usage": {
                "prompt_tokens": 100,
                "completion_tokens": 20,
                "total_tokens": 120,
                "prompt_tokens_details": { "cached_tokens": 40 },
            },
        });
        assert_eq!(
            feed(&mut state, &last).expect("terminal"),
            FrameOutcome::Terminal
        );
        assert_eq!(state.usage.input_tokens, 60);
        assert_eq!(state.usage.cache_read_input_tokens, 40);
        assert_eq!(state.usage.output_tokens, 20);
        assert_eq!(state.usage.provider_total_tokens, Some(120));
        assert_eq!(state.usage.completeness, UsageCompleteness::Exact);
        // The `[DONE]` sentinel that follows carries nothing: waiting for a
        // trailing chunk would lose the accounting entirely.
        assert_eq!(
            feed_raw(&mut state, b"[DONE]").expect("sentinel"),
            FrameOutcome::Ignored
        );
        assert_eq!(state.usage.output_tokens, 20);
    }

    #[test]
    fn cached_prompt_tokens_are_a_subset_of_the_prompt_count() {
        let mut state = fresh_state();
        let chunk = json!({
            "choices": [{ "index": 0, "delta": { "content": "x" }, "finish_reason": "stop" }],
            "usage": {
                "prompt_tokens": 10,
                "completion_tokens": 1,
                "total_tokens": 11,
                "prompt_tokens_details": { "cached_tokens": 25 },
            },
        });
        feed(&mut state, &chunk).expect("terminal");
        assert_eq!(
            state.usage.cache_read_input_tokens, 10,
            "the cached count can never exceed the prompt it is a subset of"
        );
        assert_eq!(state.usage.input_tokens, 0);
        assert!(state.usage.is_consistent());
    }

    #[test]
    fn a_partial_usage_block_records_which_fields_were_missing() {
        let mut state = fresh_state();
        let chunk = json!({
            "choices": [{ "index": 0, "delta": {}, "finish_reason": "stop" }],
            "usage": { "prompt_tokens": 5 },
        });
        feed(&mut state, &chunk).expect("terminal");
        let missing = UsageFieldSet::EMPTY
            .with(UsageField::OutputTokens)
            .with(UsageField::ProviderTotalTokens);
        assert_eq!(
            state.usage.completeness,
            UsageCompleteness::Partial { missing },
            "usage is never invented"
        );
    }

    #[test]
    fn absent_usage_is_recorded_as_absent_rather_than_invented() {
        let mut state = fresh_state();
        feed(&mut state, &content_chunk("hi")).expect("content");
        feed(&mut state, &finish_chunk("stop")).expect("terminal");
        let sealed = ZaiAdapter.finish(state).expect("seals");
        assert_eq!(sealed.usage.completeness, UsageCompleteness::Absent);
        assert_eq!(sealed.usage.output_tokens, 0);
    }

    // -----------------------------------------------------------------------
    // the mid-stream trap: three finish tokens that are failures
    // -----------------------------------------------------------------------

    fn expect_mid_stream_failure(token: &str, kind: ProviderFailureKind) {
        let mut state = fresh_state();
        feed(&mut state, &content_chunk("partial")).expect("content");
        let outcome = feed(&mut state, &finish_chunk(token)).expect("decodes");
        let FrameOutcome::Failed(failure) = outcome else {
            panic!("`{token}` must be a terminal failure, got {outcome:?}");
        };
        assert_eq!(failure.kind(), kind);
        assert_eq!(
            failure
                .detail
                .provider_code
                .as_ref()
                .map(BoundedString::as_str),
            Some(token),
            "the finish token is the only code this API publishes mid-stream"
        );
        assert!(
            state.response_started,
            "the failure follows a chunk with choices[0], so the proof is ResponseStarted"
        );
        assert!(
            ZaiAdapter.finish(state).is_err(),
            "a failing finish_reason must never seal a turn into history"
        );
    }

    #[test]
    fn a_sensitive_finish_reason_is_a_terminal_failure_not_a_stop_reason() {
        expect_mid_stream_failure("sensitive", ProviderFailureKind::ContentFiltered);
    }

    #[test]
    fn a_context_window_finish_reason_is_a_terminal_failure_not_a_stop_reason() {
        expect_mid_stream_failure(
            "model_context_window_exceeded",
            ProviderFailureKind::ContextOverflow,
        );
    }

    #[test]
    fn a_network_error_finish_reason_is_a_terminal_failure_not_a_stop_reason() {
        expect_mid_stream_failure("network_error", ProviderFailureKind::Transport);
    }

    #[test]
    fn no_failing_finish_token_is_reachable_as_a_stop_reason() {
        for token in [
            "sensitive",
            "model_context_window_exceeded",
            "network_error",
        ] {
            assert!(
                matches!(resolve_finish(token), Some(ZaiFinish::Failure(_))),
                "{token} must never resolve to a stop reason"
            );
        }
    }

    // -----------------------------------------------------------------------
    // the stop table
    // -----------------------------------------------------------------------

    #[test]
    fn the_finish_token_table_is_exhaustive_and_closed() {
        let stops = [
            ("stop", StopReason::EndTurn),
            ("tool_calls", StopReason::ToolUse),
            ("length", StopReason::MaxOutputTokens),
        ];
        for (token, expected) in stops {
            assert_eq!(resolve_finish(token), Some(ZaiFinish::Stop(expected)));
        }
        let failures = [
            ("sensitive", ProviderFailureKind::ContentFiltered),
            (
                "model_context_window_exceeded",
                ProviderFailureKind::ContextOverflow,
            ),
            ("network_error", ProviderFailureKind::Transport),
        ];
        for (token, expected) in failures {
            assert_eq!(resolve_finish(token), Some(ZaiFinish::Failure(expected)));
        }
        for unknown in ["", "content_filter", "end_turn", "max_tokens", "STOP"] {
            assert_eq!(
                resolve_finish(unknown),
                None,
                "`{unknown}` is not in Z.AI's vocabulary"
            );
        }
    }

    #[test]
    fn the_compiled_table_and_the_catalog_map_name_the_same_tokens() {
        let map = zai_stop_map();
        let mut named: Vec<&str> = map.tokens();
        named.sort_unstable();
        let mut compiled = vec![
            "stop",
            "tool_calls",
            "length",
            "sensitive",
            "model_context_window_exceeded",
            "network_error",
        ];
        compiled.sort_unstable();
        assert_eq!(named, compiled);
    }

    #[test]
    fn an_unknown_finish_reason_is_refused_rather_than_guessed() {
        let mut state = fresh_state();
        assert_eq!(
            feed(&mut state, &finish_chunk("content_filter")).expect_err("not a Z.AI token"),
            FrameDecodeError::MalformedField {
                field: "choices[].finish_reason"
            }
        );
    }

    #[test]
    fn each_stop_token_seals_to_its_canonical_reason() {
        for (token, expected) in [
            ("stop", StopReason::EndTurn),
            ("length", StopReason::MaxOutputTokens),
        ] {
            let mut state = fresh_state();
            feed(&mut state, &content_chunk("body")).expect("content");
            feed(&mut state, &finish_chunk(token)).expect("terminal");
            let sealed = ZaiAdapter.finish(state).expect("seals");
            assert_eq!(sealed.stop_reason, expected);
        }
    }

    // -----------------------------------------------------------------------
    // sealing
    // -----------------------------------------------------------------------

    #[test]
    fn a_stream_that_never_reached_a_finish_reason_cannot_seal() {
        let mut state = fresh_state();
        feed(&mut state, &content_chunk("half")).expect("content");
        assert_eq!(
            ZaiAdapter.finish(state).expect_err("no terminal frame"),
            FrameDecodeError::OutOfOrder {
                reason: "the stream ended without a finish_reason"
            }
        );
    }

    #[test]
    fn a_sealed_turn_orders_reasoning_then_prose_then_tool_calls() {
        let mut state = fresh_state();
        let chunk = json!({
            "choices": [{
                "index": 0,
                "delta": {
                    "reasoning_content": "thinking",
                    "content": "calling out",
                    "tool_calls": [{
                        "index": 0, "id": "call_1", "type": "function",
                        "function": { "name": "lookup", "arguments": "{}" },
                    }],
                },
            }],
        });
        feed(&mut state, &chunk).expect("content");
        feed(&mut state, &finish_chunk("tool_calls")).expect("terminal");
        let sealed = ZaiAdapter.finish(state).expect("seals");
        assert_eq!(sealed.blocks.len(), 3);
        let CanonicalBlock::Reasoning(reasoning) = &sealed.blocks[0] else {
            panic!("expected reasoning first");
        };
        assert_eq!(
            reasoning.body,
            ReasoningBody::Text {
                text: BoundedString::truncating("thinking")
            }
        );
        let token = reasoning.token.as_ref().expect("Z.AI replays its own text");
        assert_eq!(token.provenance, ProviderId::Zai);
        assert_eq!(token.bytes.as_ref(), b"thinking");
        assert!(matches!(sealed.blocks[1], CanonicalBlock::Text { .. }));
        assert!(matches!(sealed.blocks[2], CanonicalBlock::ToolUse { .. }));
    }

    // -----------------------------------------------------------------------
    // HTTP failures
    // -----------------------------------------------------------------------

    fn classify_body(status: u16, body: &str) -> crate::error::ProviderFailure {
        let headers = reqwest::header::HeaderMap::new();
        ZaiAdapter.classify_http(
            status,
            &HeaderView::new(&headers),
            &BoundedBody::new(body.as_bytes().to_vec(), false),
        )
    }

    #[test]
    fn the_documented_error_codes_map_exhaustively() {
        let table = [
            ("1000", ProviderFailureKind::Authentication),
            ("1001", ProviderFailureKind::Authentication),
            ("1003", ProviderFailureKind::Authentication),
            ("1113", ProviderFailureKind::Billing),
            ("1210", ProviderFailureKind::InvalidRequest),
            ("1211", ProviderFailureKind::ModelNotFound),
            ("1212", ProviderFailureKind::InvalidRequest),
            ("1213", ProviderFailureKind::InvalidRequest),
            ("1214", ProviderFailureKind::InvalidRequest),
            ("1215", ProviderFailureKind::InvalidRequest),
            ("1220", ProviderFailureKind::Authentication),
            ("1261", ProviderFailureKind::ContextOverflow),
            ("1301", ProviderFailureKind::ContentFiltered),
            ("1302", ProviderFailureKind::RateLimited),
            ("1305", ProviderFailureKind::Overloaded),
        ];
        for (code, expected) in table {
            assert_eq!(kind_for_code(code), Some(expected), "code {code}");
        }
        for numeric in 1308..=1321u32 {
            assert_eq!(
                kind_for_code(&numeric.to_string()),
                Some(ProviderFailureKind::Quota),
                "plan exhaustion {numeric} must be permanent, never retried"
            );
        }
        for outside in ["1307", "1322", "1216", "", "not-a-code"] {
            assert_eq!(kind_for_code(outside), None, "code {outside}");
        }
    }

    #[test]
    fn an_error_code_is_read_as_a_string() {
        let failure = classify_body(
            400,
            r#"{"error":{"code":"1214","message":"bad parameter"}}"#,
        );
        assert_eq!(failure.kind(), ProviderFailureKind::InvalidRequest);
        assert_eq!(
            failure
                .detail
                .provider_code
                .as_ref()
                .map(BoundedString::as_str),
            Some("1214")
        );
        assert_eq!(failure.detail.http_status, Some(400));
        assert_eq!(failure.detail.message.as_str(), "bad parameter");
    }

    #[test]
    fn a_body_code_wins_over_the_status_so_a_quota_failure_is_never_retried() {
        // A 500 carrying a plan-exhaustion code must not classify as a
        // transient server error.
        let failure = classify_body(500, r#"{"error":{"code":"1310","message":"plan used up"}}"#);
        assert_eq!(failure.kind(), ProviderFailureKind::Quota);
        assert!(!failure.kind().is_in_call_retryable());
    }

    #[test]
    fn a_status_without_a_body_code_falls_back_to_the_status_table() {
        for (status, expected) in [
            (401, ProviderFailureKind::Authentication),
            (403, ProviderFailureKind::Authentication),
            (402, ProviderFailureKind::Billing),
            (404, ProviderFailureKind::ModelNotFound),
            (429, ProviderFailureKind::RateLimited),
            (500, ProviderFailureKind::ServerError),
            (503, ProviderFailureKind::Overloaded),
            (504, ProviderFailureKind::Timeout),
            (400, ProviderFailureKind::InvalidRequest),
        ] {
            assert_eq!(kind_for_status(status), expected, "status {status}");
        }
    }

    #[test]
    fn a_truncated_error_body_falls_back_to_the_status() {
        let headers = reqwest::header::HeaderMap::new();
        let failure = ZaiAdapter.classify_http(
            503,
            &HeaderView::new(&headers),
            &BoundedBody::new(b"{\"error\":{\"code\":".to_vec(), true),
        );
        assert_eq!(failure.kind(), ProviderFailureKind::Overloaded);
        assert!(failure.detail.provider_code.is_none());
        assert_eq!(
            failure.detail.message.as_str(),
            "the provider returned no readable error body"
        );
    }

    #[test]
    fn a_body_message_is_redacted_before_it_reaches_the_detail() {
        let leaked = "sk-abcdefghij0123456789abcdefghij0123456789";
        let body = format!(r#"{{"error":{{"code":"1000","message":"bad key {leaked}"}}}}"#);
        let failure = classify_body(401, &body);
        assert_eq!(failure.kind(), ProviderFailureKind::Authentication);
        assert!(
            !failure.detail.message.as_str().contains(leaked),
            "a provider that echoes a credential must not leak it: {}",
            failure.detail.message
        );
    }

    #[test]
    fn only_the_backpressure_codes_record_error_body_feedback() {
        for code in ["1302", "1305", "1308", "1321"] {
            assert!(is_backpressure_code(code), "{code}");
            let body = format!(r#"{{"error":{{"code":"{code}","message":"slow down"}}}}"#);
            assert_eq!(
                classify_body(429, &body).rate_limit.source,
                RateLimitSource::ErrorBody,
                "{code} is the only backpressure signal Z.AI publishes"
            );
        }
        for code in ["1000", "1214", "1307", "1322"] {
            assert!(!is_backpressure_code(code), "{code}");
        }
        // A 429 with no readable code carries no feedback at all, which is the
        // honest record: this API publishes no rate-limit headers.
        assert!(classify_body(429, "{}").rate_limit.is_absent());
    }

    #[test]
    fn zai_publishes_no_rate_limit_headers_so_the_absence_is_recorded_positively() {
        let mut headers = reqwest::header::HeaderMap::new();
        // Even when an intermediary invents them, this dialect reads none: the
        // published table is login-gated and the API documents no header.
        headers.insert("retry-after", "30".parse().expect("value"));
        headers.insert("x-ratelimit-remaining", "0".parse().expect("value"));
        let feedback = ZaiAdapter.rate_limit_feedback(&HeaderView::new(&headers));
        assert!(feedback.is_absent());
        assert_eq!(feedback.source, RateLimitSource::NotProvided);
        assert_eq!(feedback.retry_after, None);
    }
}
