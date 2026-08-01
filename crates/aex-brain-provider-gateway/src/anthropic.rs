//! The `anthropic` dialect adapter — Anthropic Messages (plan 08 §5.2).
//!
//! One path (`POST /v1/messages`) on one compiled origin, one auth tag
//! (`x-api-key` plus the pinned `anthropic-version`), and one streaming
//! grammar. `anthropic-beta` is never sent: structured outputs no longer need
//! one and interleaved thinking is not a launch capability (D-17).
//!
//! # What this module is careful about
//!
//! - **Cumulative usage.** `message_delta.usage` is a running total, not an
//!   increment. [`apply_usage`] *overwrites* every field the frame carries and
//!   never sums, because summing would double-count every token in a stream
//!   that reports usage more than once. This is the single most expensive trap
//!   in the dialect and it has its own named test.
//! - **`ping` is not progress.** A keep-alive is [`FrameOutcome::Ignored`];
//!   only `message_start` moves the effect to `response_started`, because that
//!   is a durable write which must mean "the provider is generating".
//! - **No `[DONE]`.** This dialect terminates on `message_stop`. The sentinel
//!   three of the six dialects send is rejected here as a frame this dialect
//!   does not produce.
//! - **Round-trip material is mandatory.** Anthropic rejects a replayed
//!   `thinking` block whose `signature` is missing, so the entry declares
//!   [`ReasoningReplay::RequiredAlways`] and a reasoning block carried without
//!   a token fails *before* dispatch.
//!
//! [`ReasoningReplay::RequiredAlways`]: aex_model_catalog::document::ReasoningReplay::RequiredAlways

use core::time::Duration;

use aex_model_catalog::QualifiedModel;
use aex_model_catalog::canonical::{
    CacheBreakpoint, CanonicalBlock, CanonicalModelRequest, NormalizedUsage, REASON_MAX,
    ReasoningBlock, ReasoningBody, ReasoningRequest, ReasoningToken, Role, StopReason,
    StructuredOutputRequest, TEXT_MAX, ToolChoice, ToolResultPart, UsageCompleteness,
};
use aex_model_catalog::document::{
    AdapterSourceDigest, CacheMode, Capability, Dialect, EndpointPin, ReasoningMode,
    SamplingSupport, SchemaEncoding, StopResolution, StructuredOutputPolicy,
};
use aex_model_catalog::primitives::{
    Blake3Digest, BoundedString, ProviderRequestId, ToolCallId, ToolName,
};
use aex_wire::CanonicalJson;
use aex_wire::provider::ProviderId;
use aex_wire::types::Timestamp;
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

// ---------------------------------------------------------------------------
// compiled protocol facts
// ---------------------------------------------------------------------------

/// The pinned API version, sent on every request. There is exactly one (D-17).
pub const API_VERSION: &str = "2023-06-01";

/// The non-secret header the version travels in.
pub const VERSION_HEADER: &str = "anthropic-version";

/// The only path this dialect uses.
pub const MESSAGES_PATH: &str = "/v1/messages";

/// The smallest thinking budget Anthropic accepts.
pub const MIN_THINKING_BUDGET: u32 = 1_024;

/// The `retry-after` header, the only standard backpressure signal.
const RETRY_AFTER: &str = "retry-after";

/// The response header carrying the provider's own request id.
const REQUEST_ID_HEADER: &str = "request-id";

/// Every vendor rate-limit header this dialect publishes. Presence of any one
/// of them is what makes the answer [`RateLimitSource::VendorHeaders`] rather
/// than a bare `retry-after` reading.
const VENDOR_RATE_LIMIT_HEADERS: [&str; 12] = [
    "anthropic-ratelimit-requests-limit",
    "anthropic-ratelimit-requests-remaining",
    "anthropic-ratelimit-requests-reset",
    "anthropic-ratelimit-tokens-limit",
    "anthropic-ratelimit-tokens-remaining",
    "anthropic-ratelimit-tokens-reset",
    "anthropic-ratelimit-input-tokens-limit",
    "anthropic-ratelimit-input-tokens-remaining",
    "anthropic-ratelimit-input-tokens-reset",
    "anthropic-ratelimit-output-tokens-limit",
    "anthropic-ratelimit-output-tokens-remaining",
    "anthropic-ratelimit-output-tokens-reset",
];

/// The four reset headers, read together so the answer is the instant after
/// which *every* published window has rolled over.
const RESET_HEADERS: [&str; 4] = [
    "anthropic-ratelimit-requests-reset",
    "anthropic-ratelimit-tokens-reset",
    "anthropic-ratelimit-input-tokens-reset",
    "anthropic-ratelimit-output-tokens-reset",
];

/// A crude bytes-per-token divisor for the pre-dispatch context estimate.
///
/// Deliberately an estimate: [`RequestBuildError::ContextOverflowEstimated`]
/// says so in its own name. A real tokenizer would be a second, drifting copy
/// of the provider's, and being wrong in the safe direction before a socket
/// exists is cheaper than being wrong after one.
const BYTES_PER_TOKEN: u64 = 4;

/// This module's own source bytes, which [`AnthropicAdapter::source_digest`]
/// hashes.
///
/// `TODO(cross-stream): the six adapters must agree on one digest over the
/// whole adapter source tree, because a catalog document carries a single
/// required_adapter_source. That shared constant belongs in a module every
/// adapter can see; this per-module digest is a placeholder that is correct in
/// kind but not yet shared.`
const SOURCE: &[u8] = include_bytes!("anthropic.rs");

/// The Anthropic Messages dialect adapter.
///
/// Stateless: every per-stream fact lives in the [`DialectState`] the shared
/// core hands back, so one instance serves every workspace concurrently.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct AnthropicAdapter;

// ---------------------------------------------------------------------------
// wire body
// ---------------------------------------------------------------------------

/// The `POST /v1/messages` body.
///
/// A struct rather than a `serde_json::Value` map so field order is the
/// declaration order above and a golden test can assert bytes rather than a
/// parsed value.
#[derive(Debug, Serialize)]
struct RequestBody<'a> {
    model: &'a str,
    max_tokens: u32,
    messages: Vec<WireMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    system: Option<Vec<WireSystemBlock>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    top_p: Option<f64>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    stop_sequences: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    tools: Vec<WireTool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_choice: Option<WireToolChoice>,
    #[serde(skip_serializing_if = "Option::is_none")]
    thinking: Option<WireThinking>,
    #[serde(skip_serializing_if = "Option::is_none")]
    output_config: Option<WireOutputConfig>,
    /// Always `true`. AEX has no non-streaming path.
    stream: bool,
}

#[derive(Debug, Serialize)]
struct WireMessage {
    role: &'static str,
    content: Vec<WireBlock>,
}

/// A prompt-cache breakpoint. Anthropic spells one exactly this way.
#[derive(Debug, Clone, Copy, Serialize)]
struct CacheControl {
    #[serde(rename = "type")]
    kind: &'static str,
}

impl CacheControl {
    const fn ephemeral() -> Self {
        Self { kind: "ephemeral" }
    }
}

/// One content block, in every shape this dialect accepts on input.
///
/// One struct with optional members rather than an internally tagged enum,
/// because `cache_control` may ride on any of them and a tagged enum cannot
/// carry a shared member without `flatten`, which reorders output.
#[derive(Debug, Default, Serialize)]
struct WireBlock {
    #[serde(rename = "type")]
    kind: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    thinking: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    signature: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    data: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    input: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_use_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    content: Option<Vec<WireBlock>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    is_error: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    cache_control: Option<CacheControl>,
}

impl WireBlock {
    fn text(text: &str) -> Self {
        Self {
            kind: "text",
            text: Some(text.to_owned()),
            ..Self::default()
        }
    }

    fn thinking(thinking: &str, signature: &str) -> Self {
        Self {
            kind: "thinking",
            thinking: Some(thinking.to_owned()),
            signature: Some(signature.to_owned()),
            ..Self::default()
        }
    }

    fn redacted_thinking(data: &str) -> Self {
        Self {
            kind: "redacted_thinking",
            data: Some(data.to_owned()),
            ..Self::default()
        }
    }

    fn tool_use(id: &str, name: &str, input: Value) -> Self {
        Self {
            kind: "tool_use",
            id: Some(id.to_owned()),
            name: Some(name.to_owned()),
            input: Some(input),
            ..Self::default()
        }
    }

    fn tool_result(tool_use_id: &str, content: Vec<Self>, is_error: bool) -> Self {
        Self {
            kind: "tool_result",
            tool_use_id: Some(tool_use_id.to_owned()),
            content: Some(content),
            is_error: Some(is_error),
            ..Self::default()
        }
    }
}

#[derive(Debug, Serialize)]
struct WireSystemBlock {
    #[serde(rename = "type")]
    kind: &'static str,
    text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    cache_control: Option<CacheControl>,
}

#[derive(Debug, Serialize)]
struct WireTool {
    name: String,
    description: String,
    input_schema: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    strict: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    cache_control: Option<CacheControl>,
}

#[derive(Debug, Serialize)]
struct WireToolChoice {
    #[serde(rename = "type")]
    kind: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    disable_parallel_tool_use: Option<bool>,
}

#[derive(Debug, Serialize)]
struct WireThinking {
    #[serde(rename = "type")]
    kind: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    budget_tokens: Option<u32>,
}

#[derive(Debug, Serialize)]
struct WireOutputConfig {
    format: WireOutputFormat,
}

#[derive(Debug, Serialize)]
struct WireOutputFormat {
    #[serde(rename = "type")]
    kind: &'static str,
    schema: Value,
}

// ---------------------------------------------------------------------------
// the adapter
// ---------------------------------------------------------------------------

impl ProviderAdapter for AnthropicAdapter {
    fn provider(&self) -> ProviderId {
        ProviderId::Anthropic
    }

    fn source_digest(&self) -> AdapterSourceDigest {
        AdapterSourceDigest(Blake3Digest::of(SOURCE))
    }

    fn build_request(
        &self,
        model: &QualifiedModel,
        request: &CanonicalModelRequest,
    ) -> Result<WireRequest, RequestBuildError> {
        if model.provider() != ProviderId::Anthropic
            || model.dialect() != Dialect::AnthropicMessages
            || model.endpoint() != EndpointPin::AnthropicApi
        {
            return Err(RequestBuildError::Encoding {
                reason: "the entry does not pin the anthropic messages dialect",
            });
        }
        if !model.capabilities().has(Capability::Streaming) {
            return Err(RequestBuildError::CapabilityUnavailable {
                capability: Capability::Streaming,
            });
        }

        let limits = model.limits();
        if request.max_output_tokens < limits.min_output_tokens
            || request.max_output_tokens > limits.max_output_tokens
        {
            return Err(RequestBuildError::OutputTokensOutOfRange {
                min: limits.min_output_tokens,
                max: limits.max_output_tokens,
            });
        }

        check_cache_breakpoints(model, request)?;
        let thinking = build_thinking(model, request)?;
        check_sampling(model, request)?;

        let estimated = estimate_prompt_tokens(request);
        if estimated > limits.context_window_tokens {
            return Err(RequestBuildError::ContextOverflowEstimated {
                limit: limits.context_window_tokens,
                estimated,
            });
        }

        let body = RequestBody {
            model: model.model().as_str(),
            max_tokens: request.max_output_tokens,
            messages: build_messages(model, request)?,
            system: build_system(model, request)?,
            temperature: request.temperature_milli.map(milli_to_unit),
            top_p: request.top_p_milli.map(milli_to_unit),
            stop_sequences: build_stop_sequences(model, request)?,
            tools: build_tools(model, request)?,
            tool_choice: build_tool_choice(model, request)?,
            thinking,
            output_config: build_output_config(model, request)?,
            stream: true,
        };

        let bytes = serde_json::to_vec(&body).map_err(|_| RequestBuildError::Encoding {
            reason: "the request body could not be rendered as JSON",
        })?;
        if u64::try_from(bytes.len()).unwrap_or(u64::MAX) > u64::from(limits.request_body_max_bytes)
        {
            return Err(RequestBuildError::BodyTooLarge {
                limit: limits.request_body_max_bytes,
            });
        }

        Ok(WireRequest {
            endpoint: EndpointPin::AnthropicApi,
            path: BoundedString::new(MESSAGES_PATH).map_err(|_| RequestBuildError::Encoding {
                reason: "the pinned path exceeds its bound",
            })?,
            query: Vec::new(),
            headers: vec![(
                VERSION_HEADER,
                BoundedString::new(API_VERSION).map_err(|_| RequestBuildError::Encoding {
                    reason: "the pinned version exceeds its bound",
                })?,
            )],
            auth: AuthScheme::AnthropicApiKey {
                version: API_VERSION,
            },
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
        state
            .ledger
            .charge_response(budget, byte_len(event.data.len()))?;
        if event.is_done_sentinel() {
            return Err(FrameDecodeError::UnknownEvent {
                event: BoundedString::truncating("[DONE]"),
            });
        }
        let payload: Value =
            serde_json::from_slice(event.data).map_err(|_| FrameDecodeError::NotJson)?;
        let kind = payload
            .get("type")
            .and_then(Value::as_str)
            .ok_or(FrameDecodeError::MalformedField { field: "type" })?;
        // The `event:` name is documented as mirrored in `data.type`. A
        // disagreement means the frame was rewritten in flight.
        if let Some(name) = event.name
            && name != kind
        {
            return Err(FrameDecodeError::MalformedField { field: "type" });
        }
        if state.terminal {
            return Err(FrameDecodeError::OutOfOrder {
                reason: "a frame arrived after message_stop",
            });
        }
        state.ledger.count_frame();

        match kind {
            "ping" => Ok(FrameOutcome::Ignored),
            "error" => Ok(FrameOutcome::Failed(Box::new(stream_failure(
                state, &payload,
            )))),
            "message_start" => decode_message_start(state, &payload),
            "content_block_start" => {
                require_started(state)?;
                decode_block_start(state, &payload, budget)
            }
            "content_block_delta" => {
                require_started(state)?;
                decode_block_delta(state, &payload, budget)
            }
            "content_block_stop" => {
                require_started(state)?;
                decode_block_stop(state, &payload)
            }
            "message_delta" => {
                require_started(state)?;
                decode_message_delta(state, &payload)
            }
            "message_stop" => {
                require_started(state)?;
                state.terminal = true;
                Ok(FrameOutcome::Terminal)
            }
            other => Err(FrameDecodeError::UnknownEvent {
                event: BoundedString::truncating(other),
            }),
        }
    }

    fn finish(&self, state: DialectState) -> Result<SealedResponse, FrameDecodeError> {
        if !state.terminal {
            return Err(FrameDecodeError::OutOfOrder {
                reason: "the stream ended without message_stop",
            });
        }
        if !state.open_text.is_empty()
            || !state.open_reasoning.is_empty()
            || !state.open_reasoning_token.is_empty()
            || !state.open_tools.is_empty()
        {
            return Err(FrameDecodeError::OutOfOrder {
                reason: "the stream ended with an unbalanced content block",
            });
        }
        let Some(StopResolution::Stop(stop_reason)) =
            state.finish_token.as_deref().map(resolve_stop)
        else {
            return Err(FrameDecodeError::MalformedField {
                field: "delta.stop_reason",
            });
        };
        let mut usage = state.usage;
        if usage == NormalizedUsage::default() {
            // Usage is never invented: an absent tally is recorded as absent.
            usage.completeness = UsageCompleteness::Absent;
        }
        Ok(SealedResponse {
            blocks: state.blocks,
            stop_reason,
            usage,
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
            .and_then(|error| error.get("type"))
            .and_then(Value::as_str);
        let message = error
            .and_then(|error| error.get("message"))
            .and_then(Value::as_str);

        let kind = code
            .and_then(kind_for_error_type)
            .unwrap_or_else(|| kind_for_status(status));
        let detail_message = match message {
            Some(text) => redact::<512>(text, &[]),
            None if body.is_truncated() => BoundedString::truncating(
                "the provider returned an error body larger than the read bound",
            ),
            None => BoundedString::truncating("the provider returned an unparsable error body"),
        };
        let mut detail = RedactedDetail::new(kind, detail_message).with_status(status);
        if let Some(code) = code {
            detail = detail.with_code(redact::<64>(code, &[]).as_str());
        }
        let mut failure = ProviderFailure::new(detail);
        failure.rate_limit = self.rate_limit_feedback(headers);
        failure
    }

    fn rate_limit_feedback(&self, headers: &HeaderView<'_>) -> RateLimitFeedback {
        let retry_after = headers.get_u64(RETRY_AFTER).map(Duration::from_secs);
        let requests_remaining = header_u32(*headers, "anthropic-ratelimit-requests-remaining");
        let tokens_remaining = header_u64(*headers, "anthropic-ratelimit-tokens-remaining")
            .or_else(|| {
                // Falling back to the tighter of the two sub-budgets, because
                // whichever runs out first is what actually blocks the caller.
                let input = header_u64(*headers, "anthropic-ratelimit-input-tokens-remaining");
                let output = header_u64(*headers, "anthropic-ratelimit-output-tokens-remaining");
                match (input, output) {
                    (Some(input), Some(output)) => Some(input.min(output)),
                    (Some(value), None) | (None, Some(value)) => Some(value),
                    (None, None) => None,
                }
            });
        let reset_at = RESET_HEADERS
            .iter()
            .filter_map(|name| headers.get(name))
            .filter_map(parse_rfc3339)
            .max();

        let vendor = VENDOR_RATE_LIMIT_HEADERS
            .iter()
            .any(|name| headers.get(name).is_some());
        let source = if vendor {
            RateLimitSource::VendorHeaders
        } else if retry_after.is_some() {
            RateLimitSource::RetryAfterHeader
        } else {
            RateLimitSource::NotProvided
        };

        RateLimitFeedback {
            retry_after,
            requests_remaining,
            tokens_remaining,
            reset_at,
            source,
        }
    }

    fn request_id(
        &self,
        headers: &HeaderView<'_>,
        state: &DialectState,
    ) -> Option<ProviderRequestId> {
        headers
            .get(REQUEST_ID_HEADER)
            .and_then(|text| ProviderRequestId::new(text).ok())
            .or_else(|| state.request_id.clone())
    }
}

// ---------------------------------------------------------------------------
// request building
// ---------------------------------------------------------------------------

/// Renders integer milli-units as the fractional value the provider takes.
///
/// The canonical request carries no floats; the wire does. This is the single
/// place the conversion happens.
fn milli_to_unit(milli: u16) -> f64 {
    f64::from(milli) / 1_000.0
}

fn byte_len(len: usize) -> u64 {
    u64::try_from(len).unwrap_or(u64::MAX)
}

const fn role_str(role: Role) -> &'static str {
    match role {
        Role::User => "user",
        Role::Assistant => "assistant",
    }
}

/// Whether the request has reasoning switched on, which is what makes
/// `excludes_sampling` bite.
fn reasoning_active(model: &QualifiedModel, request: &CanonicalModelRequest) -> bool {
    match request.reasoning {
        ReasoningRequest::Enabled { .. } => true,
        ReasoningRequest::Disabled => false,
        ReasoningRequest::ProviderDefault => {
            model.entry().reasoning.mode == ReasoningMode::AlwaysOn
        }
    }
}

fn check_cache_breakpoints(
    model: &QualifiedModel,
    request: &CanonicalModelRequest,
) -> Result<(), RequestBuildError> {
    let cacheable_system = request
        .system
        .iter()
        .filter(|block| block.cacheable)
        .count();
    let placed = request.cache_breakpoints.len() + cacheable_system;
    if placed == 0 {
        return Ok(());
    }
    let policy = model.entry().cache_policy;
    if !model.capabilities().has(Capability::PromptCacheExplicit)
        || policy.mode != CacheMode::Explicit
    {
        return Err(RequestBuildError::CapabilityUnavailable {
            capability: Capability::PromptCacheExplicit,
        });
    }
    if placed > usize::from(policy.max_breakpoints) {
        return Err(RequestBuildError::Encoding {
            reason: "the request places more cache breakpoints than the pair accepts",
        });
    }
    Ok(())
}

fn build_system(
    model: &QualifiedModel,
    request: &CanonicalModelRequest,
) -> Result<Option<Vec<WireSystemBlock>>, RequestBuildError> {
    if request.system.is_empty() {
        if request
            .cache_breakpoints
            .iter()
            .any(|point| matches!(point, CacheBreakpoint::AfterSystem))
        {
            return Err(RequestBuildError::Encoding {
                reason: "a cache breakpoint names a system prefix that is empty",
            });
        }
        return Ok(None);
    }
    if !model.capabilities().has(Capability::SystemInstruction) {
        return Err(RequestBuildError::CapabilityUnavailable {
            capability: Capability::SystemInstruction,
        });
    }
    let mut blocks: Vec<WireSystemBlock> = request
        .system
        .iter()
        .map(|block| WireSystemBlock {
            kind: "text",
            text: block.text.as_str().to_owned(),
            cache_control: block.cacheable.then(CacheControl::ephemeral),
        })
        .collect();
    if request
        .cache_breakpoints
        .iter()
        .any(|point| matches!(point, CacheBreakpoint::AfterSystem))
        && let Some(last) = blocks.last_mut()
    {
        last.cache_control = Some(CacheControl::ephemeral());
    }
    Ok(Some(blocks))
}

fn build_messages(
    model: &QualifiedModel,
    request: &CanonicalModelRequest,
) -> Result<Vec<WireMessage>, RequestBuildError> {
    for point in &request.cache_breakpoints {
        if let CacheBreakpoint::AfterMessage { index } = point
            && usize::from(*index) >= request.messages.len()
        {
            return Err(RequestBuildError::Encoding {
                reason: "a cache breakpoint names a message that does not exist",
            });
        }
    }
    let mut out = Vec::with_capacity(request.messages.len());
    for (position, message) in request.messages.iter().enumerate() {
        let mut content = Vec::with_capacity(message.blocks.len());
        for block in &message.blocks {
            content.push(wire_block(model, block)?);
        }
        if request.cache_breakpoints.iter().any(|point| {
            matches!(point, CacheBreakpoint::AfterMessage { index } if usize::from(*index) == position)
        }) {
            let Some(last) = content.last_mut() else {
                return Err(RequestBuildError::Encoding {
                    reason: "a cache breakpoint names a message with no content",
                });
            };
            last.cache_control = Some(CacheControl::ephemeral());
        }
        out.push(WireMessage {
            role: role_str(message.role),
            content,
        });
    }
    Ok(out)
}

fn wire_block(
    model: &QualifiedModel,
    block: &CanonicalBlock,
) -> Result<WireBlock, RequestBuildError> {
    match block {
        // A refusal was model prose; Anthropic has no separate input shape for
        // one, so it replays as the text it was.
        CanonicalBlock::Text { text, .. } => Ok(WireBlock::text(text.as_str())),
        CanonicalBlock::Refusal { text } => Ok(WireBlock::text(text.as_str())),
        CanonicalBlock::Reasoning(reasoning) => wire_reasoning(model, reasoning),
        CanonicalBlock::ToolUse { id, name, input } => Ok(WireBlock::tool_use(
            id.as_str(),
            name.as_str(),
            input.to_value(),
        )),
        CanonicalBlock::ToolResult {
            call,
            content,
            is_error,
        } => {
            let parts = content
                .iter()
                .map(|part| match part {
                    ToolResultPart::Text { text } => WireBlock::text(text.as_str()),
                    ToolResultPart::Json { value } => WireBlock::text(value.as_str()),
                })
                .collect();
            Ok(WireBlock::tool_result(call.as_str(), parts, *is_error))
        }
    }
}

fn wire_reasoning(
    model: &QualifiedModel,
    reasoning: &ReasoningBlock,
) -> Result<WireBlock, RequestBuildError> {
    // Anthropic rejects a replayed thinking block with no signature whatever
    // the entry says, so the material is required per block rather than per
    // turn.
    let Some(token) = &reasoning.token else {
        return Err(RequestBuildError::ReasoningTokenRequired);
    };
    if token.provenance != ProviderId::Anthropic {
        return Err(RequestBuildError::ReasoningProvenanceMismatch {
            expected: ProviderId::Anthropic,
            found: token.provenance,
        });
    }
    if !model.capabilities().has(Capability::ReasoningReplay) {
        return Err(RequestBuildError::CapabilityUnavailable {
            capability: Capability::ReasoningReplay,
        });
    }
    let signature =
        core::str::from_utf8(token.bytes.as_ref()).map_err(|_| RequestBuildError::Encoding {
            reason: "the anthropic signature is not valid UTF-8",
        })?;
    match &reasoning.body {
        ReasoningBody::Text { text } => Ok(WireBlock::thinking(text.as_str(), signature)),
        ReasoningBody::Redacted => Ok(WireBlock::redacted_thinking(signature)),
        ReasoningBody::Summary { .. } => Err(RequestBuildError::Encoding {
            reason: "anthropic exposes reasoning verbatim, never as a summary",
        }),
    }
}

fn build_tools(
    model: &QualifiedModel,
    request: &CanonicalModelRequest,
) -> Result<Vec<WireTool>, RequestBuildError> {
    let after_tools = request
        .cache_breakpoints
        .iter()
        .any(|point| matches!(point, CacheBreakpoint::AfterTools));
    if request.tools.is_empty() {
        if after_tools {
            return Err(RequestBuildError::Encoding {
                reason: "a cache breakpoint names a tool prefix that is empty",
            });
        }
        return Ok(Vec::new());
    }
    let capabilities = model.capabilities();
    if !capabilities.has(Capability::Tools) {
        return Err(RequestBuildError::CapabilityUnavailable {
            capability: Capability::Tools,
        });
    }
    let limits = model.limits();
    if request.tools.len() > usize::from(limits.max_tools) {
        return Err(RequestBuildError::ToolLimit {
            max: limits.max_tools,
        });
    }
    let policy = &model.entry().tool_policy;
    let mut out = Vec::with_capacity(request.tools.len());
    for tool in &request.tools {
        let name = tool.name.as_str();
        if name.len() > usize::from(policy.max_name_bytes) || !policy.name_pattern.accepts(name) {
            return Err(RequestBuildError::ToolNameInvalid {
                name: tool.name.clone(),
            });
        }
        if tool.strict && !capabilities.has(Capability::StrictToolSchema) {
            return Err(RequestBuildError::CapabilityUnavailable {
                capability: Capability::StrictToolSchema,
            });
        }
        out.push(WireTool {
            name: name.to_owned(),
            description: tool.description.as_str().to_owned(),
            input_schema: tool.input_schema.to_value(),
            strict: tool.strict.then_some(true),
            cache_control: None,
        });
    }
    if after_tools && let Some(last) = out.last_mut() {
        last.cache_control = Some(CacheControl::ephemeral());
    }
    Ok(out)
}

fn build_tool_choice(
    model: &QualifiedModel,
    request: &CanonicalModelRequest,
) -> Result<Option<WireToolChoice>, RequestBuildError> {
    if request.tools.is_empty() {
        return match request.tool_choice {
            ToolChoice::Auto | ToolChoice::None => Ok(None),
            ToolChoice::Required | ToolChoice::Named { .. } => Err(RequestBuildError::Encoding {
                reason: "a required or named tool choice needs at least one tool",
            }),
        };
    }
    let capabilities = model.capabilities();
    if request.parallel_tools && !capabilities.has(Capability::ParallelTools) {
        return Err(RequestBuildError::CapabilityUnavailable {
            capability: Capability::ParallelTools,
        });
    }
    // Never silently dropped: when the caller forbids parallel calls the
    // dialect's own switch carries the constraint.
    let disable = (!request.parallel_tools).then_some(true);
    let unsupported = || RequestBuildError::ToolChoiceUnsupported {
        requested: Box::new(request.tool_choice.clone()),
    };
    match &request.tool_choice {
        ToolChoice::Auto => Ok(Some(WireToolChoice {
            kind: "auto",
            name: None,
            disable_parallel_tool_use: disable,
        })),
        ToolChoice::Required => {
            if !capabilities.has(Capability::ToolChoiceRequired) {
                return Err(unsupported());
            }
            Ok(Some(WireToolChoice {
                kind: "any",
                name: None,
                disable_parallel_tool_use: disable,
            }))
        }
        ToolChoice::Named { name } => {
            if !capabilities.has(Capability::ToolChoiceNamed) {
                return Err(unsupported());
            }
            Ok(Some(WireToolChoice {
                kind: "tool",
                name: Some(name.as_str().to_owned()),
                disable_parallel_tool_use: disable,
            }))
        }
        ToolChoice::None => {
            if !capabilities.has(Capability::ToolChoiceNone) {
                return Err(unsupported());
            }
            Ok(Some(WireToolChoice {
                kind: "none",
                name: None,
                disable_parallel_tool_use: None,
            }))
        }
    }
}

fn build_output_config(
    model: &QualifiedModel,
    request: &CanonicalModelRequest,
) -> Result<Option<WireOutputConfig>, RequestBuildError> {
    let Some(requested) = &request.structured_output else {
        return Ok(None);
    };
    let unsupported = || RequestBuildError::StructuredOutputUnsupported {
        requested: Box::new(requested.clone()),
    };
    if !model.capabilities().has(Capability::StructuredOutput) {
        return Err(RequestBuildError::CapabilityUnavailable {
            capability: Capability::StructuredOutput,
        });
    }
    let StructuredOutputPolicy::JsonSchema {
        encoding: SchemaEncoding::AnthropicOutputConfig,
        strict_default,
    } = model.entry().structured_output
    else {
        return Err(unsupported());
    };
    match requested {
        // `output_config.format` always takes a schema; there is no
        // "any JSON object" form to fall back to.
        StructuredOutputRequest::JsonObject => Err(unsupported()),
        StructuredOutputRequest::JsonSchema { schema, strict, .. } => {
            // The format carries no strictness switch, so the entry's default
            // is the only strictness this pair can offer.
            if *strict != strict_default {
                return Err(unsupported());
            }
            Ok(Some(WireOutputConfig {
                format: WireOutputFormat {
                    kind: "json_schema",
                    schema: schema.to_value(),
                },
            }))
        }
    }
}

fn build_stop_sequences(
    model: &QualifiedModel,
    request: &CanonicalModelRequest,
) -> Result<Vec<String>, RequestBuildError> {
    if request.stop_sequences.is_empty() {
        return Ok(Vec::new());
    }
    if !model.capabilities().has(Capability::StopSequences) {
        return Err(RequestBuildError::CapabilityUnavailable {
            capability: Capability::StopSequences,
        });
    }
    let max = model.limits().max_stop_sequences;
    if request.stop_sequences.len() > usize::from(max) {
        return Err(RequestBuildError::StopSequenceLimit { max });
    }
    Ok(request
        .stop_sequences
        .iter()
        .map(|sequence| sequence.as_str().to_owned())
        .collect())
}

fn build_thinking(
    model: &QualifiedModel,
    request: &CanonicalModelRequest,
) -> Result<Option<WireThinking>, RequestBuildError> {
    let policy = model.entry().reasoning;
    let capabilities = model.capabilities();
    match request.reasoning {
        ReasoningRequest::ProviderDefault => Ok(None),
        ReasoningRequest::Disabled => {
            if policy.mode == ReasoningMode::AlwaysOn {
                return Err(RequestBuildError::CapabilityUnavailable {
                    capability: Capability::Reasoning,
                });
            }
            if !capabilities.has(Capability::Reasoning) {
                return Ok(None);
            }
            Ok(Some(WireThinking {
                kind: "disabled",
                budget_tokens: None,
            }))
        }
        ReasoningRequest::Enabled {
            budget_tokens,
            effort,
        } => {
            if !capabilities.has(Capability::Reasoning) || policy.mode == ReasoningMode::Unsupported
            {
                return Err(RequestBuildError::CapabilityUnavailable {
                    capability: Capability::Reasoning,
                });
            }
            if effort.is_some() {
                return Err(RequestBuildError::Encoding {
                    reason: "anthropic takes a thinking budget, never an effort level",
                });
            }
            let limits = model.limits();
            let floor = limits
                .min_reasoning_tokens
                .unwrap_or(MIN_THINKING_BUDGET)
                .max(MIN_THINKING_BUDGET);
            // The documented rule is `budget_tokens < max_tokens`, so the
            // ceiling is one below the output ceiling of this very request.
            let ceiling = limits
                .max_reasoning_tokens
                .unwrap_or(u32::MAX)
                .min(request.max_output_tokens.saturating_sub(1));
            let out_of_range = RequestBuildError::ReasoningBudgetOutOfRange {
                min: floor,
                max: ceiling,
            };
            let Some(budget) = budget_tokens else {
                return Err(out_of_range);
            };
            if budget < floor || budget > ceiling {
                return Err(out_of_range);
            }
            Ok(Some(WireThinking {
                kind: "enabled",
                budget_tokens: Some(budget),
            }))
        }
    }
}

fn check_sampling(
    model: &QualifiedModel,
    request: &CanonicalModelRequest,
) -> Result<(), RequestBuildError> {
    let entry = model.entry();
    let capabilities = model.capabilities();
    let thinking = reasoning_active(model, request);

    if let Some(value) = request.temperature_milli {
        if thinking && entry.reasoning.excludes_sampling {
            return Err(RequestBuildError::SamplingWithReasoning {
                field: "temperature",
            });
        }
        if !capabilities.has(Capability::Temperature) || entry.sampling == SamplingSupport::None {
            return Err(RequestBuildError::SamplingUnsupported {
                field: "temperature",
            });
        }
        let Some((low, high)) = entry.limits.temperature_milli else {
            return Err(RequestBuildError::SamplingUnsupported {
                field: "temperature",
            });
        };
        if value < low || value > high {
            return Err(RequestBuildError::SamplingUnsupported {
                field: "temperature",
            });
        }
    }

    if let Some(value) = request.top_p_milli {
        if thinking && entry.reasoning.excludes_sampling {
            return Err(RequestBuildError::SamplingWithReasoning { field: "top_p" });
        }
        if !capabilities.has(Capability::TopP) || entry.sampling != SamplingSupport::Full {
            return Err(RequestBuildError::SamplingUnsupported { field: "top_p" });
        }
        let Some((low, high)) = entry.limits.top_p_milli else {
            return Err(RequestBuildError::SamplingUnsupported { field: "top_p" });
        };
        if value < low || value > high {
            return Err(RequestBuildError::SamplingUnsupported { field: "top_p" });
        }
    }

    Ok(())
}

fn estimate_prompt_tokens(request: &CanonicalModelRequest) -> u32 {
    let mut bytes: u64 = 0;
    for block in &request.system {
        bytes = bytes.saturating_add(byte_len(block.text.len()));
    }
    for tool in &request.tools {
        bytes = bytes.saturating_add(byte_len(tool.name.as_str().len()));
        bytes = bytes.saturating_add(byte_len(tool.description.len()));
        bytes = bytes.saturating_add(byte_len(tool.input_schema.as_str().len()));
    }
    for message in &request.messages {
        for block in &message.blocks {
            bytes = bytes.saturating_add(block_bytes(block));
        }
    }
    u32::try_from(bytes.div_ceil(BYTES_PER_TOKEN)).unwrap_or(u32::MAX)
}

fn block_bytes(block: &CanonicalBlock) -> u64 {
    match block {
        CanonicalBlock::Text { text, .. } => byte_len(text.len()),
        CanonicalBlock::Refusal { text } => byte_len(text.len()),
        CanonicalBlock::Reasoning(reasoning) => {
            let body = match &reasoning.body {
                ReasoningBody::Text { text } | ReasoningBody::Summary { text } => {
                    byte_len(text.len())
                }
                ReasoningBody::Redacted => 0,
            };
            body.saturating_add(
                reasoning
                    .token
                    .as_ref()
                    .map_or(0, |token| byte_len(token.bytes.len())),
            )
        }
        CanonicalBlock::ToolUse { name, input, .. } => {
            byte_len(name.as_str().len()).saturating_add(byte_len(input.as_str().len()))
        }
        CanonicalBlock::ToolResult { content, .. } => content
            .iter()
            .map(|part| match part {
                ToolResultPart::Text { text } => byte_len(text.len()),
                ToolResultPart::Json { value } => byte_len(value.as_str().len()),
            })
            .fold(0u64, u64::saturating_add),
    }
}

// ---------------------------------------------------------------------------
// frame decoding
// ---------------------------------------------------------------------------

fn require_started(state: &DialectState) -> Result<(), FrameDecodeError> {
    if state.response_started {
        return Ok(());
    }
    Err(FrameDecodeError::OutOfOrder {
        reason: "a dialect frame arrived before message_start",
    })
}

fn frame_index(payload: &Value) -> Result<u16, FrameDecodeError> {
    payload
        .get("index")
        .and_then(Value::as_u64)
        .and_then(|value| u16::try_from(value).ok())
        .ok_or(FrameDecodeError::MalformedField { field: "index" })
}

fn decode_message_start(
    state: &mut DialectState,
    payload: &Value,
) -> Result<FrameOutcome, FrameDecodeError> {
    if state.response_started {
        return Err(FrameDecodeError::OutOfOrder {
            reason: "message_start arrived twice in one stream",
        });
    }
    let message = payload
        .get("message")
        .ok_or(FrameDecodeError::MalformedField { field: "message" })?;
    if let Some(usage) = message.get("usage") {
        apply_usage(&mut state.usage, usage)?;
    }
    Ok(state.mark_started())
}

fn decode_block_start(
    state: &mut DialectState,
    payload: &Value,
    budget: &StreamBudget,
) -> Result<FrameOutcome, FrameDecodeError> {
    let index = frame_index(payload)?;
    if state.open_text.contains_key(&index)
        || state.open_reasoning.contains_key(&index)
        || state.open_reasoning_token.contains_key(&index)
        || state.open_tools.contains_key(&index)
    {
        return Err(FrameDecodeError::OutOfOrder {
            reason: "content_block_start reopened an index that is already open",
        });
    }
    let block = payload.get("content_block").ok_or({
        FrameDecodeError::MalformedField {
            field: "content_block",
        }
    })?;
    let kind = block.get("type").and_then(Value::as_str).ok_or({
        FrameDecodeError::MalformedField {
            field: "content_block.type",
        }
    })?;
    state.ledger.open_block(budget)?;
    match kind {
        "text" => {
            state.open_text.insert(
                index,
                block
                    .get("text")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned(),
            );
        }
        "thinking" => {
            state.open_reasoning.insert(
                index,
                block
                    .get("thinking")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned(),
            );
        }
        "redacted_thinking" => {
            // No text ever arrives for one of these, so the token map alone
            // records that the index is open and that its body is redacted.
            let data = block.get("data").and_then(Value::as_str).ok_or({
                FrameDecodeError::MalformedField {
                    field: "content_block.data",
                }
            })?;
            state
                .open_reasoning_token
                .insert(index, data.as_bytes().to_vec());
        }
        "tool_use" => {
            state.ledger.open_tool_call(budget)?;
            let id = block.get("id").and_then(Value::as_str).ok_or({
                FrameDecodeError::MalformedField {
                    field: "content_block.id",
                }
            })?;
            let name = block.get("name").and_then(Value::as_str).ok_or({
                FrameDecodeError::MalformedField {
                    field: "content_block.name",
                }
            })?;
            let id = ToolCallId::new(id).map_err(|_| FrameDecodeError::MalformedField {
                field: "content_block.id",
            })?;
            state.open_tools.insert(
                index,
                PartialToolCall {
                    id: Some(id),
                    name: Some(name.to_owned()),
                    arguments: String::new(),
                },
            );
        }
        // A `server_tool_use` or `web_search_tool_result` block is a
        // provider-hosted tool AEX never enables, so receiving one means the
        // request was altered in flight (D-25).
        other => {
            return Err(FrameDecodeError::UnknownEvent {
                event: BoundedString::truncating(other),
            });
        }
    }
    Ok(FrameOutcome::Progress)
}

fn decode_block_delta(
    state: &mut DialectState,
    payload: &Value,
    budget: &StreamBudget,
) -> Result<FrameOutcome, FrameDecodeError> {
    let index = frame_index(payload)?;
    let delta = payload
        .get("delta")
        .ok_or(FrameDecodeError::MalformedField { field: "delta" })?;
    let kind =
        delta
            .get("type")
            .and_then(Value::as_str)
            .ok_or(FrameDecodeError::MalformedField {
                field: "delta.type",
            })?;
    match kind {
        "text_delta" => {
            let text = delta.get("text").and_then(Value::as_str).ok_or(
                FrameDecodeError::MalformedField {
                    field: "delta.text",
                },
            )?;
            let Some(open) = state.open_text.get_mut(&index) else {
                return Err(FrameDecodeError::OutOfOrder {
                    reason: "a text delta arrived for a block that is not open as text",
                });
            };
            open.push_str(text);
            state.ledger.charge_text(budget, byte_len(text.len()))?;
        }
        "thinking_delta" => {
            let text = delta.get("thinking").and_then(Value::as_str).ok_or({
                FrameDecodeError::MalformedField {
                    field: "delta.thinking",
                }
            })?;
            let Some(open) = state.open_reasoning.get_mut(&index) else {
                return Err(FrameDecodeError::OutOfOrder {
                    reason: "a thinking delta arrived for a block that is not open as thinking",
                });
            };
            open.push_str(text);
            state
                .ledger
                .charge_reasoning(budget, byte_len(text.len()))?;
        }
        "signature_delta" => {
            let signature = delta.get("signature").and_then(Value::as_str).ok_or({
                FrameDecodeError::MalformedField {
                    field: "delta.signature",
                }
            })?;
            if !state.open_reasoning.contains_key(&index) {
                return Err(FrameDecodeError::OutOfOrder {
                    reason: "a signature delta arrived for a block that is not open as thinking",
                });
            }
            if state.open_reasoning_token.contains_key(&index) {
                return Err(FrameDecodeError::OutOfOrder {
                    reason: "a thinking block carries exactly one signature delta",
                });
            }
            state
                .open_reasoning_token
                .insert(index, signature.as_bytes().to_vec());
        }
        "input_json_delta" => {
            let fragment = delta.get("partial_json").and_then(Value::as_str).ok_or({
                FrameDecodeError::MalformedField {
                    field: "delta.partial_json",
                }
            })?;
            let Some(open) = state.open_tools.get_mut(&index) else {
                return Err(FrameDecodeError::OutOfOrder {
                    reason: "an argument delta arrived for a block that is not open as a tool call",
                });
            };
            open.arguments.push_str(fragment);
            let accumulated = open.arguments.len();
            let call = open.id.clone().ok_or({
                FrameDecodeError::MalformedField {
                    field: "content_block.id",
                }
            })?;
            BudgetLedger::check_tool_arguments(budget, &call, accumulated)?;
        }
        other => {
            return Err(FrameDecodeError::UnknownEvent {
                event: BoundedString::truncating(other),
            });
        }
    }
    Ok(FrameOutcome::Progress)
}

fn decode_block_stop(
    state: &mut DialectState,
    payload: &Value,
) -> Result<FrameOutcome, FrameDecodeError> {
    let index = frame_index(payload)?;
    if let Some(text) = state.open_text.remove(&index) {
        let bounded = BoundedString::<TEXT_MAX>::new(text).map_err(|_| {
            FrameDecodeError::Budget(BudgetOverrun::Text {
                limit: byte_len(TEXT_MAX),
            })
        })?;
        state.blocks.push(CanonicalBlock::Text {
            text: bounded,
            annotations: Vec::new(),
        });
        return Ok(FrameOutcome::Progress);
    }
    if let Some(text) = state.open_reasoning.remove(&index) {
        let Some(signature) = state.open_reasoning_token.remove(&index) else {
            return Err(FrameDecodeError::MalformedField {
                field: "delta.signature",
            });
        };
        let bounded = BoundedString::<REASON_MAX>::new(text).map_err(|_| {
            FrameDecodeError::Budget(BudgetOverrun::Reasoning {
                limit: byte_len(REASON_MAX),
            })
        })?;
        state.blocks.push(CanonicalBlock::Reasoning(ReasoningBlock {
            body: ReasoningBody::Text { text: bounded },
            token: Some(anthropic_token(signature)),
        }));
        return Ok(FrameOutcome::Progress);
    }
    if let Some(signature) = state.open_reasoning_token.remove(&index) {
        state.blocks.push(CanonicalBlock::Reasoning(ReasoningBlock {
            body: ReasoningBody::Redacted,
            token: Some(anthropic_token(signature)),
        }));
        return Ok(FrameOutcome::Progress);
    }
    if let Some(partial) = state.open_tools.remove(&index) {
        let call = partial.id.ok_or({
            FrameDecodeError::MalformedField {
                field: "content_block.id",
            }
        })?;
        let raw_name = partial.name.ok_or({
            FrameDecodeError::MalformedField {
                field: "content_block.name",
            }
        })?;
        let name = ToolName::parse(&raw_name).map_err(|_| FrameDecodeError::MalformedField {
            field: "content_block.name",
        })?;
        // A tool whose input is `{}` gets no `input_json_delta` at all, which
        // is the empty accumulation rather than a broken one.
        let raw = if partial.arguments.trim().is_empty() {
            "{}"
        } else {
            partial.arguments.as_str()
        };
        let input = CanonicalJson::parse(raw)
            .map_err(|_| FrameDecodeError::ToolArgumentsNotJson { call: call.clone() })?;
        state.blocks.push(CanonicalBlock::ToolUse {
            id: call,
            name,
            input,
        });
        return Ok(FrameOutcome::Progress);
    }
    Err(FrameDecodeError::OutOfOrder {
        reason: "content_block_stop names a block that is not open",
    })
}

fn anthropic_token(signature: Vec<u8>) -> ReasoningToken {
    ReasoningToken {
        provenance: ProviderId::Anthropic,
        bytes: Bytes::from(signature),
    }
}

fn decode_message_delta(
    state: &mut DialectState,
    payload: &Value,
) -> Result<FrameOutcome, FrameDecodeError> {
    if let Some(usage) = payload.get("usage") {
        apply_usage(&mut state.usage, usage)?;
    }
    let Some(token) = payload
        .get("delta")
        .and_then(|delta| delta.get("stop_reason"))
        .and_then(Value::as_str)
    else {
        return Ok(FrameOutcome::Progress);
    };
    match resolve_stop(token) {
        // A refusal that produced nothing is a content filter, not a turn.
        StopResolution::Stop(StopReason::Refusal) if !has_non_refusal(&state.blocks) => Ok(
            FrameOutcome::Failed(Box::new(ProviderFailure::new(RedactedDetail::internal(
                ProviderFailureKind::ContentFiltered,
                "the model refused without producing any content",
            )))),
        ),
        StopResolution::Stop(_) => {
            state.finish_token = Some(token.to_owned());
            Ok(FrameOutcome::Progress)
        }
        StopResolution::Failure(kind) => Ok(FrameOutcome::Failed(Box::new(ProviderFailure::new(
            RedactedDetail::internal(kind, stop_failure_message(kind)),
        )))),
        StopResolution::Unknown => Err(FrameDecodeError::MalformedField {
            field: "delta.stop_reason",
        }),
    }
}

fn has_non_refusal(blocks: &[CanonicalBlock]) -> bool {
    blocks
        .iter()
        .any(|block| !matches!(block, CanonicalBlock::Refusal { .. }))
}

/// Overwrites the usage tally with whatever the frame reports.
///
/// **`message_delta.usage` is cumulative.** Summing would double-count every
/// token on the second and later reports, so each present field replaces its
/// counterpart and each absent field is left exactly as it was — which is what
/// preserves `message_start`'s prompt accounting through a `message_delta` that
/// only carries `output_tokens`.
fn apply_usage(usage: &mut NormalizedUsage, value: &Value) -> Result<(), FrameDecodeError> {
    if let Some(reported) = value.get("input_tokens").and_then(Value::as_u64) {
        usage.input_tokens = reported;
    }
    if let Some(reported) = value
        .get("cache_creation_input_tokens")
        .and_then(Value::as_u64)
    {
        usage.cache_write_input_tokens = reported;
    }
    if let Some(reported) = value.get("cache_read_input_tokens").and_then(Value::as_u64) {
        usage.cache_read_input_tokens = reported;
    }
    if let Some(reported) = value.get("output_tokens").and_then(Value::as_u64) {
        usage.output_tokens = reported;
    }
    // `reasoning_included_in_output` is true here, so the thinking count is
    // taken as the subset it is and never added to the output count.
    if let Some(reported) = value
        .get("output_tokens_details")
        .and_then(|details| details.get("thinking_tokens"))
        .and_then(Value::as_u64)
    {
        usage.reasoning_tokens = reported;
    }
    if !usage.is_consistent() {
        return Err(FrameDecodeError::MalformedField { field: "usage" });
    }
    Ok(())
}

fn stream_failure(state: &mut DialectState, payload: &Value) -> ProviderFailure {
    if let Some(id) = payload.get("request_id").and_then(Value::as_str)
        && let Ok(bounded) = ProviderRequestId::new(id)
    {
        state.request_id = Some(bounded);
    }
    let error = payload.get("error");
    let code = error
        .and_then(|error| error.get("type"))
        .and_then(Value::as_str);
    let message = error
        .and_then(|error| error.get("message"))
        .and_then(Value::as_str);
    let kind = code
        .and_then(kind_for_error_type)
        .unwrap_or(ProviderFailureKind::ServerError);
    let detail_message = message.map_or_else(
        || BoundedString::truncating("the provider reported an error mid-stream"),
        |text| redact::<512>(text, &[]),
    );
    let mut detail = RedactedDetail::new(kind, detail_message);
    if let Some(code) = code {
        detail = detail.with_code(redact::<64>(code, &[]).as_str());
    }
    ProviderFailure::new(detail)
}

// ---------------------------------------------------------------------------
// the documented tables
// ---------------------------------------------------------------------------

/// The complete finish-token table (plan 08 §5.2).
///
/// A safety block and a context overflow are **failures**, not stop reasons
/// (D-09): `pause_turn` implies a server tool AEX never enables, so it means
/// the request was altered in flight rather than that the turn ended.
fn resolve_stop(token: &str) -> StopResolution {
    match token {
        "end_turn" => StopResolution::Stop(StopReason::EndTurn),
        "tool_use" => StopResolution::Stop(StopReason::ToolUse),
        "max_tokens" => StopResolution::Stop(StopReason::MaxOutputTokens),
        "stop_sequence" => StopResolution::Stop(StopReason::StopSequence),
        "refusal" => StopResolution::Stop(StopReason::Refusal),
        "model_context_window_exceeded" => {
            StopResolution::Failure(ProviderFailureKind::ContextOverflow)
        }
        "pause_turn" => StopResolution::Failure(ProviderFailureKind::ProtocolViolation),
        _ => StopResolution::Unknown,
    }
}

const fn stop_failure_message(kind: ProviderFailureKind) -> &'static str {
    match kind {
        ProviderFailureKind::ContextOverflow => {
            "the provider reported that the model context window was exceeded"
        }
        ProviderFailureKind::ProtocolViolation => {
            "the provider paused the turn for a server tool this request never enabled"
        }
        _ => "the provider reported a terminal failure in place of a stop reason",
    }
}

/// The complete `error.type` table (plan 08 §5.2).
///
/// `None` means the string is outside the documented set, in which case the
/// status is the answer rather than a guess.
fn kind_for_error_type(code: &str) -> Option<ProviderFailureKind> {
    Some(match code {
        // `request_too_large` is a byte bound on the request, not a token bound
        // on the window, so it joins the other malformed-request answers rather
        // than `ContextOverflow`.
        "invalid_request_error" | "conflict_error" | "request_too_large" => {
            ProviderFailureKind::InvalidRequest
        }
        // A permission refusal is a credential-shaped answer: the binding is
        // flagged for the workspace and never auto-rotated (§6.4).
        "authentication_error" | "permission_error" => ProviderFailureKind::Authentication,
        "billing_error" => ProviderFailureKind::Billing,
        "not_found_error" => ProviderFailureKind::ModelNotFound,
        "rate_limit_error" => ProviderFailureKind::RateLimited,
        "api_error" => ProviderFailureKind::ServerError,
        "timeout_error" => ProviderFailureKind::Timeout,
        "overloaded_error" => ProviderFailureKind::Overloaded,
        _ => return None,
    })
}

/// The status fallback, for a body that carried no usable `error.type`.
const fn kind_for_status(status: u16) -> ProviderFailureKind {
    match status {
        400 | 409 | 413 => ProviderFailureKind::InvalidRequest,
        401 | 403 => ProviderFailureKind::Authentication,
        402 => ProviderFailureKind::Billing,
        404 => ProviderFailureKind::ModelNotFound,
        429 => ProviderFailureKind::RateLimited,
        504 => ProviderFailureKind::Timeout,
        529 => ProviderFailureKind::Overloaded,
        _ => ProviderFailureKind::ServerError,
    }
}

fn header_u32(headers: HeaderView<'_>, name: &str) -> Option<u32> {
    headers
        .get(name)
        .and_then(|text| text.trim().parse::<u32>().ok())
}

fn header_u64(headers: HeaderView<'_>, name: &str) -> Option<u64> {
    headers
        .get(name)
        .and_then(|text| text.trim().parse::<u64>().ok())
}

/// Parses one of the RFC 3339 reset stamps this dialect publishes.
///
/// The wire spelling Anthropic uses (`2026-08-01T12:00:00Z`, no fractional
/// part) is a wider grammar than [`Timestamp::parse`] accepts, so the parse
/// goes through `time` and is then truncated to whole milliseconds.
fn parse_rfc3339(text: &str) -> Option<Timestamp> {
    time::OffsetDateTime::parse(text.trim(), &time::format_description::well_known::Rfc3339)
        .ok()
        .and_then(|value| Timestamp::from_datetime_trunc_ms(value).ok())
}

#[cfg(test)]
mod tests {
    use aex_model_catalog::canonical::{
        CacheBreakpoint, CanonicalBlock, CanonicalMessage, CanonicalModelRequest, CanonicalToolDef,
        CorrelationId, NormalizedUsage, ReasoningBlock, ReasoningBody, ReasoningEffort,
        ReasoningRequest, ReasoningToken, Role, StopReason, StructuredOutputRequest, SystemBlock,
        ToolChoice, ToolResultPart, UsageCompleteness,
    };
    use aex_model_catalog::catalog::Catalog;
    use aex_model_catalog::document::{
        CacheMode, CachePolicy, CacheReadSemantics, Capability, CapabilitySet, CodeClassRule,
        Dialect, EndpointPin, EntryState, ErrorClassMap, ModelEntry, NamePattern,
        ReasoningEncoding, ReasoningMode, ReasoningPolicy, ReasoningReplay, SamplingSupport,
        SchemaEncoding, StatusClassRule, StopFailureMapping, StopReasonMap, StopResolution,
        StreamUsageDelivery, StructuredOutputPolicy, ToolArgumentEncoding, ToolEncoding,
        ToolPolicy, UsageMapping,
    };
    use aex_model_catalog::primitives::{BoundedString, ToolCallId, ToolName};
    use aex_model_catalog::signature::{
        CatalogEnvelope, CatalogSignature, P256_PUBLIC_KEY_BYTES, SigAlg, SigningKeyId, TrustedKey,
        TrustedKeys,
    };
    use aex_model_catalog::{QualifiedModel, fixture};
    use aex_wire::provider::{ModelSelection, ProviderId};
    use aex_wire::{CanonicalJson, ContentHash};

    use super::{
        API_VERSION, AnthropicAdapter, MESSAGES_PATH, VERSION_HEADER, kind_for_error_type,
        kind_for_status, resolve_stop,
    };
    use crate::adapter::{
        BoundedBody, DialectState, FrameDecodeError, FrameOutcome, HeaderView, ProviderAdapter,
        RequestBuildError, SealedResponse,
    };
    use crate::budget::StreamBudget;
    use crate::error::{ProviderFailureKind, RateLimitSource};
    use crate::sse::SseEvent;
    use crate::transport::{Accept, AuthScheme};

    // -----------------------------------------------------------------------
    // fixture catalog
    //
    // `Catalog::load` is the only public way to mint a `QualifiedModel`, and it
    // demands a real P-256 signature over the exact canonical document bytes.
    // This crate has no signing dependency, so the key pair and the signature
    // below were produced once, offline, over the bytes `document()` renders.
    // Every variant this file needs is a separate model slug inside that one
    // document, so there is exactly one signature to keep in step.
    // -----------------------------------------------------------------------

    const NOW_MS: i64 = 1_800_000_000_000;
    const PUBLISHER: &str = "aex-catalog-test";

    /// The full-capability pair every happy-path test uses.
    const FULL: &str = "claude-opus-5";
    /// Declares nothing at all, including `Streaming`.
    const MINIMAL: &str = "claude-minimal";
    /// Streams text and nothing else, so every optional capability gate fires.
    const PLAIN: &str = "claude-plain";
    /// Streams text, tools and reasoning, but no tool-choice mode, no parallel
    /// calls, no strict schemas and no reasoning replay.
    const TOOLS_ONLY: &str = "claude-tools-only";
    /// Narrow countable bounds: one tool, one stop sequence, a small output
    /// range and a small reasoning ceiling.
    const TIGHT: &str = "claude-tight";
    /// A tiny context window and a tiny request-body bound.
    const NARROW: &str = "claude-narrow";
    /// Reasoning that forbids sampling, the `SamplingWithReasoning` path.
    const NO_SAMPLING: &str = "claude-thinking-only";

    /// The uncompressed P-256 point of the throwaway fixture key.
    const PUBLIC_KEY_HEX: &str = concat!(
        "043108269ad7e7682d85f54d725f45fb627a06dd41b0f441621a98bf990fd83849",
        "70fca0edc09b1d3d18e54493af8add656c926e2f3c5215505af5ae2b80810906"
    );
    /// `ECDSA_P256_SHA256_ASN1` over `SIGNING_PREFIX || canonical_bytes(document())`.
    ///
    /// Regenerate by dumping `fixture::canonical_bytes(&document())`, prefixing
    /// `aex-model-catalog/v1\n`, and signing with the matching private key.
    const SIGNATURE_HEX: &str = concat!(
        "3046022100eed6cba7ebbdf1a6797b2bc8050d0a6aee2268d68437bdea92ba476d",
        "f1a602b5022100e3ccfae14f6f78e4721aca5e8aaada3793138f8a0fab0c000a2c",
        "1b89c5c681c2"
    );

    fn from_hex(text: &str) -> Vec<u8> {
        assert!(
            text.len().is_multiple_of(2),
            "a hex literal has even length"
        );
        text.as_bytes()
            .chunks_exact(2)
            .map(|pair| {
                let high = char::from(pair[0]).to_digit(16).expect("hex digit");
                let low = char::from(pair[1]).to_digit(16).expect("hex digit");
                u8::try_from((high << 4) | low).expect("a byte")
            })
            .collect()
    }

    fn anthropic_stop_map() -> StopReasonMap {
        StopReasonMap {
            end_turn: vec![fixture::bounded("end_turn")],
            tool_use: vec![fixture::bounded("tool_use")],
            max_output_tokens: vec![fixture::bounded("max_tokens")],
            stop_sequence: vec![fixture::bounded("stop_sequence")],
            refusal: vec![fixture::bounded("refusal")],
            failure: vec![
                StopFailureMapping {
                    token: fixture::bounded("model_context_window_exceeded"),
                    kind: ProviderFailureKind::ContextOverflow,
                },
                StopFailureMapping {
                    token: fixture::bounded("pause_turn"),
                    kind: ProviderFailureKind::ProtocolViolation,
                },
            ],
        }
    }

    fn anthropic_error_map() -> ErrorClassMap {
        ErrorClassMap {
            status: vec![
                StatusClassRule {
                    status: 401,
                    kind: ProviderFailureKind::Authentication,
                },
                StatusClassRule {
                    status: 429,
                    kind: ProviderFailureKind::RateLimited,
                },
                StatusClassRule {
                    status: 529,
                    kind: ProviderFailureKind::Overloaded,
                },
            ],
            codes: vec![CodeClassRule {
                code: fixture::bounded("billing_error"),
                kind: ProviderFailureKind::Billing,
            }],
            default_kind: ProviderFailureKind::ServerError,
        }
    }

    fn full_capabilities() -> CapabilitySet {
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
            Capability::PromptCacheExplicit,
            Capability::StopSequences,
            Capability::Temperature,
            Capability::TopP,
            Capability::SystemInstruction,
        ])
    }

    fn anthropic_entry(model: &str, capabilities: CapabilitySet) -> ModelEntry {
        let mut entry = fixture::entry(ProviderId::Anthropic, model, capabilities);
        entry.reasoning = ReasoningPolicy {
            mode: ReasoningMode::Optional,
            encoding: ReasoningEncoding::AnthropicThinking,
            replay: ReasoningReplay::RequiredAlways,
            excludes_sampling: false,
        };
        entry.structured_output = StructuredOutputPolicy::JsonSchema {
            encoding: SchemaEncoding::AnthropicOutputConfig,
            strict_default: true,
        };
        entry.tool_policy = ToolPolicy {
            encoding: ToolEncoding::AnthropicInputSchema,
            arguments: ToolArgumentEncoding::JsonObject,
            requires_stream_opt_in: false,
            max_name_bytes: 64,
            name_pattern: NamePattern::AnthropicToolName,
        };
        entry.cache_policy = CachePolicy {
            mode: CacheMode::Explicit,
            max_breakpoints: 4,
        };
        entry.usage_map = UsageMapping {
            reasoning_included_in_output: true,
            cache_read_field: CacheReadSemantics::SeparateReadWrite,
            stream_usage_delivery: StreamUsageDelivery::CumulativeDeltas,
            known_missing: aex_model_catalog::canonical::UsageFieldSet::EMPTY,
        };
        entry.stop_reason_map = anthropic_stop_map();
        entry.error_map = anthropic_error_map();
        entry.sampling = SamplingSupport::Full;
        entry
    }

    fn entries() -> Vec<ModelEntry> {
        let mut minimal = anthropic_entry(MINIMAL, CapabilitySet::EMPTY);
        minimal.cache_policy = CachePolicy {
            mode: CacheMode::None,
            max_breakpoints: 0,
        };
        minimal.sampling = SamplingSupport::None;
        minimal.structured_output = StructuredOutputPolicy::JsonObjectOnly;

        let plain = anthropic_entry(
            PLAIN,
            CapabilitySet::from_slice(&[
                Capability::TextIn,
                Capability::TextOut,
                Capability::Streaming,
            ]),
        );

        let tools_only = anthropic_entry(
            TOOLS_ONLY,
            CapabilitySet::from_slice(&[
                Capability::TextIn,
                Capability::TextOut,
                Capability::Streaming,
                Capability::Tools,
                Capability::Reasoning,
            ]),
        );

        let mut tight = anthropic_entry(TIGHT, full_capabilities());
        tight.limits.max_tools = 1;
        tight.limits.max_stop_sequences = 1;
        tight.limits.max_output_tokens = 2_048;
        tight.limits.min_output_tokens = 16;

        let mut narrow = anthropic_entry(NARROW, full_capabilities());
        narrow.limits.context_window_tokens = 8;
        narrow.limits.request_body_max_bytes = 64;

        let mut no_sampling = anthropic_entry(NO_SAMPLING, full_capabilities());
        no_sampling.reasoning.excludes_sampling = true;

        vec![
            anthropic_entry(FULL, full_capabilities()),
            minimal,
            plain,
            tools_only,
            tight,
            narrow,
            no_sampling,
        ]
    }

    fn document() -> aex_model_catalog::document::CatalogDocument {
        fixture::document(
            PUBLISHER,
            1,
            entries(),
            fixture::at(NOW_MS),
            fixture::adapter("fixture-adapter"),
        )
    }

    fn catalog() -> &'static Catalog {
        static LOADED: std::sync::OnceLock<Catalog> = std::sync::OnceLock::new();
        LOADED.get_or_init(|| {
            let mut public = [0u8; P256_PUBLIC_KEY_BYTES];
            public.copy_from_slice(&from_hex(PUBLIC_KEY_HEX));
            let compiled: &'static [TrustedKey] = Box::leak(Box::new([(PUBLISHER, public)]));
            let envelope = CatalogEnvelope {
                document: fixture::canonical_bytes(&document()),
                signatures: vec![CatalogSignature {
                    key_id: SigningKeyId(fixture::bounded(PUBLISHER)),
                    algorithm: SigAlg::EcdsaP256Sha256Asn1,
                    bytes: bytes::Bytes::from(from_hex(SIGNATURE_HEX)),
                }],
            };
            Catalog::load(
                &envelope,
                &TrustedKeys::new(compiled),
                fixture::at(NOW_MS),
                None,
                fixture::adapter("fixture-adapter"),
            )
            .expect("the fixture catalog loads; re-sign it if the document shape changed")
        })
    }

    fn qualified(model: &str) -> QualifiedModel {
        catalog()
            .qualified(&ModelSelection {
                credential_id: None,
                model: model.to_owned(),
                provider: ProviderId::Anthropic,
            })
            .expect("the fixture catalog carries the pair")
    }

    // -----------------------------------------------------------------------
    // canonical request helpers
    // -----------------------------------------------------------------------

    fn text_message(role: Role, text: &str) -> CanonicalMessage {
        CanonicalMessage {
            role,
            blocks: vec![CanonicalBlock::Text {
                text: BoundedString::truncating(text),
                annotations: Vec::new(),
            }],
        }
    }

    fn request(model: &str) -> CanonicalModelRequest {
        CanonicalModelRequest {
            selection: qualified(model),
            system: Vec::new(),
            messages: vec![text_message(Role::User, "hello")],
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
            correlation: CorrelationId::from_effect([0u8; 16]),
            request_hash: ContentHash::of(b""),
        }
    }

    fn json(text: &str) -> CanonicalJson {
        CanonicalJson::parse(text).expect("the fixture JSON parses")
    }

    fn tool(name: &str) -> CanonicalToolDef {
        CanonicalToolDef {
            name: ToolName::parse(name).expect("a fixture tool name"),
            description: BoundedString::truncating("Look up the weather."),
            input_schema: json(r#"{"type":"object","properties":{"city":{"type":"string"}}}"#),
            strict: false,
        }
    }

    fn built(request: &CanonicalModelRequest) -> String {
        let wire = AnthropicAdapter
            .build_request(&request.selection, request)
            .expect("the request builds");
        core::str::from_utf8(&wire.body)
            .expect("the body is UTF-8")
            .to_owned()
    }

    fn refused(request: &CanonicalModelRequest) -> RequestBuildError {
        AnthropicAdapter
            .build_request(&request.selection, request)
            .expect_err("the request must be refused before dispatch")
    }

    // -----------------------------------------------------------------------
    // streaming helpers
    // -----------------------------------------------------------------------

    fn event<'a>(name: &'a str, data: &'a str) -> SseEvent<'a> {
        SseEvent {
            name: Some(name),
            data: data.as_bytes(),
            id: None,
        }
    }

    const MESSAGE_START: &str = r#"{"type":"message_start","message":{"id":"msg_1","usage":{"input_tokens":11,"output_tokens":1}}}"#;

    fn fresh_state() -> DialectState {
        AnthropicAdapter.new_state(&qualified(FULL))
    }

    fn feed(
        state: &mut DialectState,
        frames: &[(&str, &str)],
    ) -> Result<Vec<FrameOutcome>, FrameDecodeError> {
        let budget = StreamBudget::default();
        let mut out = Vec::with_capacity(frames.len());
        for (name, data) in frames {
            out.push(AnthropicAdapter.decode(state, &event(name, data), &budget)?);
        }
        Ok(out)
    }

    fn stream(frames: &[(&str, &str)]) -> Result<SealedResponse, FrameDecodeError> {
        let mut state = fresh_state();
        feed(&mut state, frames)?;
        AnthropicAdapter.finish(state)
    }

    fn headers(pairs: &[(&str, &str)]) -> reqwest::header::HeaderMap {
        let mut map = reqwest::header::HeaderMap::new();
        for (name, value) in pairs {
            let name: reqwest::header::HeaderName = (*name).parse().expect("a header name");
            map.insert(name, (*value).parse().expect("a header value"));
        }
        map
    }

    // -----------------------------------------------------------------------
    // identity
    // -----------------------------------------------------------------------

    #[test]
    fn the_adapter_speaks_only_for_anthropic() {
        assert_eq!(AnthropicAdapter.provider(), ProviderId::Anthropic);
    }

    #[test]
    fn the_source_digest_is_deterministic() {
        assert_eq!(
            AnthropicAdapter.source_digest(),
            AnthropicAdapter.source_digest()
        );
    }

    #[test]
    fn the_fixture_entry_declares_the_anthropic_dialect_and_origin() {
        let model = qualified(FULL);
        assert_eq!(model.dialect(), Dialect::AnthropicMessages);
        assert_eq!(model.endpoint(), EndpointPin::AnthropicApi);
        assert_eq!(model.state(), EntryState::Staged);
    }

    // -----------------------------------------------------------------------
    // request goldens
    // -----------------------------------------------------------------------

    #[test]
    fn the_wire_request_pins_the_path_version_header_and_auth_tag() {
        let request = request(FULL);
        let wire = AnthropicAdapter
            .build_request(&request.selection, &request)
            .expect("the request builds");
        assert_eq!(wire.endpoint, EndpointPin::AnthropicApi);
        assert_eq!(wire.path.as_str(), MESSAGES_PATH);
        assert!(wire.query.is_empty(), "this dialect takes no query");
        assert_eq!(
            wire.auth,
            AuthScheme::AnthropicApiKey {
                version: API_VERSION
            }
        );
        assert_eq!(wire.accept, Accept::TextEventStream);
        assert_eq!(
            wire.headers
                .iter()
                .map(|(name, value)| (*name, value.as_str()))
                .collect::<Vec<_>>(),
            vec![(VERSION_HEADER, "2023-06-01")]
        );
        assert!(
            !wire
                .headers
                .iter()
                .any(|(name, _)| *name == "anthropic-beta"),
            "anthropic-beta is never sent (D-17)"
        );
        assert_eq!(
            wire.url().expect("the URL assembles").as_str(),
            "https://api.anthropic.com/v1/messages"
        );
    }

    #[test]
    fn a_text_request_serializes_to_the_documented_body() {
        assert_eq!(
            built(&request(FULL)),
            r#"{"model":"claude-opus-5","max_tokens":1024,"messages":[{"role":"user","content":[{"type":"text","text":"hello"}]}],"stream":true}"#
        );
    }

    #[test]
    fn a_system_sampling_and_stop_sequence_request_serializes_every_optional_member() {
        let mut request = request(FULL);
        request.system = vec![SystemBlock {
            text: BoundedString::truncating("Be brief."),
            cacheable: false,
        }];
        request.temperature_milli = Some(700);
        request.top_p_milli = Some(950);
        request.stop_sequences = vec![BoundedString::truncating("STOP")];
        assert_eq!(
            built(&request),
            r#"{"model":"claude-opus-5","max_tokens":1024,"messages":[{"role":"user","content":[{"type":"text","text":"hello"}]}],"system":[{"type":"text","text":"Be brief."}],"temperature":0.7,"top_p":0.95,"stop_sequences":["STOP"],"stream":true}"#
        );
    }

    #[test]
    fn a_tool_request_serializes_the_input_schema_shape_and_tool_choice() {
        let mut request = request(FULL);
        request.tools = vec![tool("get_weather")];
        assert_eq!(
            built(&request),
            r#"{"model":"claude-opus-5","max_tokens":1024,"messages":[{"role":"user","content":[{"type":"text","text":"hello"}]}],"tools":[{"name":"get_weather","description":"Look up the weather.","input_schema":{"properties":{"city":{"type":"string"}},"type":"object"}}],"tool_choice":{"type":"auto"},"stream":true}"#
        );
    }

    #[test]
    fn forbidding_parallel_calls_reaches_the_wire_rather_than_being_dropped() {
        let mut request = request(FULL);
        request.tools = vec![tool("get_weather")];
        request.parallel_tools = false;
        request.tool_choice = ToolChoice::Named {
            name: ToolName::parse("get_weather").expect("a tool name"),
        };
        assert!(
            built(&request).contains(
                r#""tool_choice":{"type":"tool","name":"get_weather","disable_parallel_tool_use":true}"#
            ),
            "{}",
            built(&request)
        );
    }

    #[test]
    fn a_structured_output_request_serializes_an_output_config_format() {
        let mut request = request(FULL);
        request.structured_output = Some(StructuredOutputRequest::JsonSchema {
            name: aex_wire::ResourceName::parse("answer").expect("a schema name"),
            schema: json(
                r#"{"type":"object","additionalProperties":false,"properties":{"answer":{"type":"string"}}}"#,
            ),
            strict: true,
        });
        assert_eq!(
            built(&request),
            r#"{"model":"claude-opus-5","max_tokens":1024,"messages":[{"role":"user","content":[{"type":"text","text":"hello"}]}],"output_config":{"format":{"type":"json_schema","schema":{"additionalProperties":false,"properties":{"answer":{"type":"string"}},"type":"object"}}},"stream":true}"#
        );
    }

    #[test]
    fn a_reasoning_request_serializes_an_enabled_thinking_budget() {
        let mut request = request(FULL);
        request.max_output_tokens = 4_096;
        request.reasoning = ReasoningRequest::Enabled {
            budget_tokens: Some(2_048),
            effort: None,
        };
        assert_eq!(
            built(&request),
            r#"{"model":"claude-opus-5","max_tokens":4096,"messages":[{"role":"user","content":[{"type":"text","text":"hello"}]}],"thinking":{"type":"enabled","budget_tokens":2048},"stream":true}"#
        );
    }

    #[test]
    fn a_disabled_reasoning_request_says_so_explicitly() {
        let mut request = request(FULL);
        request.reasoning = ReasoningRequest::Disabled;
        assert!(
            built(&request).contains(r#""thinking":{"type":"disabled"}"#),
            "{}",
            built(&request)
        );
    }

    #[test]
    fn a_replayed_reasoning_block_carries_its_signature_back() {
        let mut request = request(FULL);
        request.messages.push(CanonicalMessage {
            role: Role::Assistant,
            blocks: vec![CanonicalBlock::Reasoning(ReasoningBlock {
                body: ReasoningBody::Text {
                    text: BoundedString::truncating("step one"),
                },
                token: Some(ReasoningToken {
                    provenance: ProviderId::Anthropic,
                    bytes: bytes::Bytes::from_static(b"sig-abc"),
                }),
            })],
        });
        assert!(
            built(&request)
                .contains(r#"{"type":"thinking","thinking":"step one","signature":"sig-abc"}"#),
            "{}",
            built(&request)
        );
    }

    #[test]
    fn a_tool_result_replays_as_a_user_role_block() {
        let mut request = request(FULL);
        request.messages.push(CanonicalMessage {
            role: Role::User,
            blocks: vec![CanonicalBlock::ToolResult {
                call: ToolCallId::new("toolu_01").expect("a call id"),
                content: vec![ToolResultPart::Json {
                    value: json(r#"{"c":1}"#),
                }],
                is_error: false,
            }],
        });
        let body = built(&request);
        assert!(
            body.contains(
                r#"{"role":"user","content":[{"type":"tool_result","tool_use_id":"toolu_01","content":[{"type":"text","text":"{\"c\":1}"}],"is_error":false}]}"#
            ),
            "{body}"
        );
    }

    #[test]
    fn cache_breakpoints_place_ephemeral_markers() {
        let mut request = request(FULL);
        request.system = vec![SystemBlock {
            text: BoundedString::truncating("Be brief."),
            cacheable: false,
        }];
        request.tools = vec![tool("get_weather")];
        request.cache_breakpoints = vec![
            CacheBreakpoint::AfterSystem,
            CacheBreakpoint::AfterTools,
            CacheBreakpoint::AfterMessage { index: 0 },
        ];
        let body = built(&request);
        assert_eq!(
            body.matches(r#""cache_control":{"type":"ephemeral"}"#)
                .count(),
            3,
            "{body}"
        );
    }

    // -----------------------------------------------------------------------
    // every RequestBuildError arm this provider can raise
    // -----------------------------------------------------------------------

    #[test]
    fn a_pair_that_does_not_declare_streaming_is_refused() {
        // AEX has no non-streaming path, so this is the first gate every
        // request passes.
        assert_eq!(
            refused(&request(MINIMAL)),
            RequestBuildError::CapabilityUnavailable {
                capability: Capability::Streaming
            }
        );
    }

    #[test]
    fn a_system_instruction_the_pair_does_not_declare_is_refused() {
        let mut request = request(PLAIN);
        request.system = vec![SystemBlock {
            text: BoundedString::truncating("Be brief."),
            cacheable: false,
        }];
        assert_eq!(
            refused(&request),
            RequestBuildError::CapabilityUnavailable {
                capability: Capability::SystemInstruction
            }
        );
    }

    #[test]
    fn tools_the_pair_does_not_declare_are_refused() {
        let mut request = request(PLAIN);
        request.tools = vec![tool("get_weather")];
        assert_eq!(
            refused(&request),
            RequestBuildError::CapabilityUnavailable {
                capability: Capability::Tools
            }
        );
    }

    #[test]
    fn structured_output_the_pair_does_not_declare_is_refused() {
        let mut request = request(PLAIN);
        request.structured_output = Some(StructuredOutputRequest::JsonObject);
        assert_eq!(
            refused(&request),
            RequestBuildError::CapabilityUnavailable {
                capability: Capability::StructuredOutput
            }
        );
    }

    #[test]
    fn stop_sequences_the_pair_does_not_declare_are_refused() {
        let mut request = request(PLAIN);
        request.stop_sequences = vec![BoundedString::truncating("STOP")];
        assert_eq!(
            refused(&request),
            RequestBuildError::CapabilityUnavailable {
                capability: Capability::StopSequences
            }
        );
    }

    #[test]
    fn reasoning_the_pair_does_not_declare_is_refused() {
        let mut request = request(PLAIN);
        request.max_output_tokens = 4_096;
        request.reasoning = ReasoningRequest::Enabled {
            budget_tokens: Some(2_048),
            effort: None,
        };
        assert_eq!(
            refused(&request),
            RequestBuildError::CapabilityUnavailable {
                capability: Capability::Reasoning
            }
        );
    }

    #[test]
    fn parallel_tool_calls_the_pair_does_not_declare_are_refused() {
        let mut request = request(TOOLS_ONLY);
        request.tools = vec![tool("get_weather")];
        request.parallel_tools = true;
        assert_eq!(
            refused(&request),
            RequestBuildError::CapabilityUnavailable {
                capability: Capability::ParallelTools
            }
        );
    }

    #[test]
    fn a_strict_tool_schema_the_pair_does_not_declare_is_refused() {
        let mut request = request(TOOLS_ONLY);
        let mut strict = tool("get_weather");
        strict.strict = true;
        request.tools = vec![strict];
        assert_eq!(
            refused(&request),
            RequestBuildError::CapabilityUnavailable {
                capability: Capability::StrictToolSchema
            }
        );
    }

    #[test]
    fn a_strict_tool_schema_the_pair_does_declare_reaches_the_wire() {
        let mut request = request(FULL);
        let mut strict = tool("get_weather");
        strict.strict = true;
        request.tools = vec![strict];
        assert!(
            built(&request).contains(r#""strict":true"#),
            "{}",
            built(&request)
        );
    }

    #[test]
    fn a_tool_choice_mode_the_pair_does_not_declare_is_refused() {
        let mut request = request(TOOLS_ONLY);
        request.tools = vec![tool("get_weather")];
        request.parallel_tools = false;
        request.tool_choice = ToolChoice::Required;
        assert_eq!(
            refused(&request),
            RequestBuildError::ToolChoiceUnsupported {
                requested: Box::new(ToolChoice::Required)
            }
        );
    }

    #[test]
    fn replaying_reasoning_without_the_replay_capability_is_refused() {
        let mut request = request(TOOLS_ONLY);
        request.messages.push(CanonicalMessage {
            role: Role::Assistant,
            blocks: vec![CanonicalBlock::Reasoning(ReasoningBlock {
                body: ReasoningBody::Text {
                    text: BoundedString::truncating("signed but unreplayable"),
                },
                token: Some(ReasoningToken {
                    provenance: ProviderId::Anthropic,
                    bytes: bytes::Bytes::from_static(b"sig"),
                }),
            })],
        });
        assert_eq!(
            refused(&request),
            RequestBuildError::CapabilityUnavailable {
                capability: Capability::ReasoningReplay
            }
        );
    }

    #[test]
    fn tools_beyond_the_declared_bound_are_refused() {
        let mut request = request(TIGHT);
        request.tools = vec![tool("one"), tool("two")];
        assert_eq!(refused(&request), RequestBuildError::ToolLimit { max: 1 });
    }

    #[test]
    fn a_tool_name_outside_the_provider_grammar_is_refused() {
        let mut request = request(FULL);
        let mut broken = tool("get_weather");
        broken.name = ToolName::parse("get.weather").expect("the workspace grammar allows a dot");
        request.tools = vec![broken.clone()];
        assert_eq!(
            refused(&request),
            RequestBuildError::ToolNameInvalid { name: broken.name }
        );
    }

    #[test]
    fn a_required_tool_choice_with_no_tools_is_refused() {
        let mut request = request(FULL);
        request.tool_choice = ToolChoice::Required;
        assert_eq!(
            refused(&request),
            RequestBuildError::Encoding {
                reason: "a required or named tool choice needs at least one tool"
            }
        );
    }

    #[test]
    fn a_json_object_structured_output_is_refused_because_the_format_needs_a_schema() {
        let mut request = request(FULL);
        request.structured_output = Some(StructuredOutputRequest::JsonObject);
        assert_eq!(
            refused(&request),
            RequestBuildError::StructuredOutputUnsupported {
                requested: Box::new(StructuredOutputRequest::JsonObject)
            }
        );
    }

    #[test]
    fn a_non_strict_schema_is_refused_because_the_format_has_no_strictness_switch() {
        let mut request = request(FULL);
        let requested = StructuredOutputRequest::JsonSchema {
            name: aex_wire::ResourceName::parse("answer").expect("a schema name"),
            schema: json(r#"{"type":"object"}"#),
            strict: false,
        };
        request.structured_output = Some(requested.clone());
        assert_eq!(
            refused(&request),
            RequestBuildError::StructuredOutputUnsupported {
                requested: Box::new(requested)
            }
        );
    }

    #[test]
    fn more_stop_sequences_than_the_pair_accepts_are_refused() {
        let mut request = request(TIGHT);
        request.stop_sequences = vec![
            BoundedString::truncating("A"),
            BoundedString::truncating("B"),
        ];
        assert_eq!(
            refused(&request),
            RequestBuildError::StopSequenceLimit { max: 1 }
        );
    }

    #[test]
    fn a_sampling_field_outside_the_pair_range_is_refused() {
        // Anthropic's temperature is 0–1, so 1.5 is out of range rather than
        // silently clamped.
        let mut request = request(FULL);
        request.temperature_milli = Some(1_500);
        assert_eq!(
            refused(&request),
            RequestBuildError::SamplingUnsupported {
                field: "temperature"
            }
        );
        request.temperature_milli = None;
        request.top_p_milli = Some(0);
        assert_eq!(
            refused(&request),
            RequestBuildError::SamplingUnsupported { field: "top_p" }
        );
    }

    #[test]
    fn a_sampling_field_the_pair_does_not_declare_is_refused() {
        let mut request = request(PLAIN);
        request.temperature_milli = Some(500);
        assert_eq!(
            refused(&request),
            RequestBuildError::SamplingUnsupported {
                field: "temperature"
            }
        );
        request.temperature_milli = None;
        request.top_p_milli = Some(500);
        assert_eq!(
            refused(&request),
            RequestBuildError::SamplingUnsupported { field: "top_p" }
        );
    }

    #[test]
    fn reasoning_forbids_sampling_on_a_pair_that_says_so() {
        let mut request = request(NO_SAMPLING);
        request.max_output_tokens = 4_096;
        request.reasoning = ReasoningRequest::Enabled {
            budget_tokens: Some(2_048),
            effort: None,
        };
        request.temperature_milli = Some(500);
        assert_eq!(
            refused(&request),
            RequestBuildError::SamplingWithReasoning {
                field: "temperature"
            }
        );
    }

    #[test]
    fn an_output_ceiling_outside_the_pair_range_is_refused() {
        let mut request = request(TIGHT);
        request.max_output_tokens = 4;
        assert_eq!(
            refused(&request),
            RequestBuildError::OutputTokensOutOfRange {
                min: 16,
                max: 2_048
            }
        );
    }

    #[test]
    fn a_reasoning_budget_below_the_documented_floor_is_refused() {
        let mut request = request(FULL);
        request.max_output_tokens = 4_096;
        request.reasoning = ReasoningRequest::Enabled {
            budget_tokens: Some(512),
            effort: None,
        };
        assert_eq!(
            refused(&request),
            RequestBuildError::ReasoningBudgetOutOfRange {
                min: 1_024,
                max: 4_095
            }
        );
    }

    #[test]
    fn a_reasoning_budget_that_is_not_below_max_tokens_is_refused() {
        let mut request = request(FULL);
        request.max_output_tokens = 2_048;
        request.reasoning = ReasoningRequest::Enabled {
            budget_tokens: Some(2_048),
            effort: None,
        };
        assert_eq!(
            refused(&request),
            RequestBuildError::ReasoningBudgetOutOfRange {
                min: 1_024,
                max: 2_047
            }
        );
    }

    #[test]
    fn a_reasoning_effort_level_is_refused_because_this_dialect_takes_a_budget() {
        let mut request = request(FULL);
        request.max_output_tokens = 4_096;
        request.reasoning = ReasoningRequest::Enabled {
            budget_tokens: Some(2_048),
            effort: Some(ReasoningEffort::High),
        };
        assert_eq!(
            refused(&request),
            RequestBuildError::Encoding {
                reason: "anthropic takes a thinking budget, never an effort level"
            }
        );
    }

    #[test]
    fn reasoning_material_from_another_provider_is_refused() {
        let mut request = request(FULL);
        request.messages.push(CanonicalMessage {
            role: Role::Assistant,
            blocks: vec![CanonicalBlock::Reasoning(ReasoningBlock {
                body: ReasoningBody::Text {
                    text: BoundedString::truncating("borrowed"),
                },
                token: Some(ReasoningToken {
                    provenance: ProviderId::Openai,
                    bytes: bytes::Bytes::from_static(b"not-mine"),
                }),
            })],
        });
        assert_eq!(
            refused(&request),
            RequestBuildError::ReasoningProvenanceMismatch {
                expected: ProviderId::Anthropic,
                found: ProviderId::Openai,
            }
        );
    }

    #[test]
    fn a_reasoning_block_without_round_trip_material_is_refused() {
        let mut request = request(FULL);
        request.messages.push(CanonicalMessage {
            role: Role::Assistant,
            blocks: vec![CanonicalBlock::Reasoning(ReasoningBlock {
                body: ReasoningBody::Text {
                    text: BoundedString::truncating("unsigned"),
                },
                token: None,
            })],
        });
        assert_eq!(refused(&request), RequestBuildError::ReasoningTokenRequired);
    }

    #[test]
    fn a_prompt_larger_than_the_declared_window_is_refused_before_dispatch() {
        let mut request = request(NARROW);
        request.messages = vec![text_message(Role::User, &"x".repeat(400))];
        assert_eq!(
            refused(&request),
            RequestBuildError::ContextOverflowEstimated {
                limit: 8,
                estimated: 100
            }
        );
    }

    #[test]
    fn a_body_over_the_pair_bound_is_refused_before_dispatch() {
        // Small enough to pass the context estimate, large enough to cross the
        // 64-byte body bound.
        assert_eq!(
            refused(&request(NARROW)),
            RequestBuildError::BodyTooLarge { limit: 64 }
        );
    }

    #[test]
    fn cache_breakpoints_the_pair_does_not_declare_are_refused() {
        let mut request = request(PLAIN);
        request.cache_breakpoints = vec![CacheBreakpoint::AfterMessage { index: 0 }];
        assert_eq!(
            refused(&request),
            RequestBuildError::CapabilityUnavailable {
                capability: Capability::PromptCacheExplicit
            }
        );
    }

    #[test]
    fn a_cache_breakpoint_naming_a_message_that_does_not_exist_is_refused() {
        let mut request = request(FULL);
        request.cache_breakpoints = vec![CacheBreakpoint::AfterMessage { index: 9 }];
        assert_eq!(
            refused(&request),
            RequestBuildError::Encoding {
                reason: "a cache breakpoint names a message that does not exist"
            }
        );
    }

    #[test]
    fn more_cache_breakpoints_than_the_pair_accepts_are_refused() {
        let mut request = request(FULL);
        request.messages = vec![
            text_message(Role::User, "a"),
            text_message(Role::Assistant, "b"),
            text_message(Role::User, "c"),
            text_message(Role::Assistant, "d"),
            text_message(Role::User, "e"),
        ];
        request.cache_breakpoints = (0..5)
            .map(|index| CacheBreakpoint::AfterMessage { index })
            .collect();
        assert_eq!(
            refused(&request),
            RequestBuildError::Encoding {
                reason: "the request places more cache breakpoints than the pair accepts"
            }
        );
    }

    // -----------------------------------------------------------------------
    // the frame taxonomy
    // -----------------------------------------------------------------------

    #[test]
    fn a_whole_text_stream_seals_one_text_block() {
        let sealed = stream(&[
            ("message_start", MESSAGE_START),
            (
                "content_block_start",
                r#"{"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}"#,
            ),
            (
                "content_block_delta",
                r#"{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"Hel"}}"#,
            ),
            (
                "content_block_delta",
                r#"{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"lo."}}"#,
            ),
            (
                "content_block_stop",
                r#"{"type":"content_block_stop","index":0}"#,
            ),
            (
                "message_delta",
                r#"{"type":"message_delta","delta":{"stop_reason":"end_turn","stop_sequence":null},"usage":{"output_tokens":7}}"#,
            ),
            ("message_stop", r#"{"type":"message_stop"}"#),
        ])
        .expect("the stream seals");
        assert_eq!(sealed.stop_reason, StopReason::EndTurn);
        assert_eq!(
            sealed.blocks,
            vec![CanonicalBlock::Text {
                text: BoundedString::truncating("Hello."),
                annotations: Vec::new(),
            }]
        );
        assert_eq!(sealed.usage.input_tokens, 11);
        assert_eq!(sealed.usage.output_tokens, 7);
        assert_eq!(sealed.usage.completeness, UsageCompleteness::Exact);
    }

    #[test]
    fn only_message_start_proves_the_response_started() {
        let mut state = fresh_state();
        let budget = StreamBudget::default();
        assert_eq!(
            AnthropicAdapter
                .decode(&mut state, &event("ping", r#"{"type":"ping"}"#), &budget)
                .expect("a ping decodes"),
            FrameOutcome::Ignored
        );
        assert!(
            !state.response_started,
            "a keep-alive is not proof the provider is generating"
        );
        assert_eq!(
            AnthropicAdapter
                .decode(&mut state, &event("message_start", MESSAGE_START), &budget)
                .expect("message_start decodes"),
            FrameOutcome::ResponseStarted
        );
    }

    #[test]
    fn a_ping_may_appear_anywhere_in_the_stream() {
        let sealed = stream(&[
            ("ping", r#"{"type":"ping"}"#),
            ("message_start", MESSAGE_START),
            ("ping", r#"{"type":"ping"}"#),
            (
                "content_block_start",
                r#"{"type":"content_block_start","index":0,"content_block":{"type":"text","text":"hi"}}"#,
            ),
            ("ping", r#"{"type":"ping"}"#),
            (
                "content_block_stop",
                r#"{"type":"content_block_stop","index":0}"#,
            ),
            (
                "message_delta",
                r#"{"type":"message_delta","delta":{"stop_reason":"end_turn"}}"#,
            ),
            ("message_stop", r#"{"type":"message_stop"}"#),
        ])
        .expect("pings do not disturb the state machine");
        assert_eq!(sealed.blocks.len(), 1);
    }

    #[test]
    fn an_unknown_event_type_is_a_protocol_violation() {
        let mut state = fresh_state();
        feed(&mut state, &[("message_start", MESSAGE_START)]).expect("start");
        let error = feed(
            &mut state,
            &[(
                "web_search_tool_result",
                r#"{"type":"web_search_tool_result"}"#,
            )],
        )
        .expect_err("a provider-hosted tool event is never sent to us");
        assert_eq!(
            error,
            FrameDecodeError::UnknownEvent {
                event: BoundedString::truncating("web_search_tool_result")
            }
        );
    }

    #[test]
    fn an_unknown_content_block_type_is_a_protocol_violation() {
        let mut state = fresh_state();
        feed(&mut state, &[("message_start", MESSAGE_START)]).expect("start");
        let error = feed(
            &mut state,
            &[(
                "content_block_start",
                r#"{"type":"content_block_start","index":0,"content_block":{"type":"server_tool_use"}}"#,
            )],
        )
        .expect_err("a server tool block is never enabled");
        assert_eq!(
            error,
            FrameDecodeError::UnknownEvent {
                event: BoundedString::truncating("server_tool_use")
            }
        );
    }

    #[test]
    fn an_unknown_delta_type_is_a_protocol_violation() {
        let mut state = fresh_state();
        feed(
            &mut state,
            &[
                ("message_start", MESSAGE_START),
                (
                    "content_block_start",
                    r#"{"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}"#,
                ),
            ],
        )
        .expect("start");
        let error = feed(
            &mut state,
            &[(
                "content_block_delta",
                r#"{"type":"content_block_delta","index":0,"delta":{"type":"citations_delta"}}"#,
            )],
        )
        .expect_err("an undocumented delta is refused");
        assert_eq!(
            error,
            FrameDecodeError::UnknownEvent {
                event: BoundedString::truncating("citations_delta")
            }
        );
    }

    #[test]
    fn an_event_name_that_disagrees_with_the_payload_type_is_refused() {
        let mut state = fresh_state();
        let error = feed(&mut state, &[("message_stop", MESSAGE_START)])
            .expect_err("the name is documented as mirrored in the payload");
        assert_eq!(error, FrameDecodeError::MalformedField { field: "type" });
    }

    #[test]
    fn a_payload_that_is_not_json_is_refused() {
        let mut state = fresh_state();
        let budget = StreamBudget::default();
        let error = AnthropicAdapter
            .decode(&mut state, &event("message_start", "not json"), &budget)
            .expect_err("a non-JSON payload is refused");
        assert_eq!(error, FrameDecodeError::NotJson);
    }

    #[test]
    fn there_is_no_done_sentinel_in_this_dialect() {
        let mut state = fresh_state();
        let budget = StreamBudget::default();
        let sentinel = SseEvent {
            name: None,
            data: b"[DONE]",
            id: None,
        };
        assert_eq!(
            AnthropicAdapter
                .decode(&mut state, &sentinel, &budget)
                .expect_err("this dialect sends no sentinel"),
            FrameDecodeError::UnknownEvent {
                event: BoundedString::truncating("[DONE]")
            }
        );
    }

    #[test]
    fn a_content_frame_before_message_start_is_out_of_order() {
        let mut state = fresh_state();
        let error = feed(
            &mut state,
            &[(
                "content_block_start",
                r#"{"type":"content_block_start","index":0,"content_block":{"type":"text"}}"#,
            )],
        )
        .expect_err("nothing precedes message_start");
        assert_eq!(
            error,
            FrameDecodeError::OutOfOrder {
                reason: "a dialect frame arrived before message_start"
            }
        );
    }

    #[test]
    fn a_second_message_start_is_out_of_order() {
        let mut state = fresh_state();
        let error = feed(
            &mut state,
            &[
                ("message_start", MESSAGE_START),
                ("message_start", MESSAGE_START),
            ],
        )
        .expect_err("one stream carries one message");
        assert_eq!(
            error,
            FrameDecodeError::OutOfOrder {
                reason: "message_start arrived twice in one stream"
            }
        );
    }

    #[test]
    fn a_frame_after_message_stop_is_out_of_order() {
        let mut state = fresh_state();
        let error = feed(
            &mut state,
            &[
                ("message_start", MESSAGE_START),
                (
                    "message_delta",
                    r#"{"type":"message_delta","delta":{"stop_reason":"end_turn"}}"#,
                ),
                ("message_stop", r#"{"type":"message_stop"}"#),
                ("ping", r#"{"type":"ping"}"#),
            ],
        )
        .expect_err("message_stop is terminal");
        assert_eq!(
            error,
            FrameDecodeError::OutOfOrder {
                reason: "a frame arrived after message_stop"
            }
        );
    }

    #[test]
    fn content_block_stop_for_an_unopened_index_is_out_of_order() {
        let mut state = fresh_state();
        let error = feed(
            &mut state,
            &[
                ("message_start", MESSAGE_START),
                (
                    "content_block_stop",
                    r#"{"type":"content_block_stop","index":3}"#,
                ),
            ],
        )
        .expect_err("nothing is open at index 3");
        assert_eq!(
            error,
            FrameDecodeError::OutOfOrder {
                reason: "content_block_stop names a block that is not open"
            }
        );
    }

    #[test]
    fn a_stream_that_ends_without_message_stop_is_refused() {
        let error = stream(&[
            ("message_start", MESSAGE_START),
            (
                "message_delta",
                r#"{"type":"message_delta","delta":{"stop_reason":"end_turn"}}"#,
            ),
        ])
        .expect_err("a cut stream is never a shorter result");
        assert_eq!(
            error,
            FrameDecodeError::OutOfOrder {
                reason: "the stream ended without message_stop"
            }
        );
    }

    #[test]
    fn a_stream_that_ends_with_an_open_block_is_refused() {
        let error = stream(&[
            ("message_start", MESSAGE_START),
            (
                "content_block_start",
                r#"{"type":"content_block_start","index":0,"content_block":{"type":"text","text":"half"}}"#,
            ),
            (
                "message_delta",
                r#"{"type":"message_delta","delta":{"stop_reason":"end_turn"}}"#,
            ),
            ("message_stop", r#"{"type":"message_stop"}"#),
        ])
        .expect_err("an unbalanced block set never seals");
        assert_eq!(
            error,
            FrameDecodeError::OutOfOrder {
                reason: "the stream ended with an unbalanced content block"
            }
        );
    }

    #[test]
    fn a_terminal_without_a_stop_reason_is_refused() {
        let error = stream(&[
            ("message_start", MESSAGE_START),
            ("message_stop", r#"{"type":"message_stop"}"#),
        ])
        .expect_err("a turn without a stop reason cannot seal");
        assert_eq!(
            error,
            FrameDecodeError::MalformedField {
                field: "delta.stop_reason"
            }
        );
    }

    // -----------------------------------------------------------------------
    // tool-call reassembly
    // -----------------------------------------------------------------------

    fn tool_stream(fragments: &[&str]) -> Result<SealedResponse, FrameDecodeError> {
        let mut frames: Vec<(String, String)> = vec![
            ("message_start".to_owned(), MESSAGE_START.to_owned()),
            (
                "content_block_start".to_owned(),
                r#"{"type":"content_block_start","index":0,"content_block":{"type":"tool_use","id":"toolu_01","name":"get_weather","input":{}}}"#.to_owned(),
            ),
        ];
        for fragment in fragments {
            let escaped = serde_json::to_string(fragment).expect("a JSON string");
            frames.push((
                "content_block_delta".to_owned(),
                format!(
                    r#"{{"type":"content_block_delta","index":0,"delta":{{"type":"input_json_delta","partial_json":{escaped}}}}}"#
                ),
            ));
        }
        frames.push((
            "content_block_stop".to_owned(),
            r#"{"type":"content_block_stop","index":0}"#.to_owned(),
        ));
        frames.push((
            "message_delta".to_owned(),
            r#"{"type":"message_delta","delta":{"stop_reason":"tool_use"},"usage":{"output_tokens":9}}"#.to_owned(),
        ));
        frames.push((
            "message_stop".to_owned(),
            r#"{"type":"message_stop"}"#.to_owned(),
        ));
        let borrowed: Vec<(&str, &str)> = frames
            .iter()
            .map(|(name, data)| (name.as_str(), data.as_str()))
            .collect();
        stream(&borrowed)
    }

    #[test]
    fn tool_arguments_reassemble_from_partial_json_fragments() {
        let sealed = tool_stream(&[r#"{"ci"#, r#"ty":"Ly"#, r#"on"}"#]).expect("the stream seals");
        assert_eq!(sealed.stop_reason, StopReason::ToolUse);
        assert_eq!(
            sealed.blocks,
            vec![CanonicalBlock::ToolUse {
                id: ToolCallId::new("toolu_01").expect("a call id"),
                name: ToolName::parse("get_weather").expect("a tool name"),
                input: json(r#"{"city":"Lyon"}"#),
            }]
        );
    }

    #[test]
    fn a_fragment_boundary_inside_a_string_escape_reassembles() {
        // The split lands between the backslash and the `n` of `\n`, and again
        // between the `\u` and its four hex digits: byte-wise concatenation is
        // the only reassembly that survives either.
        let sealed = tool_stream(&[
            r#"{"note":"line\"#,
            r"none\u",
            r#"0041","city":"Ly"#,
            r#"on"}"#,
        ])
        .expect("the stream seals");
        let CanonicalBlock::ToolUse { input, .. } = &sealed.blocks[0] else {
            panic!("expected a tool call");
        };
        assert_eq!(input, &json(r#"{"city":"Lyon","note":"line\noneA"}"#));
    }

    #[test]
    fn tool_arguments_that_do_not_reassemble_into_json_are_refused() {
        let error = tool_stream(&[r#"{"city":"#]).expect_err("a truncated object is not JSON");
        assert_eq!(
            error,
            FrameDecodeError::ToolArgumentsNotJson {
                call: ToolCallId::new("toolu_01").expect("a call id")
            }
        );
    }

    #[test]
    fn a_tool_call_with_no_argument_fragments_is_an_empty_object() {
        let sealed = tool_stream(&[]).expect("the stream seals");
        let CanonicalBlock::ToolUse { input, .. } = &sealed.blocks[0] else {
            panic!("expected a tool call");
        };
        assert_eq!(input, &json("{}"));
    }

    // -----------------------------------------------------------------------
    // reasoning
    // -----------------------------------------------------------------------

    #[test]
    fn a_thinking_block_captures_its_signature_as_round_trip_material() {
        let sealed = stream(&[
            ("message_start", MESSAGE_START),
            (
                "content_block_start",
                r#"{"type":"content_block_start","index":0,"content_block":{"type":"thinking","thinking":""}}"#,
            ),
            (
                "content_block_delta",
                r#"{"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":"weigh it"}}"#,
            ),
            (
                "content_block_delta",
                r#"{"type":"content_block_delta","index":0,"delta":{"type":"signature_delta","signature":"EqQBCgIYAh"}}"#,
            ),
            (
                "content_block_stop",
                r#"{"type":"content_block_stop","index":0}"#,
            ),
            (
                "message_delta",
                r#"{"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":12,"output_tokens_details":{"thinking_tokens":9}}}"#,
            ),
            ("message_stop", r#"{"type":"message_stop"}"#),
        ])
        .expect("the stream seals");
        assert_eq!(
            sealed.blocks,
            vec![CanonicalBlock::Reasoning(ReasoningBlock {
                body: ReasoningBody::Text {
                    text: BoundedString::truncating("weigh it")
                },
                token: Some(ReasoningToken {
                    provenance: ProviderId::Anthropic,
                    bytes: bytes::Bytes::from_static(b"EqQBCgIYAh"),
                }),
            })]
        );
        assert_eq!(sealed.usage.reasoning_tokens, 9);
        assert_eq!(sealed.usage.output_tokens, 12);
        assert!(
            sealed.usage.is_consistent(),
            "reasoning is a subset of output"
        );
    }

    #[test]
    fn a_thinking_block_that_never_signed_is_refused() {
        let error = stream(&[
            ("message_start", MESSAGE_START),
            (
                "content_block_start",
                r#"{"type":"content_block_start","index":0,"content_block":{"type":"thinking","thinking":"x"}}"#,
            ),
            (
                "content_block_stop",
                r#"{"type":"content_block_stop","index":0}"#,
            ),
        ])
        .expect_err("the signature is mandatory round-trip material");
        assert_eq!(
            error,
            FrameDecodeError::MalformedField {
                field: "delta.signature"
            }
        );
    }

    #[test]
    fn a_second_signature_delta_is_out_of_order() {
        let mut state = fresh_state();
        let error = feed(
            &mut state,
            &[
                ("message_start", MESSAGE_START),
                (
                    "content_block_start",
                    r#"{"type":"content_block_start","index":0,"content_block":{"type":"thinking","thinking":"x"}}"#,
                ),
                (
                    "content_block_delta",
                    r#"{"type":"content_block_delta","index":0,"delta":{"type":"signature_delta","signature":"one"}}"#,
                ),
                (
                    "content_block_delta",
                    r#"{"type":"content_block_delta","index":0,"delta":{"type":"signature_delta","signature":"two"}}"#,
                ),
            ],
        )
        .expect_err("exactly one signature delta closes a thinking block");
        assert_eq!(
            error,
            FrameDecodeError::OutOfOrder {
                reason: "a thinking block carries exactly one signature delta"
            }
        );
    }

    #[test]
    fn a_redacted_thinking_block_carries_only_its_token() {
        let sealed = stream(&[
            ("message_start", MESSAGE_START),
            (
                "content_block_start",
                r#"{"type":"content_block_start","index":0,"content_block":{"type":"redacted_thinking","data":"EroBCkY"}}"#,
            ),
            (
                "content_block_stop",
                r#"{"type":"content_block_stop","index":0}"#,
            ),
            (
                "message_delta",
                r#"{"type":"message_delta","delta":{"stop_reason":"end_turn"}}"#,
            ),
            ("message_stop", r#"{"type":"message_stop"}"#),
        ])
        .expect("the stream seals");
        assert_eq!(
            sealed.blocks,
            vec![CanonicalBlock::Reasoning(ReasoningBlock {
                body: ReasoningBody::Redacted,
                token: Some(ReasoningToken {
                    provenance: ProviderId::Anthropic,
                    bytes: bytes::Bytes::from_static(b"EroBCkY"),
                }),
            })]
        );
    }

    // -----------------------------------------------------------------------
    // the cumulative-usage trap
    // -----------------------------------------------------------------------

    #[test]
    fn message_delta_usage_overwrites_rather_than_sums() {
        // Anthropic reports a running total on every `message_delta`. Summing
        // 10 and 25 would bill 35 for a turn that generated 25.
        let sealed = stream(&[
            ("message_start", MESSAGE_START),
            (
                "message_delta",
                r#"{"type":"message_delta","delta":{"stop_reason":null},"usage":{"output_tokens":10}}"#,
            ),
            (
                "message_delta",
                r#"{"type":"message_delta","delta":{"stop_reason":null},"usage":{"output_tokens":25}}"#,
            ),
            (
                "message_delta",
                r#"{"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":25}}"#,
            ),
            ("message_stop", r#"{"type":"message_stop"}"#),
        ])
        .expect("the stream seals");
        assert_eq!(sealed.usage.output_tokens, 25, "the tally is overwritten");
        assert_eq!(
            sealed.usage.input_tokens, 11,
            "a delta that omits input_tokens must not erase the prompt count"
        );
    }

    #[test]
    fn every_usage_field_the_dialect_publishes_is_normalized() {
        let sealed = stream(&[
            (
                "message_start",
                r#"{"type":"message_start","message":{"id":"msg_2","usage":{"input_tokens":5,"cache_creation_input_tokens":40,"cache_read_input_tokens":900,"output_tokens":0}}}"#,
            ),
            (
                "message_delta",
                r#"{"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":33,"output_tokens_details":{"thinking_tokens":21}}}"#,
            ),
            ("message_stop", r#"{"type":"message_stop"}"#),
        ])
        .expect("the stream seals");
        assert_eq!(
            sealed.usage,
            NormalizedUsage {
                input_tokens: 5,
                cache_read_input_tokens: 900,
                cache_write_input_tokens: 40,
                output_tokens: 33,
                reasoning_tokens: 21,
                tool_use_prompt_tokens: 0,
                provider_total_tokens: None,
                completeness: UsageCompleteness::Exact,
            }
        );
    }

    #[test]
    fn a_stream_that_reports_no_usage_records_the_absence() {
        let sealed = stream(&[
            (
                "message_start",
                r#"{"type":"message_start","message":{"id":"msg_3"}}"#,
            ),
            (
                "message_delta",
                r#"{"type":"message_delta","delta":{"stop_reason":"end_turn"}}"#,
            ),
            ("message_stop", r#"{"type":"message_stop"}"#),
        ])
        .expect("the stream seals");
        assert_eq!(sealed.usage.completeness, UsageCompleteness::Absent);
    }

    #[test]
    fn reasoning_tokens_larger_than_output_tokens_are_refused() {
        let mut state = fresh_state();
        let error = feed(
            &mut state,
            &[
                ("message_start", MESSAGE_START),
                (
                    "message_delta",
                    r#"{"type":"message_delta","delta":{"stop_reason":null},"usage":{"output_tokens":3,"output_tokens_details":{"thinking_tokens":9}}}"#,
                ),
            ],
        )
        .expect_err("reasoning is a subset of output, never larger");
        assert_eq!(error, FrameDecodeError::MalformedField { field: "usage" });
    }

    // -----------------------------------------------------------------------
    // the stop table
    // -----------------------------------------------------------------------

    #[test]
    fn every_documented_stop_token_resolves_exactly_as_the_reference_records() {
        let table: [(&str, StopResolution); 7] = [
            ("end_turn", StopResolution::Stop(StopReason::EndTurn)),
            ("tool_use", StopResolution::Stop(StopReason::ToolUse)),
            (
                "max_tokens",
                StopResolution::Stop(StopReason::MaxOutputTokens),
            ),
            (
                "stop_sequence",
                StopResolution::Stop(StopReason::StopSequence),
            ),
            ("refusal", StopResolution::Stop(StopReason::Refusal)),
            (
                "model_context_window_exceeded",
                StopResolution::Failure(ProviderFailureKind::ContextOverflow),
            ),
            (
                "pause_turn",
                StopResolution::Failure(ProviderFailureKind::ProtocolViolation),
            ),
        ];
        for (token, expected) in table {
            assert_eq!(resolve_stop(token), expected, "{token}");
        }
        assert_eq!(resolve_stop("stop"), StopResolution::Unknown);
        assert_eq!(resolve_stop(""), StopResolution::Unknown);
    }

    #[test]
    fn the_fixture_entry_and_the_compiled_table_agree() {
        // The catalog's own map is data; this module's table is code. They must
        // not drift, or a receipt would be earned against a different meaning.
        let model = qualified(FULL);
        let map = &model.entry().stop_reason_map;
        for token in map.tokens() {
            assert_eq!(map.resolve(token), resolve_stop(token), "{token}");
        }
    }

    #[test]
    fn a_context_overflow_stop_token_is_a_failure_not_a_stop_reason() {
        let mut state = fresh_state();
        let outcomes = feed(
            &mut state,
            &[
                ("message_start", MESSAGE_START),
                (
                    "message_delta",
                    r#"{"type":"message_delta","delta":{"stop_reason":"model_context_window_exceeded"}}"#,
                ),
            ],
        )
        .expect("the frame decodes");
        let FrameOutcome::Failed(failure) = &outcomes[1] else {
            panic!("expected a failure outcome, got {:?}", outcomes[1]);
        };
        assert_eq!(failure.kind(), ProviderFailureKind::ContextOverflow);
    }

    #[test]
    fn a_paused_turn_is_a_protocol_violation_because_no_server_tool_was_enabled() {
        let mut state = fresh_state();
        let outcomes = feed(
            &mut state,
            &[
                ("message_start", MESSAGE_START),
                (
                    "message_delta",
                    r#"{"type":"message_delta","delta":{"stop_reason":"pause_turn"}}"#,
                ),
            ],
        )
        .expect("the frame decodes");
        let FrameOutcome::Failed(failure) = &outcomes[1] else {
            panic!("expected a failure outcome");
        };
        assert_eq!(failure.kind(), ProviderFailureKind::ProtocolViolation);
    }

    #[test]
    fn an_unknown_stop_token_is_refused() {
        let mut state = fresh_state();
        let error = feed(
            &mut state,
            &[
                ("message_start", MESSAGE_START),
                (
                    "message_delta",
                    r#"{"type":"message_delta","delta":{"stop_reason":"finished"}}"#,
                ),
            ],
        )
        .expect_err("an undocumented token is a protocol violation");
        assert_eq!(
            error,
            FrameDecodeError::MalformedField {
                field: "delta.stop_reason"
            }
        );
    }

    #[test]
    fn a_refusal_with_no_content_at_all_is_a_content_filter_failure() {
        let mut state = fresh_state();
        let outcomes = feed(
            &mut state,
            &[
                ("message_start", MESSAGE_START),
                (
                    "message_delta",
                    r#"{"type":"message_delta","delta":{"stop_reason":"refusal"}}"#,
                ),
            ],
        )
        .expect("the frame decodes");
        let FrameOutcome::Failed(failure) = &outcomes[1] else {
            panic!("expected a failure outcome");
        };
        assert_eq!(failure.kind(), ProviderFailureKind::ContentFiltered);
    }

    #[test]
    fn a_refusal_that_produced_content_is_a_stop_reason() {
        let sealed = stream(&[
            ("message_start", MESSAGE_START),
            (
                "content_block_start",
                r#"{"type":"content_block_start","index":0,"content_block":{"type":"text","text":"I cannot help with that."}}"#,
            ),
            (
                "content_block_stop",
                r#"{"type":"content_block_stop","index":0}"#,
            ),
            (
                "message_delta",
                r#"{"type":"message_delta","delta":{"stop_reason":"refusal"}}"#,
            ),
            ("message_stop", r#"{"type":"message_stop"}"#),
        ])
        .expect("the stream seals");
        assert_eq!(sealed.stop_reason, StopReason::Refusal);
        assert_eq!(sealed.blocks.len(), 1);
    }

    // -----------------------------------------------------------------------
    // the error table
    // -----------------------------------------------------------------------

    fn classify(status: u16, body: &str) -> crate::error::ProviderFailure {
        let map = headers(&[]);
        AnthropicAdapter.classify_http(
            status,
            &HeaderView::new(&map),
            &BoundedBody::new(body.as_bytes().to_vec(), false),
        )
    }

    #[test]
    fn every_documented_error_type_maps_to_its_kind() {
        let table: [(u16, &str, ProviderFailureKind); 11] = [
            (
                400,
                "invalid_request_error",
                ProviderFailureKind::InvalidRequest,
            ),
            (
                401,
                "authentication_error",
                ProviderFailureKind::Authentication,
            ),
            (402, "billing_error", ProviderFailureKind::Billing),
            (403, "permission_error", ProviderFailureKind::Authentication),
            (404, "not_found_error", ProviderFailureKind::ModelNotFound),
            (409, "conflict_error", ProviderFailureKind::InvalidRequest),
            (
                413,
                "request_too_large",
                ProviderFailureKind::InvalidRequest,
            ),
            (429, "rate_limit_error", ProviderFailureKind::RateLimited),
            (500, "api_error", ProviderFailureKind::ServerError),
            (504, "timeout_error", ProviderFailureKind::Timeout),
            (529, "overloaded_error", ProviderFailureKind::Overloaded),
        ];
        for (status, code, expected) in table {
            assert_eq!(kind_for_error_type(code), Some(expected), "{code}");
            let body = format!(
                r#"{{"type":"error","error":{{"type":"{code}","message":"nope"}},"request_id":"req_1"}}"#
            );
            let failure = classify(status, &body);
            assert_eq!(failure.kind(), expected, "{code}");
            assert_eq!(failure.detail.http_status, Some(status));
            assert_eq!(
                failure
                    .detail
                    .provider_code
                    .as_ref()
                    .map(BoundedString::as_str),
                Some(code)
            );
            assert_eq!(failure.detail.message.as_str(), "nope");
        }
        assert_eq!(kind_for_error_type("some_new_error"), None);
    }

    #[test]
    fn every_documented_status_maps_to_its_kind_without_a_body() {
        let table: [(u16, ProviderFailureKind); 12] = [
            (400, ProviderFailureKind::InvalidRequest),
            (401, ProviderFailureKind::Authentication),
            (402, ProviderFailureKind::Billing),
            (403, ProviderFailureKind::Authentication),
            (404, ProviderFailureKind::ModelNotFound),
            (409, ProviderFailureKind::InvalidRequest),
            (413, ProviderFailureKind::InvalidRequest),
            (429, ProviderFailureKind::RateLimited),
            (500, ProviderFailureKind::ServerError),
            (504, ProviderFailureKind::Timeout),
            (529, ProviderFailureKind::Overloaded),
            (418, ProviderFailureKind::ServerError),
        ];
        for (status, expected) in table {
            assert_eq!(kind_for_status(status), expected, "{status}");
            assert_eq!(classify(status, "<html>gateway</html>").kind(), expected);
        }
    }

    #[test]
    fn an_unknown_error_type_falls_back_to_the_status() {
        let failure = classify(
            429,
            r#"{"type":"error","error":{"type":"some_new_error","message":"slow down"}}"#,
        );
        assert_eq!(failure.kind(), ProviderFailureKind::RateLimited);
    }

    #[test]
    fn an_error_body_is_redacted_before_it_reaches_the_detail() {
        let key = "sk-ant-api03-AAAABBBBCCCCDDDDEEEE1111";
        let body = format!(
            r#"{{"type":"error","error":{{"type":"authentication_error","message":"invalid x-api-key: {key}"}}}}"#
        );
        let failure = classify(401, &body);
        assert!(
            !failure.detail.message.as_str().contains(key),
            "{}",
            failure.detail.message
        );
        assert!(failure.detail.message.as_str().contains("[redacted]"));
    }

    #[test]
    fn a_truncated_error_body_says_so_rather_than_guessing() {
        let map = headers(&[]);
        let failure = AnthropicAdapter.classify_http(
            500,
            &HeaderView::new(&map),
            &BoundedBody::new(br#"{"type":"error","error":"#.to_vec(), true),
        );
        assert_eq!(failure.kind(), ProviderFailureKind::ServerError);
        assert!(
            failure.detail.message.as_str().contains("read bound"),
            "{}",
            failure.detail.message
        );
    }

    #[test]
    fn an_in_stream_error_event_is_a_known_failure_that_records_the_request_id() {
        let mut state = fresh_state();
        let outcomes = feed(
            &mut state,
            &[
                ("message_start", MESSAGE_START),
                (
                    "error",
                    r#"{"type":"error","error":{"type":"overloaded_error","message":"Overloaded"},"request_id":"req_abc"}"#,
                ),
            ],
        )
        .expect("the frame decodes");
        let FrameOutcome::Failed(failure) = &outcomes[1] else {
            panic!("expected a failure outcome");
        };
        assert_eq!(failure.kind(), ProviderFailureKind::Overloaded);
        assert_eq!(failure.detail.message.as_str(), "Overloaded");
        let map = headers(&[]);
        assert_eq!(
            AnthropicAdapter
                .request_id(&HeaderView::new(&map), &state)
                .map(|id| id.as_str().to_owned()),
            Some("req_abc".to_owned())
        );
    }

    #[test]
    fn an_error_event_may_arrive_before_any_content() {
        let mut state = fresh_state();
        let outcomes = feed(
            &mut state,
            &[(
                "error",
                r#"{"type":"error","error":{"type":"api_error","message":"boom"}}"#,
            )],
        )
        .expect("an error may precede message_start after a 200");
        assert!(matches!(outcomes[0], FrameOutcome::Failed(_)));
    }

    // -----------------------------------------------------------------------
    // backpressure and correlation
    // -----------------------------------------------------------------------

    #[test]
    fn vendor_rate_limit_headers_are_read_in_full() {
        let map = headers(&[
            ("retry-after", "8"),
            ("anthropic-ratelimit-requests-remaining", "3"),
            ("anthropic-ratelimit-tokens-remaining", "1200"),
            ("anthropic-ratelimit-requests-reset", "2027-01-01T00:00:00Z"),
            ("anthropic-ratelimit-tokens-reset", "2027-01-01T00:01:00Z"),
        ]);
        let feedback = AnthropicAdapter.rate_limit_feedback(&HeaderView::new(&map));
        assert_eq!(feedback.source, RateLimitSource::VendorHeaders);
        assert_eq!(
            feedback.retry_after,
            Some(core::time::Duration::from_secs(8))
        );
        assert_eq!(feedback.requests_remaining, Some(3));
        assert_eq!(feedback.tokens_remaining, Some(1_200));
        assert_eq!(
            feedback.reset_at.map(aex_wire::types::Timestamp::to_wire),
            Some("2027-01-01T00:01:00.000Z".to_owned()),
            "the answer is the instant after which every window has rolled over"
        );
    }

    #[test]
    fn the_tighter_token_sub_budget_answers_when_the_aggregate_is_absent() {
        let map = headers(&[
            ("anthropic-ratelimit-input-tokens-remaining", "900"),
            ("anthropic-ratelimit-output-tokens-remaining", "40"),
        ]);
        let feedback = AnthropicAdapter.rate_limit_feedback(&HeaderView::new(&map));
        assert_eq!(feedback.tokens_remaining, Some(40));
        assert_eq!(feedback.source, RateLimitSource::VendorHeaders);
    }

    #[test]
    fn retry_after_alone_is_recorded_as_a_header_answer() {
        let map = headers(&[("retry-after", "30")]);
        let feedback = AnthropicAdapter.rate_limit_feedback(&HeaderView::new(&map));
        assert_eq!(feedback.source, RateLimitSource::RetryAfterHeader);
        assert_eq!(
            feedback.retry_after,
            Some(core::time::Duration::from_secs(30))
        );
        assert_eq!(feedback.reset_at, None);
    }

    #[test]
    fn no_backpressure_headers_at_all_is_a_positive_absence() {
        let map = headers(&[]);
        let feedback = AnthropicAdapter.rate_limit_feedback(&HeaderView::new(&map));
        assert!(feedback.is_absent());
        assert_eq!(feedback.source, RateLimitSource::NotProvided);
    }

    #[test]
    fn the_request_id_header_wins_over_the_body_and_falls_back_to_it() {
        let mut state = fresh_state();
        feed(
            &mut state,
            &[(
                "error",
                r#"{"type":"error","error":{"type":"api_error","message":"boom"},"request_id":"req_body"}"#,
            )],
        )
        .expect("the error decodes");
        let with_header = headers(&[("request-id", "req_header")]);
        assert_eq!(
            AnthropicAdapter
                .request_id(&HeaderView::new(&with_header), &state)
                .map(|id| id.as_str().to_owned()),
            Some("req_header".to_owned())
        );
        let without = headers(&[]);
        assert_eq!(
            AnthropicAdapter
                .request_id(&HeaderView::new(&without), &state)
                .map(|id| id.as_str().to_owned()),
            Some("req_body".to_owned())
        );
    }

    #[test]
    fn a_classified_failure_carries_the_backpressure_from_the_same_response() {
        let map = headers(&[
            ("retry-after", "12"),
            ("anthropic-ratelimit-requests-remaining", "0"),
        ]);
        let failure = AnthropicAdapter.classify_http(
            429,
            &HeaderView::new(&map),
            &BoundedBody::new(
                br#"{"type":"error","error":{"type":"rate_limit_error","message":"slow"}}"#
                    .to_vec(),
                false,
            ),
        );
        assert_eq!(failure.kind(), ProviderFailureKind::RateLimited);
        assert_eq!(
            failure.rate_limit.retry_after,
            Some(core::time::Duration::from_secs(12))
        );
        assert_eq!(failure.rate_limit.source, RateLimitSource::VendorHeaders);
    }
}
