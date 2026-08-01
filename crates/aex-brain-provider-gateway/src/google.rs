//! The `google` dialect adapter: the Gemini Developer API `generateContent`
//! surface (plan 08 §5.6).
//!
//! # Two facts this module exists to make structural
//!
//! - `POST /v1beta/models/{model}:streamGenerateContent` answers with a `JSON`
//!   **array** unless `?alt=sse` is present, so the query parameter is compiled
//!   into the path builder and is never optional (D-14).
//! - the `?key=` query form of the credential is forbidden: it would place a
//!   customer key in a `URL` that reaches proxies, access logs and error strings
//!   (D-15). [`crate::transport::AuthScheme::GoogleApiKeyHeader`] is the only
//!   credential channel, and this module cannot construct a query parameter
//!   named `key` because the only name it ever writes is a compiled constant.
//!
//! # Launch exclusions
//!
//! Vertex AI, the Live `WebSocket` API, provider-hosted tools and the
//! Interactions API (`POST /v1beta/interactions`, GA but server-stateful by
//! default) are all out (D-19). Receiving a frame that only a hosted tool could
//! produce is a `ProtocolViolation`, because such a tool is never sent.
//!
//! # The usage trap
//!
//! Gemini documents `thoughtsTokenCount` as **excluded** from
//! `candidatesTokenCount`, which is why the catalog's
//! `reasoning_included_in_output` is `false` for `google` and why this adapter
//! computes `output_tokens = candidatesTokenCount + thoughtsTokenCount` with
//! `reasoning_tokens = thoughtsTokenCount`. One arithmetic definition reaches
//! every consumer (D-11).
//!
//! # Completion proof
//!
//! There is no `[DONE]` sentinel: the stream simply ends. A terminal
//! `finishReason` is therefore the only completion proof, and a stream that ends
//! without one is a failure rather than a shorter success.

use std::collections::BTreeMap;

use aex_model_catalog::QualifiedModel;
use aex_model_catalog::canonical::{
    CacheBreakpoint, CanonicalBlock, CanonicalMessage, CanonicalModelRequest, CanonicalToolDef,
    NormalizedUsage, REASON_MAX, ReasoningBlock, ReasoningBody, ReasoningEffort, ReasoningRequest,
    ReasoningToken, Role, StopReason, StructuredOutputRequest, SystemBlock, TEXT_MAX, ToolChoice,
    ToolResultPart, UsageCompleteness, UsageField, UsageFieldSet,
};
use aex_model_catalog::document::{
    AdapterSourceDigest, CacheMode, Capability, EndpointPin, ModelEntry, ReasoningMode,
    ReasoningReplay, SamplingSupport, SchemaEncoding, StructuredOutputPolicy, ToolEncoding,
};
use aex_model_catalog::primitives::{
    Blake3Digest, BoundedString, ProviderRequestId, ToolCallId, ToolName,
};
use aex_wire::CanonicalJson;
use aex_wire::provider::ProviderId;
use bytes::Bytes;
use serde_json::{Map, Value};

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

/// The identity [`ProviderAdapter::source_digest`] is taken over.
///
/// `TODO(cross-stream): the release tool replaces this with the blake3 digest of
/// the adapter source tree it built, which is what a conformance receipt is
/// bound to (D-06). Until then it is a compiled dialect identity, stable across
/// a build and distinct per dialect.`
pub const DIALECT_TAG: &str = "aex-brain-provider-gateway/google/generateContent/1";

/// The media type every Gemini structured-output form takes.
const JSON_MIME: &str = "application/json";

/// The query parameter without which Gemini answers with a `JSON` array.
const ALT_PARAMETER: &str = "alt";

/// Its one permitted value.
const ALT_SSE: &str = "sse";

/// The block index the single accumulated reasoning block occupies.
const REASONING_BLOCK: u16 = 0;

/// The block index the single accumulated text block occupies.
const TEXT_BLOCK: u16 = 1;

/// The first block index a decoded function call occupies.
const FIRST_TOOL_BLOCK: u16 = 2;

/// The top-level members a `GenerateContentResponse` frame may carry. Anything
/// else means the dialect changed under us, which is a protocol violation rather
/// than a member to ignore.
const FRAME_KEYS: [&str; 6] = [
    "candidates",
    "createTime",
    "modelVersion",
    "promptFeedback",
    "responseId",
    "usageMetadata",
];

/// The `Part` members this dialect speaks. `inlineData`, `fileData`,
/// `executableCode`, `codeExecutionResult` and `functionResponse` are the shapes
/// a provider-hosted tool would arrive in, and those are never requested.
const PART_KEYS: [&str; 4] = ["functionCall", "text", "thought", "thoughtSignature"];

/// What a documented `finishReason` means.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FinishClass {
    /// The model finished its turn; tool use is decided by the block set.
    Stop,
    /// The output ceiling was reached.
    MaxTokens,
    /// The token is terminal but is a failure, never a stop reason (D-09).
    Failure(ProviderFailureKind),
}

/// The complete documented `finishReason` enumeration and what each member
/// means. Content filtering, recitation and malformed calls are **failures**.
const FINISH_REASONS: [(&str, FinishClass); 14] = [
    ("STOP", FinishClass::Stop),
    ("MAX_TOKENS", FinishClass::MaxTokens),
    (
        "SAFETY",
        FinishClass::Failure(ProviderFailureKind::ContentFiltered),
    ),
    (
        "PROHIBITED_CONTENT",
        FinishClass::Failure(ProviderFailureKind::ContentFiltered),
    ),
    (
        "BLOCKLIST",
        FinishClass::Failure(ProviderFailureKind::ContentFiltered),
    ),
    (
        "SPII",
        FinishClass::Failure(ProviderFailureKind::ContentFiltered),
    ),
    (
        "IMAGE_SAFETY",
        FinishClass::Failure(ProviderFailureKind::ContentFiltered),
    ),
    (
        "RECITATION",
        FinishClass::Failure(ProviderFailureKind::ProtocolViolation),
    ),
    (
        "LANGUAGE",
        FinishClass::Failure(ProviderFailureKind::ProtocolViolation),
    ),
    (
        "OTHER",
        FinishClass::Failure(ProviderFailureKind::ProtocolViolation),
    ),
    (
        "FINISH_REASON_UNSPECIFIED",
        FinishClass::Failure(ProviderFailureKind::ProtocolViolation),
    ),
    (
        "MALFORMED_FUNCTION_CALL",
        FinishClass::Failure(ProviderFailureKind::InvalidRequest),
    ),
    (
        "UNEXPECTED_TOOL_CALL",
        FinishClass::Failure(ProviderFailureKind::InvalidRequest),
    ),
    (
        "TOO_MANY_TOOL_CALLS",
        FinishClass::Failure(ProviderFailureKind::InvalidRequest),
    ),
];

/// The `snake_case` error codes the current reference documents. Any other code
/// is documented as the `snake_case` rendering of the `HTTP` status text, which is
/// exactly what falling through to [`GoogleAdapter::kind_for_status`] produces.
const SNAKE_CODES: [(&str, ProviderFailureKind); 8] = [
    ("api_error", ProviderFailureKind::ServerError),
    ("authentication", ProviderFailureKind::Authentication),
    ("invalid_request", ProviderFailureKind::InvalidRequest),
    ("not_found", ProviderFailureKind::ModelNotFound),
    ("permission_denied", ProviderFailureKind::Authentication),
    ("quota_exceeded", ProviderFailureKind::Quota),
    ("rate_limit_exceeded", ProviderFailureKind::RateLimited),
    ("service_unavailable", ProviderFailureKind::Overloaded),
];

/// The classic `gRPC` status vocabulary. No longer documented, still observed.
const GRPC_STATUSES: [(&str, ProviderFailureKind); 10] = [
    ("CANCELLED", ProviderFailureKind::Cancelled),
    ("DEADLINE_EXCEEDED", ProviderFailureKind::Timeout),
    ("FAILED_PRECONDITION", ProviderFailureKind::InvalidRequest),
    ("INTERNAL", ProviderFailureKind::ServerError),
    ("INVALID_ARGUMENT", ProviderFailureKind::InvalidRequest),
    ("NOT_FOUND", ProviderFailureKind::ModelNotFound),
    ("PERMISSION_DENIED", ProviderFailureKind::Authentication),
    ("RESOURCE_EXHAUSTED", ProviderFailureKind::RateLimited),
    ("UNAUTHENTICATED", ProviderFailureKind::Authentication),
    ("UNAVAILABLE", ProviderFailureKind::Overloaded),
];

/// The Gemini Developer API adapter.
///
/// Stateless: every decoder fact lives on the caller's
/// [`DialectState`], so one adapter value serves every concurrent stream.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct GoogleAdapter;

// ---------------------------------------------------------------------------
// request projection
// ---------------------------------------------------------------------------

/// The members of a [`CanonicalModelRequest`] this dialect encodes.
///
/// A borrowed projection with exactly one construction site
/// ([`GeminiRequestView::of`]). It exists because `CanonicalModelRequest`
/// carries a live [`QualifiedModel`] handle that only a loaded catalog can mint,
/// so the encoder — which needs no such handle — stays constructible, and
/// therefore testable, from catalog fixture data alone.
#[derive(Debug, Clone, Copy)]
struct GeminiRequestView<'a> {
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
}

impl<'a> GeminiRequestView<'a> {
    /// Projects a canonical request. Field-for-field, no interpretation.
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
        }
    }
}

// ---------------------------------------------------------------------------
// request building
// ---------------------------------------------------------------------------

impl GoogleAdapter {
    /// Builds the whole wire request from an entry and a projected request.
    fn encode(
        entry: &ModelEntry,
        view: &GeminiRequestView<'_>,
    ) -> Result<WireRequest, RequestBuildError> {
        Self::check_ceilings(entry, view)?;

        let mut body = Map::new();
        body.insert(
            "contents".to_owned(),
            Value::Array(Self::contents(entry, view)?),
        );
        if let Some(instruction) = Self::system_instruction(entry, view)? {
            body.insert("systemInstruction".to_owned(), instruction);
        }
        if let Some(declarations) = Self::tool_declarations(entry, view)? {
            body.insert("tools".to_owned(), declarations);
            body.insert("toolConfig".to_owned(), Self::tool_config(entry, view)?);
        } else if !matches!(view.tool_choice, ToolChoice::Auto | ToolChoice::None) {
            return Err(RequestBuildError::ToolChoiceUnsupported {
                requested: Box::new(view.tool_choice.clone()),
            });
        }
        body.insert(
            "generationConfig".to_owned(),
            Self::generation_config(entry, view)?,
        );

        let encoded =
            serde_json::to_vec(&Value::Object(body)).map_err(|_| RequestBuildError::Encoding {
                reason: "the request body could not be serialized",
            })?;
        let size = u32::try_from(encoded.len()).unwrap_or(u32::MAX);
        if size > entry.limits.request_body_max_bytes {
            return Err(RequestBuildError::BodyTooLarge {
                limit: entry.limits.request_body_max_bytes,
            });
        }

        Ok(WireRequest {
            endpoint: EndpointPin::GeminiV1Beta,
            path: Self::path_for(entry)?,
            // Compiled name, compiled value. There is no code path here that can
            // name a query parameter `key`.
            query: vec![(ALT_PARAMETER, BoundedString::truncating(ALT_SSE))],
            headers: vec![("content-type", BoundedString::truncating(JSON_MIME))],
            auth: AuthScheme::GoogleApiKeyHeader,
            body: Bytes::from(encoded),
            accept: Accept::TextEventStream,
        })
    }

    /// `/v1beta/models/{model}:streamGenerateContent`, from the catalog slug.
    ///
    /// The slug is re-validated against a bare-id grammar rather than trusted:
    /// a slug carrying `/` or `..` would otherwise steer the request off the
    /// pinned path.
    fn path_for(entry: &ModelEntry) -> Result<BoundedString<256>, RequestBuildError> {
        let slug = entry.model.as_str();
        let bare = !slug.is_empty()
            && slug
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'_'));
        if !bare {
            return Err(RequestBuildError::Encoding {
                reason: "the model slug is not a bare Gemini model id",
            });
        }
        BoundedString::new(format!("/v1beta/models/{slug}:streamGenerateContent")).map_err(|_| {
            RequestBuildError::Encoding {
                reason: "the model slug makes the request path exceed its bound",
            }
        })
    }

    /// Whether the entry declares a capability the request needs.
    fn require(entry: &ModelEntry, capability: Capability) -> Result<(), RequestBuildError> {
        if entry.capabilities.has(capability) {
            return Ok(());
        }
        Err(RequestBuildError::CapabilityUnavailable { capability })
    }

    /// Numeric and policy ceilings, all of which fire before any socket exists.
    fn check_ceilings(
        entry: &ModelEntry,
        view: &GeminiRequestView<'_>,
    ) -> Result<(), RequestBuildError> {
        let limits = &entry.limits;
        if view.max_output_tokens < limits.min_output_tokens
            || view.max_output_tokens > limits.max_output_tokens
        {
            return Err(RequestBuildError::OutputTokensOutOfRange {
                min: limits.min_output_tokens,
                max: limits.max_output_tokens,
            });
        }
        if !view.stop_sequences.is_empty() {
            Self::require(entry, Capability::StopSequences)?;
            if view.stop_sequences.len() > usize::from(limits.max_stop_sequences) {
                return Err(RequestBuildError::StopSequenceLimit {
                    max: limits.max_stop_sequences,
                });
            }
        }
        if !view.cache_breakpoints.is_empty() && entry.cache_policy.mode != CacheMode::Explicit {
            return Err(RequestBuildError::CapabilityUnavailable {
                capability: Capability::PromptCacheExplicit,
            });
        }
        Self::check_sampling(entry, view)
    }

    /// Sampling controls, never silently dropped.
    fn check_sampling(
        entry: &ModelEntry,
        view: &GeminiRequestView<'_>,
    ) -> Result<(), RequestBuildError> {
        let thinking = matches!(view.reasoning, ReasoningRequest::Enabled { .. })
            || entry.reasoning.mode == ReasoningMode::AlwaysOn;
        if let Some(milli) = view.temperature_milli {
            if entry.sampling == SamplingSupport::None {
                return Err(RequestBuildError::SamplingUnsupported {
                    field: "temperature",
                });
            }
            Self::require(entry, Capability::Temperature)?;
            if thinking && entry.reasoning.excludes_sampling {
                return Err(RequestBuildError::SamplingWithReasoning {
                    field: "temperature",
                });
            }
            Self::check_range(entry.limits.temperature_milli, milli, "temperature")?;
        }
        if let Some(milli) = view.top_p_milli {
            if entry.sampling != SamplingSupport::Full {
                return Err(RequestBuildError::SamplingUnsupported { field: "top_p" });
            }
            Self::require(entry, Capability::TopP)?;
            if thinking && entry.reasoning.excludes_sampling {
                return Err(RequestBuildError::SamplingWithReasoning { field: "top_p" });
            }
            Self::check_range(entry.limits.top_p_milli, milli, "top_p")?;
        }
        Ok(())
    }

    /// A milli-unit value against the pair's declared inclusive range.
    fn check_range(
        range: Option<(u16, u16)>,
        value: u16,
        field: &'static str,
    ) -> Result<(), RequestBuildError> {
        match range {
            Some((low, high)) if value >= low && value <= high => Ok(()),
            Some(_) | None => Err(RequestBuildError::SamplingUnsupported { field }),
        }
    }
}

// ---------------------------------------------------------------------------
// request body members
// ---------------------------------------------------------------------------

impl GoogleAdapter {
    /// The wire role. Gemini calls the assistant `model`.
    const fn role(role: Role) -> &'static str {
        match role {
            Role::User => "user",
            Role::Assistant => "model",
        }
    }

    /// A `{"text": …}` part.
    fn text_part(text: &str) -> Value {
        let mut part = Map::new();
        part.insert("text".to_owned(), Value::String(text.to_owned()));
        Value::Object(part)
    }

    /// `contents[]`, in conversation order.
    ///
    /// A turn whose blocks produce no part at all — an assistant turn carrying
    /// only reasoning, whose sole wire contribution is the `thoughtSignature`
    /// echoed on a function call — is omitted rather than sent empty, which
    /// Gemini rejects.
    fn contents(
        entry: &ModelEntry,
        view: &GeminiRequestView<'_>,
    ) -> Result<Vec<Value>, RequestBuildError> {
        let mut names: BTreeMap<&str, &str> = BTreeMap::new();
        let mut turns = Vec::with_capacity(view.messages.len());
        for message in view.messages {
            let signature = Self::turn_signature(message)?;
            let parts = Self::turn_parts(entry, message, signature.as_deref(), &mut names)?;
            if parts.is_empty() {
                continue;
            }
            let mut turn = Map::new();
            turn.insert("parts".to_owned(), Value::Array(parts));
            turn.insert(
                "role".to_owned(),
                Value::String(Self::role(message.role).to_owned()),
            );
            turns.push(Value::Object(turn));
        }
        Ok(turns)
    }

    /// The round-trip material this turn must echo, if it carries any.
    ///
    /// Every reasoning token on the turn is checked for provenance, so material
    /// minted by another provider can never be replayed here (D-12); the first
    /// one is what a function call echoes.
    fn turn_signature(message: &CanonicalMessage) -> Result<Option<String>, RequestBuildError> {
        let mut first: Option<String> = None;
        for block in &message.blocks {
            let CanonicalBlock::Reasoning(reasoning) = block else {
                continue;
            };
            let Some(token) = &reasoning.token else {
                continue;
            };
            if token.provenance != ProviderId::Google {
                return Err(RequestBuildError::ReasoningProvenanceMismatch {
                    expected: ProviderId::Google,
                    found: token.provenance,
                });
            }
            if first.is_none() {
                let text = core::str::from_utf8(&token.bytes).map_err(|_| {
                    RequestBuildError::Encoding {
                        reason: "a Gemini thoughtSignature must be valid UTF-8",
                    }
                })?;
                first = Some(text.to_owned());
            }
        }
        Ok(first)
    }

    /// One turn's parts, and the replay obligation the turn carries.
    fn turn_parts<'a>(
        entry: &ModelEntry,
        message: &'a CanonicalMessage,
        signature: Option<&str>,
        names: &mut BTreeMap<&'a str, &'a str>,
    ) -> Result<Vec<Value>, RequestBuildError> {
        let mut parts = Vec::with_capacity(message.blocks.len());
        let mut calls = 0usize;
        let mut reasoning = 0usize;
        for block in &message.blocks {
            match block {
                CanonicalBlock::Text { text, .. } => parts.push(Self::text_part(text.as_str())),
                CanonicalBlock::Refusal { text } => parts.push(Self::text_part(text.as_str())),
                CanonicalBlock::Reasoning(_) => reasoning += 1,
                CanonicalBlock::ToolUse { id, name, input } => {
                    names.insert(id.as_str(), name.as_str());
                    // In parallel calls only the first part carries one.
                    let echo = if calls == 0 { signature } else { None };
                    parts.push(Self::function_call_part(id, name, input, echo)?);
                    calls += 1;
                }
                CanonicalBlock::ToolResult {
                    call,
                    content,
                    is_error,
                } => parts.push(Self::function_response_part(
                    call, content, *is_error, names,
                )?),
            }
        }
        let required = match entry.reasoning.replay {
            ReasoningReplay::NotRequired | ReasoningReplay::RecommendedEcho => false,
            ReasoningReplay::RequiredWithToolCalls => calls > 0,
            ReasoningReplay::RequiredAlways => true,
        };
        if reasoning > 0 && required && signature.is_none() {
            return Err(RequestBuildError::ReasoningTokenRequired);
        }
        Ok(parts)
    }

    /// `{"functionCall":{"args","id","name"}}`, with `args` an object rather
    /// than a `JSON` string (`ToolArgumentEncoding::JsonObject`).
    fn function_call_part(
        id: &ToolCallId,
        name: &ToolName,
        input: &CanonicalJson,
        signature: Option<&str>,
    ) -> Result<Value, RequestBuildError> {
        let args = input.to_value();
        if !args.is_object() {
            return Err(RequestBuildError::Encoding {
                reason: "a Gemini functionCall takes an object argument set",
            });
        }
        let mut call = Map::new();
        call.insert("args".to_owned(), args);
        call.insert("id".to_owned(), Value::String(id.as_str().to_owned()));
        call.insert("name".to_owned(), Value::String(name.as_str().to_owned()));
        let mut part = Map::new();
        part.insert("functionCall".to_owned(), Value::Object(call));
        if let Some(signature) = signature {
            part.insert(
                "thoughtSignature".to_owned(),
                Value::String(signature.to_owned()),
            );
        }
        Ok(Value::Object(part))
    }

    /// `{"functionResponse":{"id","name","response"}}`.
    ///
    /// Gemini requires the function **name** on a response, which the canonical
    /// `ToolResult` block does not carry, so it is resolved from the call this
    /// conversation actually made. A result answering an unknown call is
    /// refused rather than guessed at.
    fn function_response_part(
        call: &ToolCallId,
        content: &[ToolResultPart],
        is_error: bool,
        names: &BTreeMap<&str, &str>,
    ) -> Result<Value, RequestBuildError> {
        let Some(name) = names.get(call.as_str()) else {
            return Err(RequestBuildError::Encoding {
                reason: "a tool result answers a call this conversation never made",
            });
        };
        let mut response = Map::new();
        response.insert(
            if is_error { "error" } else { "output" }.to_owned(),
            Self::tool_result_payload(content),
        );
        let mut answer = Map::new();
        answer.insert("id".to_owned(), Value::String(call.as_str().to_owned()));
        answer.insert("name".to_owned(), Value::String((*name).to_owned()));
        answer.insert("response".to_owned(), Value::Object(response));
        let mut part = Map::new();
        part.insert("functionResponse".to_owned(), Value::Object(answer));
        Ok(Value::Object(part))
    }

    /// One tool result's parts as a single `JSON` value.
    fn tool_result_payload(content: &[ToolResultPart]) -> Value {
        let mut values: Vec<Value> = content
            .iter()
            .map(|part| match part {
                ToolResultPart::Text { text } => Value::String(text.as_str().to_owned()),
                ToolResultPart::Json { value } => value.to_value(),
            })
            .collect();
        match values.len() {
            0 => Value::Null,
            1 => values.remove(0),
            _ => Value::Array(values),
        }
    }

    /// `systemInstruction`, Gemini's dedicated instruction channel.
    ///
    /// `SystemBlock::cacheable` has no wire counterpart here: Gemini's caching
    /// is implicit, and its explicit form is a `cachedContent` resource handle
    /// AEX does not mint. An explicit breakpoint is refused in
    /// [`GoogleAdapter::check_ceilings`] rather than silently downgraded.
    fn system_instruction(
        entry: &ModelEntry,
        view: &GeminiRequestView<'_>,
    ) -> Result<Option<Value>, RequestBuildError> {
        if view.system.is_empty() {
            return Ok(None);
        }
        Self::require(entry, Capability::SystemInstruction)?;
        let parts: Vec<Value> = view
            .system
            .iter()
            .map(|block| Self::text_part(block.text.as_str()))
            .collect();
        let mut instruction = Map::new();
        instruction.insert("parts".to_owned(), Value::Array(parts));
        Ok(Some(Value::Object(instruction)))
    }

    /// `tools:[{functionDeclarations:[…]}]`.
    fn tool_declarations(
        entry: &ModelEntry,
        view: &GeminiRequestView<'_>,
    ) -> Result<Option<Value>, RequestBuildError> {
        if view.tools.is_empty() {
            return Ok(None);
        }
        Self::require(entry, Capability::Tools)?;
        if entry.tool_policy.encoding != ToolEncoding::GeminiFunctionDeclarations {
            return Err(RequestBuildError::Encoding {
                reason: "the entry declares a tool encoding this dialect does not speak",
            });
        }
        if view.tools.len() > usize::from(entry.limits.max_tools) {
            return Err(RequestBuildError::ToolLimit {
                max: entry.limits.max_tools,
            });
        }
        if !view.parallel_tools {
            return Err(RequestBuildError::Encoding {
                reason: "google has no field that forbids parallel function calls",
            });
        }
        let mut declarations = Vec::with_capacity(view.tools.len());
        for tool in view.tools {
            declarations.push(Self::declaration(entry, tool)?);
        }
        let mut wrapper = Map::new();
        wrapper.insert(
            "functionDeclarations".to_owned(),
            Value::Array(declarations),
        );
        Ok(Some(Value::Array(vec![Value::Object(wrapper)])))
    }

    /// One `functionDeclaration`.
    fn declaration(
        entry: &ModelEntry,
        tool: &CanonicalToolDef,
    ) -> Result<Value, RequestBuildError> {
        let name = tool.name.as_str();
        if name.len() > usize::from(entry.tool_policy.max_name_bytes)
            || !entry.tool_policy.name_pattern.accepts(name)
        {
            return Err(RequestBuildError::ToolNameInvalid {
                name: tool.name.clone(),
            });
        }
        if tool.strict {
            // Gemini constrains a declared schema by default and offers no
            // per-function switch, so `strict` is a declaration the entry must
            // carry rather than a field this dialect writes.
            Self::require(entry, Capability::StrictToolSchema)?;
        }
        let parameters = tool.input_schema.to_value();
        if !parameters.is_object() {
            return Err(RequestBuildError::Encoding {
                reason: "a Gemini function declaration takes an object schema",
            });
        }
        let mut declaration = Map::new();
        declaration.insert(
            "description".to_owned(),
            Value::String(tool.description.as_str().to_owned()),
        );
        declaration.insert("name".to_owned(), Value::String(name.to_owned()));
        declaration.insert("parameters".to_owned(), parameters);
        Ok(Value::Object(declaration))
    }

    /// `toolConfig.functionCallingConfig`.
    fn tool_config(
        entry: &ModelEntry,
        view: &GeminiRequestView<'_>,
    ) -> Result<Value, RequestBuildError> {
        let mut config = Map::new();
        let mode = match view.tool_choice {
            ToolChoice::Auto => "AUTO",
            ToolChoice::None => {
                Self::require(entry, Capability::ToolChoiceNone)?;
                "NONE"
            }
            ToolChoice::Required => {
                Self::require(entry, Capability::ToolChoiceRequired)?;
                "ANY"
            }
            ToolChoice::Named { name } => {
                Self::require(entry, Capability::ToolChoiceNamed)?;
                config.insert(
                    "allowedFunctionNames".to_owned(),
                    Value::Array(vec![Value::String(name.as_str().to_owned())]),
                );
                "ANY"
            }
        };
        config.insert("mode".to_owned(), Value::String(mode.to_owned()));
        let mut wrapper = Map::new();
        wrapper.insert("functionCallingConfig".to_owned(), Value::Object(config));
        Ok(Value::Object(wrapper))
    }

    /// `generationConfig`.
    fn generation_config(
        entry: &ModelEntry,
        view: &GeminiRequestView<'_>,
    ) -> Result<Value, RequestBuildError> {
        let mut config = Map::new();
        config.insert(
            "maxOutputTokens".to_owned(),
            Value::from(view.max_output_tokens),
        );
        if let Some(milli) = view.temperature_milli {
            config.insert("temperature".to_owned(), Self::milli(milli)?);
        }
        if let Some(milli) = view.top_p_milli {
            config.insert("topP".to_owned(), Self::milli(milli)?);
        }
        if !view.stop_sequences.is_empty() {
            config.insert(
                "stopSequences".to_owned(),
                Value::Array(
                    view.stop_sequences
                        .iter()
                        .map(|sequence| Value::String(sequence.as_str().to_owned()))
                        .collect(),
                ),
            );
        }
        Self::structured_output(entry, view, &mut config)?;
        if let Some(thinking) = Self::thinking_config(entry, view)? {
            config.insert("thinkingConfig".to_owned(), thinking);
        }
        Ok(Value::Object(config))
    }

    /// A milli-unit sampling value as the fraction Gemini takes.
    fn milli(value: u16) -> Result<Value, RequestBuildError> {
        serde_json::Number::from_f64(f64::from(value) / 1000.0)
            .map(Value::Number)
            .ok_or(RequestBuildError::Encoding {
                reason: "a sampling value could not be represented as a JSON number",
            })
    }

    /// Whichever of the two documented structured-output forms the **entry**
    /// declares. The adapter never guesses: probe P-08 decides per model and
    /// the receipt records the answer.
    fn structured_output(
        entry: &ModelEntry,
        view: &GeminiRequestView<'_>,
        config: &mut Map<String, Value>,
    ) -> Result<(), RequestBuildError> {
        let Some(requested) = view.structured_output else {
            return Ok(());
        };
        Self::require(entry, Capability::StructuredOutput)?;
        let refuse = || RequestBuildError::StructuredOutputUnsupported {
            requested: Box::new(requested.clone()),
        };
        let schema = match requested {
            StructuredOutputRequest::JsonObject => None,
            StructuredOutputRequest::JsonSchema { schema, strict, .. } => {
                // Gemini always constrains a declared schema; there is no
                // best-effort mode to ask for.
                if !*strict {
                    return Err(refuse());
                }
                let value = schema.to_value();
                if !value.is_object() {
                    return Err(RequestBuildError::Encoding {
                        reason: "a Gemini response schema must be a JSON object",
                    });
                }
                Some(value)
            }
        };
        match entry.structured_output {
            StructuredOutputPolicy::JsonObjectOnly => {
                if schema.is_some() {
                    return Err(refuse());
                }
                config.insert(
                    "responseMimeType".to_owned(),
                    Value::String(JSON_MIME.to_owned()),
                );
                Ok(())
            }
            StructuredOutputPolicy::JsonSchema {
                encoding: SchemaEncoding::GeminiGenerationConfigFlat,
                ..
            } => {
                config.insert(
                    "responseMimeType".to_owned(),
                    Value::String(JSON_MIME.to_owned()),
                );
                if let Some(schema) = schema {
                    config.insert("responseSchema".to_owned(), schema);
                }
                Ok(())
            }
            StructuredOutputPolicy::JsonSchema {
                encoding: SchemaEncoding::GeminiResponseFormatNested,
                ..
            } => {
                let mut text = Map::new();
                text.insert("mimeType".to_owned(), Value::String(JSON_MIME.to_owned()));
                if let Some(schema) = schema {
                    text.insert("schema".to_owned(), schema);
                }
                let mut format = Map::new();
                format.insert("text".to_owned(), Value::Object(text));
                config.insert("responseFormat".to_owned(), Value::Object(format));
                Ok(())
            }
            // An entry that names another provider's encoding is a publishing
            // mistake, refused rather than reinterpreted.
            StructuredOutputPolicy::Unsupported | StructuredOutputPolicy::JsonSchema { .. } => {
                Err(refuse())
            }
        }
    }

    /// `generationConfig.thinkingConfig`.
    fn thinking_config(
        entry: &ModelEntry,
        view: &GeminiRequestView<'_>,
    ) -> Result<Option<Value>, RequestBuildError> {
        match view.reasoning {
            ReasoningRequest::ProviderDefault => Ok(None),
            ReasoningRequest::Disabled => Self::thinking_disabled(entry),
            ReasoningRequest::Enabled {
                budget_tokens,
                effort,
            } => Self::thinking_enabled(entry, budget_tokens, effort),
        }
    }

    /// Reasoning explicitly off. Gemini 2.5 Pro cannot be turned off, which the
    /// entry states as `ReasoningMode::AlwaysOn`.
    fn thinking_disabled(entry: &ModelEntry) -> Result<Option<Value>, RequestBuildError> {
        match entry.reasoning.mode {
            ReasoningMode::Unsupported => Ok(None),
            ReasoningMode::AlwaysOn => Err(RequestBuildError::Encoding {
                reason: "this pair cannot disable reasoning",
            }),
            ReasoningMode::Optional => {
                let mut config = Map::new();
                config.insert("includeThoughts".to_owned(), Value::Bool(false));
                config.insert("thinkingBudget".to_owned(), Value::from(0u32));
                Ok(Some(Value::Object(config)))
            }
        }
    }

    /// Reasoning explicitly on: `thinkingLevel` (3.x) **or** `thinkingBudget`
    /// (2.5), never both.
    fn thinking_enabled(
        entry: &ModelEntry,
        budget_tokens: Option<u32>,
        effort: Option<ReasoningEffort>,
    ) -> Result<Option<Value>, RequestBuildError> {
        Self::require(entry, Capability::Reasoning)?;
        if entry.reasoning.mode == ReasoningMode::Unsupported {
            return Err(RequestBuildError::CapabilityUnavailable {
                capability: Capability::Reasoning,
            });
        }
        if budget_tokens.is_some() && effort.is_some() {
            return Err(RequestBuildError::Encoding {
                reason: "google takes either thinkingLevel or thinkingBudget, never both",
            });
        }
        let mut config = Map::new();
        // Thoughts must be requested for `thoughtSignature` capture to be
        // possible at all, and the entry requires the material with tool calls.
        config.insert("includeThoughts".to_owned(), Value::Bool(true));
        if let Some(budget) = budget_tokens {
            let min = entry.limits.min_reasoning_tokens.unwrap_or(0);
            let max = entry.limits.max_reasoning_tokens.unwrap_or(u32::MAX);
            if budget < min || budget > max {
                return Err(RequestBuildError::ReasoningBudgetOutOfRange { min, max });
            }
            config.insert("thinkingBudget".to_owned(), Value::from(budget));
        }
        if let Some(level) = effort {
            config.insert(
                "thinkingLevel".to_owned(),
                Value::String(Self::thinking_level(level).to_owned()),
            );
        }
        Ok(Some(Value::Object(config)))
    }

    /// The neutral effort ladder onto Gemini's `thinkingLevel` vocabulary. The
    /// accepted set is per-model and is what probe R-2 records.
    const fn thinking_level(effort: ReasoningEffort) -> &'static str {
        match effort {
            ReasoningEffort::Minimal | ReasoningEffort::Low => "low",
            ReasoningEffort::Medium => "medium",
            ReasoningEffort::High | ReasoningEffort::Max => "high",
        }
    }
}

// ---------------------------------------------------------------------------
// stream decoding
// ---------------------------------------------------------------------------

impl GoogleAdapter {
    /// The compiled table row for a provider finish token.
    fn finish_entry(token: &str) -> Option<(&'static str, FinishClass)> {
        FINISH_REASONS
            .iter()
            .copied()
            .find(|(name, _)| *name == token)
    }

    /// Rejects a top-level member this dialect does not send.
    fn check_frame_keys(frame: &Map<String, Value>) -> Result<(), FrameDecodeError> {
        for key in frame.keys() {
            if !FRAME_KEYS.contains(&key.as_str()) {
                return Err(FrameDecodeError::UnknownEvent {
                    event: BoundedString::truncating(key),
                });
            }
        }
        Ok(())
    }

    /// `responseId`, the only request correlation Gemini publishes.
    fn read_response_id(
        state: &mut DialectState,
        frame: &Map<String, Value>,
    ) -> Result<(), FrameDecodeError> {
        let Some(value) = frame.get("responseId") else {
            return Ok(());
        };
        let text = value.as_str().ok_or(FrameDecodeError::MalformedField {
            field: "responseId",
        })?;
        state.request_id =
            Some(
                ProviderRequestId::new(text).map_err(|_| FrameDecodeError::MalformedField {
                    field: "responseId",
                })?,
            );
        Ok(())
    }

    /// `usageMetadata`, with the exclusion arithmetic applied once.
    ///
    /// `thoughtsTokenCount` is excluded from `candidatesTokenCount`, so
    /// `output_tokens` is their sum; `cachedContentTokenCount` is a subset of
    /// `promptTokenCount`, so the uncached remainder is the input count.
    fn normalized_usage(value: &Value) -> Result<NormalizedUsage, FrameDecodeError> {
        let map = value.as_object().ok_or(FrameDecodeError::MalformedField {
            field: "usageMetadata",
        })?;
        let count = |key: &'static str| -> Result<Option<u64>, FrameDecodeError> {
            match map.get(key) {
                None | Some(Value::Null) => Ok(None),
                Some(found) => found
                    .as_u64()
                    .map(Some)
                    .ok_or(FrameDecodeError::MalformedField { field: key }),
            }
        };
        let prompt = count("promptTokenCount")?;
        let cached = count("cachedContentTokenCount")?;
        let produced = count("candidatesTokenCount")?;
        let thoughts = count("thoughtsTokenCount")?;
        let tool_prompt = count("toolUsePromptTokenCount")?;
        let total = count("totalTokenCount")?;

        let mut missing = UsageFieldSet::EMPTY;
        if prompt.is_none() {
            missing = missing.with(UsageField::InputTokens);
        }
        if produced.is_none() {
            missing = missing.with(UsageField::OutputTokens);
        }
        if total.is_none() {
            missing = missing.with(UsageField::ProviderTotalTokens);
        }
        let nothing = prompt.is_none()
            && cached.is_none()
            && produced.is_none()
            && thoughts.is_none()
            && tool_prompt.is_none()
            && total.is_none();

        let cache_read = cached.unwrap_or(0);
        let reasoning = thoughts.unwrap_or(0);
        Ok(NormalizedUsage {
            input_tokens: prompt.unwrap_or(0).saturating_sub(cache_read),
            cache_read_input_tokens: cache_read,
            // Gemini publishes no cache-write counter at all.
            cache_write_input_tokens: 0,
            output_tokens: produced.unwrap_or(0).saturating_add(reasoning),
            reasoning_tokens: reasoning,
            tool_use_prompt_tokens: tool_prompt.unwrap_or(0),
            provider_total_tokens: total,
            completeness: if nothing {
                UsageCompleteness::Absent
            } else if missing.is_empty() {
                UsageCompleteness::Exact
            } else {
                UsageCompleteness::Partial { missing }
            },
        })
    }

    /// A prompt Gemini refused to answer at all.
    fn prompt_block(
        frame: &Map<String, Value>,
    ) -> Result<Option<ProviderFailure>, FrameDecodeError> {
        let Some(feedback) = frame.get("promptFeedback") else {
            return Ok(None);
        };
        let feedback = feedback
            .as_object()
            .ok_or(FrameDecodeError::MalformedField {
                field: "promptFeedback",
            })?;
        let Some(reason) = feedback.get("blockReason") else {
            return Ok(None);
        };
        let reason = reason.as_str().ok_or(FrameDecodeError::MalformedField {
            field: "blockReason",
        })?;
        if reason == "BLOCK_REASON_UNSPECIFIED" {
            return Ok(None);
        }
        Ok(Some(ProviderFailure::new(
            RedactedDetail::internal(
                ProviderFailureKind::ContentFiltered,
                "the provider blocked the prompt",
            )
            .with_code(redact::<64>(reason, &[]).as_str()),
        )))
    }

    /// `candidates[0]`, its parts and its `finishReason`.
    ///
    /// Returns the terminal outcome where the frame carried one, and `None`
    /// where it carried only content.
    fn read_candidate(
        state: &mut DialectState,
        candidates: &Value,
        budget: &StreamBudget,
    ) -> Result<Option<FrameOutcome>, FrameDecodeError> {
        let list = candidates
            .as_array()
            .ok_or(FrameDecodeError::MalformedField {
                field: "candidates",
            })?;
        // AEX never sets `candidateCount`, so a second candidate means the
        // request was altered in flight.
        if list.len() > 1 {
            return Err(FrameDecodeError::MalformedField {
                field: "candidates",
            });
        }
        let Some(candidate) = list.first() else {
            return Ok(None);
        };
        let candidate = candidate
            .as_object()
            .ok_or(FrameDecodeError::MalformedField {
                field: "candidates",
            })?;
        if let Some(content) = candidate.get("content") {
            let content = content
                .as_object()
                .ok_or(FrameDecodeError::MalformedField { field: "content" })?;
            if let Some(parts) = content.get("parts") {
                let parts = parts
                    .as_array()
                    .ok_or(FrameDecodeError::MalformedField { field: "parts" })?;
                for part in parts {
                    Self::read_part(state, part, budget)?;
                }
            }
        }
        let Some(reason) = candidate.get("finishReason") else {
            return Ok(None);
        };
        let token = reason.as_str().ok_or(FrameDecodeError::MalformedField {
            field: "finishReason",
        })?;
        let (literal, class) =
            Self::finish_entry(token).ok_or(FrameDecodeError::MalformedField {
                field: "finishReason",
            })?;
        match class {
            FinishClass::Stop | FinishClass::MaxTokens => {
                state.finish_token = Some(literal.to_owned());
                state.terminal = true;
                Ok(Some(FrameOutcome::Terminal))
            }
            FinishClass::Failure(kind) => {
                Ok(Some(FrameOutcome::Failed(Box::new(ProviderFailure::new(
                    RedactedDetail::internal(kind, "the provider ended the generation")
                        .with_code(literal),
                )))))
            }
        }
    }

    /// One `Part`. Function calls arrive whole; text and thoughts accumulate.
    fn read_part(
        state: &mut DialectState,
        part: &Value,
        budget: &StreamBudget,
    ) -> Result<(), FrameDecodeError> {
        let part = part
            .as_object()
            .ok_or(FrameDecodeError::MalformedField { field: "parts" })?;
        for key in part.keys() {
            if !PART_KEYS.contains(&key.as_str()) {
                return Err(FrameDecodeError::UnknownEvent {
                    event: BoundedString::truncating(key),
                });
            }
        }
        if let Some(signature) = part.get("thoughtSignature") {
            let text = signature.as_str().ok_or(FrameDecodeError::MalformedField {
                field: "thoughtSignature",
            })?;
            // In parallel calls only the first part carries one, so the first
            // seen is the one the next turn must echo.
            state
                .open_reasoning_token
                .entry(REASONING_BLOCK)
                .or_insert_with(|| text.as_bytes().to_vec());
        }
        if let Some(call) = part.get("functionCall") {
            return Self::read_function_call(state, call, budget);
        }
        let Some(text) = part.get("text") else {
            return Ok(());
        };
        let text = text
            .as_str()
            .ok_or(FrameDecodeError::MalformedField { field: "text" })?;
        let bytes = u64::try_from(text.len()).unwrap_or(u64::MAX);
        if part
            .get("thought")
            .and_then(Value::as_bool)
            .unwrap_or(false)
        {
            state.ledger.charge_reasoning(budget, bytes)?;
            if !state.open_reasoning.contains_key(&REASONING_BLOCK) {
                state.ledger.open_block(budget)?;
            }
            state
                .open_reasoning
                .entry(REASONING_BLOCK)
                .or_default()
                .push_str(text);
        } else {
            state.ledger.charge_text(budget, bytes)?;
            if !state.open_text.contains_key(&TEXT_BLOCK) {
                state.ledger.open_block(budget)?;
            }
            state
                .open_text
                .entry(TEXT_BLOCK)
                .or_default()
                .push_str(text);
        }
        Ok(())
    }

    /// A whole `functionCall` part.
    fn read_function_call(
        state: &mut DialectState,
        call: &Value,
        budget: &StreamBudget,
    ) -> Result<(), FrameDecodeError> {
        let call = call.as_object().ok_or(FrameDecodeError::MalformedField {
            field: "functionCall",
        })?;
        let name =
            call.get("name")
                .and_then(Value::as_str)
                .ok_or(FrameDecodeError::MalformedField {
                    field: "functionCall.name",
                })?;
        // Gemini 3 always returns a unique id and it must be mirrored, so an
        // absent one is a protocol violation rather than an id to invent.
        let raw =
            call.get("id")
                .and_then(Value::as_str)
                .ok_or(FrameDecodeError::MalformedField {
                    field: "functionCall.id",
                })?;
        let id = ToolCallId::new(raw).map_err(|_| FrameDecodeError::MalformedField {
            field: "functionCall.id",
        })?;
        let arguments = match call.get("args") {
            None | Some(Value::Null) => "{}".to_owned(),
            Some(args) if args.is_object() => {
                serde_json::to_string(args).map_err(|_| FrameDecodeError::MalformedField {
                    field: "functionCall.args",
                })?
            }
            Some(_) => {
                return Err(FrameDecodeError::MalformedField {
                    field: "functionCall.args",
                });
            }
        };
        BudgetLedger::check_tool_arguments(budget, &id, arguments.len())?;
        state.ledger.open_tool_call(budget)?;
        state.ledger.open_block(budget)?;
        let index = FIRST_TOOL_BLOCK
            .saturating_add(u16::try_from(state.open_tools.len()).unwrap_or(u16::MAX));
        state.open_tools.insert(
            index,
            PartialToolCall {
                id: Some(id),
                name: Some(name.to_owned()),
                arguments,
            },
        );
        Ok(())
    }

    /// The single accumulated reasoning block, where the stream produced one.
    fn reasoning_block(state: &DialectState) -> Result<Option<CanonicalBlock>, FrameDecodeError> {
        let text = state.open_reasoning.get(&REASONING_BLOCK);
        let material = state.open_reasoning_token.get(&REASONING_BLOCK);
        if text.is_none() && material.is_none() {
            return Ok(None);
        }
        let body = match text {
            Some(text) => ReasoningBody::Text {
                text: BoundedString::new(text.as_str()).map_err(|_| {
                    FrameDecodeError::Budget(BudgetOverrun::Reasoning {
                        limit: u64::try_from(REASON_MAX).unwrap_or(u64::MAX),
                    })
                })?,
            },
            // A `thoughtSignature` with no thought text is the ordinary Gemini 3
            // shape when thoughts were not requested: the material is all there
            // is, which is exactly what `Redacted` means.
            None => ReasoningBody::Redacted,
        };
        Ok(Some(CanonicalBlock::Reasoning(ReasoningBlock {
            body,
            token: material.map(|bytes| ReasoningToken {
                provenance: ProviderId::Google,
                bytes: Bytes::copy_from_slice(bytes),
            }),
        })))
    }

    /// A fresh decoder state.
    ///
    /// Usage starts recorded as **absent** rather than as a zeroed `Exact`: a
    /// stream that never carries `usageMetadata` reported nothing, and usage is
    /// never invented. The first `usageMetadata` frame replaces the whole tally,
    /// completeness included.
    fn fresh_state() -> DialectState {
        let mut state = DialectState::new();
        state.usage.completeness = UsageCompleteness::Absent;
        state
    }

    /// One decoded function call as a canonical block.
    fn tool_block(partial: &PartialToolCall) -> Result<CanonicalBlock, FrameDecodeError> {
        let id = partial.id.clone().ok_or(FrameDecodeError::MalformedField {
            field: "functionCall.id",
        })?;
        let name = partial
            .name
            .as_deref()
            .ok_or(FrameDecodeError::MalformedField {
                field: "functionCall.name",
            })?;
        let name = ToolName::parse(name).map_err(|_| FrameDecodeError::MalformedField {
            field: "functionCall.name",
        })?;
        let input = CanonicalJson::parse(&partial.arguments)
            .map_err(|_| FrameDecodeError::ToolArgumentsNotJson { call: id.clone() })?;
        Ok(CanonicalBlock::ToolUse { id, name, input })
    }
}

// ---------------------------------------------------------------------------
// failure classification
// ---------------------------------------------------------------------------

impl GoogleAdapter {
    /// The kind a documented `snake_case` code names.
    fn kind_for_snake_code(code: &str) -> Option<ProviderFailureKind> {
        SNAKE_CODES
            .iter()
            .find(|(name, _)| *name == code)
            .map(|(_, kind)| *kind)
    }

    /// The kind a classic `gRPC` status names.
    fn kind_for_grpc_status(status: &str) -> Option<ProviderFailureKind> {
        GRPC_STATUSES
            .iter()
            .find(|(name, _)| *name == status)
            .map(|(_, kind)| *kind)
    }

    /// The kind an `HTTP` status alone implies, which is also the documented
    /// answer for any `snake_case` code outside the published set.
    const fn kind_for_status(status: u16) -> ProviderFailureKind {
        match status {
            400 | 413 => ProviderFailureKind::InvalidRequest,
            401 | 403 => ProviderFailureKind::Authentication,
            404 => ProviderFailureKind::ModelNotFound,
            429 => ProviderFailureKind::RateLimited,
            503 => ProviderFailureKind::Overloaded,
            504 => ProviderFailureKind::Timeout,
            _ => ProviderFailureKind::ServerError,
        }
    }

    /// `error.details[].retryDelay`, the only backpressure Gemini publishes.
    fn retry_info(error: Option<&Value>) -> RateLimitFeedback {
        let Some(details) = error.and_then(|value| value.get("details")) else {
            return RateLimitFeedback::none();
        };
        let Some(details) = details.as_array() else {
            return RateLimitFeedback::none();
        };
        for detail in details {
            let Some(delay) = detail.get("retryDelay").and_then(Value::as_str) else {
                continue;
            };
            let Some(after) = Self::parse_retry_delay(delay) else {
                continue;
            };
            return RateLimitFeedback {
                retry_after: Some(after),
                source: RateLimitSource::ErrorBody,
                ..RateLimitFeedback::none()
            };
        }
        RateLimitFeedback::none()
    }

    /// A protobuf `Duration` rendering such as `30s` or `1.500s`.
    fn parse_retry_delay(text: &str) -> Option<core::time::Duration> {
        let digits = text.strip_suffix('s')?;
        let (whole, fraction) = digits.split_once('.').unwrap_or((digits, ""));
        let seconds: u64 = whole.parse().ok()?;
        if fraction.len() > 9 || !fraction.bytes().all(|byte| byte.is_ascii_digit()) {
            return None;
        }
        let mut nanos: u32 = 0;
        for index in 0..9usize {
            let digit = fraction
                .as_bytes()
                .get(index)
                .map_or(0, |byte| u32::from(*byte - b'0'));
            nanos = nanos * 10 + digit;
        }
        Some(core::time::Duration::new(seconds, nanos))
    }
}

// ---------------------------------------------------------------------------
// the adapter
// ---------------------------------------------------------------------------

impl ProviderAdapter for GoogleAdapter {
    fn provider(&self) -> ProviderId {
        ProviderId::Google
    }

    fn source_digest(&self) -> AdapterSourceDigest {
        AdapterSourceDigest(Blake3Digest::of(DIALECT_TAG.as_bytes()))
    }

    fn build_request(
        &self,
        model: &QualifiedModel,
        request: &CanonicalModelRequest,
    ) -> Result<WireRequest, RequestBuildError> {
        Self::encode(model.entry(), &GeminiRequestView::of(request))
    }

    fn new_state(&self, _model: &QualifiedModel) -> DialectState {
        Self::fresh_state()
    }

    fn decode(
        &self,
        state: &mut DialectState,
        event: &SseEvent<'_>,
        budget: &StreamBudget,
    ) -> Result<FrameOutcome, FrameDecodeError> {
        if let Some(name) = event.name
            && name != "message"
        {
            return Err(FrameDecodeError::UnknownEvent {
                event: BoundedString::truncating(name),
            });
        }
        // Gemini has no `[DONE]` sentinel, so one arriving means this is not the
        // dialect the entry pinned.
        if event.is_done_sentinel() {
            return Err(FrameDecodeError::UnknownEvent {
                event: BoundedString::truncating("[DONE]"),
            });
        }

        let payload = event.data_str().map_err(|_| FrameDecodeError::NotJson)?;
        let parsed: Value = serde_json::from_str(payload).map_err(|_| FrameDecodeError::NotJson)?;
        let Value::Object(frame) = parsed else {
            return Err(FrameDecodeError::NotJson);
        };

        state
            .ledger
            .charge_response(budget, u64::try_from(event.data.len()).unwrap_or(u64::MAX))?;
        state.ledger.count_frame();

        Self::check_frame_keys(&frame)?;
        Self::read_response_id(state, &frame)?;
        if let Some(usage) = frame.get("usageMetadata") {
            state.usage = Self::normalized_usage(usage)?;
        }
        if let Some(blocked) = Self::prompt_block(&frame)? {
            state.response_started = true;
            return Ok(FrameOutcome::Failed(Box::new(blocked)));
        }

        // Only `candidates` or `promptFeedback` proves the provider is
        // generating; a usage-only frame is not a first validated frame.
        let generating = frame.contains_key("candidates") || frame.contains_key("promptFeedback");
        if let Some(candidates) = frame.get("candidates")
            && let Some(outcome) = Self::read_candidate(state, candidates, budget)?
        {
            state.response_started = true;
            return Ok(outcome);
        }
        if generating {
            return Ok(state.mark_started());
        }
        Ok(FrameOutcome::Ignored)
    }

    fn finish(&self, state: DialectState) -> Result<SealedResponse, FrameDecodeError> {
        // There is no sentinel, so a terminal `finishReason` is the whole of the
        // completion proof: a stream that stops without one is a failure, never
        // a shorter success.
        if !state.terminal {
            return Err(FrameDecodeError::OutOfOrder {
                reason: "the stream ended without a terminal finishReason",
            });
        }
        let Some(token) = state.finish_token.as_deref() else {
            return Err(FrameDecodeError::OutOfOrder {
                reason: "a terminal frame carried no finishReason",
            });
        };

        let mut blocks = Vec::new();
        if let Some(reasoning) = Self::reasoning_block(&state)? {
            blocks.push(reasoning);
        }
        if let Some(text) = state.open_text.get(&TEXT_BLOCK) {
            blocks.push(CanonicalBlock::Text {
                text: BoundedString::new(text.as_str()).map_err(|_| {
                    FrameDecodeError::Budget(BudgetOverrun::Text {
                        limit: u64::try_from(TEXT_MAX).unwrap_or(u64::MAX),
                    })
                })?,
                annotations: Vec::new(),
            });
        }
        let mut calls = 0usize;
        for partial in state.open_tools.values() {
            blocks.push(Self::tool_block(partial)?);
            calls += 1;
        }
        if blocks.is_empty() {
            return Err(FrameDecodeError::OutOfOrder {
                reason: "the stream ended with no content at all",
            });
        }

        let stop_reason = if token == "MAX_TOKENS" {
            StopReason::MaxOutputTokens
        } else if calls > 0 {
            StopReason::ToolUse
        } else {
            StopReason::EndTurn
        };
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
        _headers: &HeaderView<'_>,
        body: &BoundedBody,
    ) -> ProviderFailure {
        let document = body.as_json();
        let error = document.as_ref().and_then(|value| value.get("error"));
        let message = error
            .and_then(|value| value.get("message"))
            .and_then(Value::as_str);
        // The current shape carries a snake_case string code; the classic gRPC
        // shape carries a numeric code and a `status` token. Both are parsed.
        let snake = error
            .and_then(|value| value.get("code"))
            .and_then(Value::as_str);
        let grpc = error
            .and_then(|value| value.get("status"))
            .and_then(Value::as_str);

        let kind = snake
            .and_then(Self::kind_for_snake_code)
            .or_else(|| grpc.and_then(Self::kind_for_grpc_status))
            .unwrap_or_else(|| Self::kind_for_status(status));

        let rendered = match message {
            Some(text) => redact::<512>(text, &[]),
            None if body.is_truncated() => {
                BoundedString::truncating("the provider error body exceeded its bound")
            }
            None => BoundedString::truncating("the provider published no error message"),
        };
        let mut detail = RedactedDetail::new(kind, rendered).with_status(status);
        if let Some(code) = snake.or(grpc) {
            detail = detail.with_code(redact::<64>(code, &[]).as_str());
        }
        ProviderFailure {
            detail,
            rate_limit: Self::retry_info(error),
        }
    }

    fn rate_limit_feedback(&self, _headers: &HeaderView<'_>) -> RateLimitFeedback {
        // Gemini documents no `x-ratelimit-*` family and no `Retry-After`. The
        // absence is a positive record, and the only backpressure it does
        // publish — `retryDelay` inside an error body — reaches the caller on
        // `ProviderFailure::rate_limit` instead.
        RateLimitFeedback::none()
    }

    fn request_id(
        &self,
        _headers: &HeaderView<'_>,
        state: &DialectState,
    ) -> Option<ProviderRequestId> {
        // `responseId` lives in the body; Gemini publishes no id header.
        state.request_id.clone()
    }
}

#[cfg(test)]
mod tests {
    use aex_model_catalog::canonical::{
        CacheBreakpoint, CanonicalBlock, CanonicalMessage, CanonicalToolDef, ReasoningBlock,
        ReasoningBody, ReasoningEffort, ReasoningRequest, ReasoningToken, Role, StopReason,
        StructuredOutputRequest, SystemBlock, TEXT_MAX, ToolChoice, ToolResultPart,
        UsageCompleteness, UsageField, UsageFieldSet,
    };
    use aex_model_catalog::document::{
        CacheMode, CachePolicy, Capability, CapabilitySet, EndpointPin, ModelEntry, NamePattern,
        ReasoningEncoding, ReasoningMode, ReasoningPolicy, ReasoningReplay, SamplingSupport,
        SchemaEncoding, StructuredOutputPolicy, ToolArgumentEncoding, ToolEncoding, ToolPolicy,
    };
    use aex_model_catalog::fixture;
    use aex_model_catalog::primitives::{BoundedString, ToolCallId, ToolName};
    use aex_wire::CanonicalJson;
    use aex_wire::provider::ProviderId;

    use super::{
        ALT_PARAMETER, ALT_SSE, DIALECT_TAG, FINISH_REASONS, FinishClass, GeminiRequestView,
        GoogleAdapter,
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
    // fixtures
    // -----------------------------------------------------------------------

    /// Everything the launch `google` entry is expected to declare.
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

    /// A `google` entry shaped as plan 08 §5.6 records the dialect.
    fn entry() -> ModelEntry {
        let mut entry = fixture::entry(ProviderId::Google, "gemini-3-pro", capabilities());
        entry.tool_policy = ToolPolicy {
            encoding: ToolEncoding::GeminiFunctionDeclarations,
            arguments: ToolArgumentEncoding::JsonObject,
            requires_stream_opt_in: false,
            max_name_bytes: 64,
            name_pattern: NamePattern::GeminiFunctionName,
        };
        entry.reasoning = ReasoningPolicy {
            mode: ReasoningMode::Optional,
            encoding: ReasoningEncoding::GeminiThinkingConfig,
            replay: ReasoningReplay::RequiredWithToolCalls,
            excludes_sampling: false,
        };
        entry.structured_output = StructuredOutputPolicy::JsonSchema {
            encoding: SchemaEncoding::GeminiGenerationConfigFlat,
            strict_default: true,
        };
        entry.cache_policy = CachePolicy {
            mode: CacheMode::Implicit,
            max_breakpoints: 0,
        };
        entry
    }

    /// Owned storage for a [`GeminiRequestView`], which borrows everything.
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
    }

    impl Draft {
        fn new() -> Self {
            Self {
                system: Vec::new(),
                messages: vec![user("Hello")],
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
            }
        }

        fn view(&self) -> GeminiRequestView<'_> {
            GeminiRequestView {
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
    }

    fn build(entry: &ModelEntry, draft: &Draft) -> Result<WireRequest, RequestBuildError> {
        GoogleAdapter::encode(entry, &draft.view())
    }

    fn body_of(entry: &ModelEntry, draft: &Draft) -> String {
        let request = build(entry, draft).expect("the draft builds");
        String::from_utf8(request.body.to_vec()).expect("a JSON body is UTF-8")
    }

    fn text(value: &str) -> BoundedString<TEXT_MAX> {
        BoundedString::new(value).expect("fixture text fits")
    }

    fn user(message: &str) -> CanonicalMessage {
        CanonicalMessage {
            role: Role::User,
            blocks: vec![CanonicalBlock::Text {
                text: text(message),
                annotations: Vec::new(),
            }],
        }
    }

    fn turn(role: Role, blocks: Vec<CanonicalBlock>) -> CanonicalMessage {
        CanonicalMessage { role, blocks }
    }

    fn json(value: &str) -> CanonicalJson {
        CanonicalJson::parse(value).expect("fixture JSON parses")
    }

    fn tool_name(value: &str) -> ToolName {
        ToolName::parse(value).expect("fixture tool name parses")
    }

    fn call_id(value: &str) -> ToolCallId {
        ToolCallId::new(value).expect("fixture call id fits")
    }

    fn weather_tool() -> CanonicalToolDef {
        CanonicalToolDef {
            name: tool_name("get_weather"),
            description: BoundedString::new("Look up the weather.").expect("fits"),
            input_schema: json(r#"{"type":"object","properties":{"city":{"type":"string"}}}"#),
            strict: false,
        }
    }

    fn google_signature(material: &str) -> ReasoningToken {
        ReasoningToken {
            provenance: ProviderId::Google,
            bytes: bytes::Bytes::copy_from_slice(material.as_bytes()),
        }
    }

    // -----------------------------------------------------------------------
    // identity, path and the two credential facts
    // -----------------------------------------------------------------------

    #[test]
    fn the_adapter_speaks_for_google_on_the_pinned_origin() {
        let adapter = GoogleAdapter;
        assert_eq!(adapter.provider(), ProviderId::Google);
        let request = build(&entry(), &Draft::new()).expect("builds");
        assert_eq!(request.endpoint, EndpointPin::GeminiV1Beta);
        assert_eq!(request.accept, Accept::TextEventStream);
        assert_eq!(
            request.endpoint.origin(),
            "https://generativelanguage.googleapis.com"
        );
    }

    #[test]
    fn the_source_digest_is_a_compiled_dialect_identity() {
        let adapter = GoogleAdapter;
        assert_eq!(
            adapter.source_digest(),
            aex_model_catalog::document::AdapterSourceDigest(
                aex_model_catalog::primitives::Blake3Digest::of(DIALECT_TAG.as_bytes())
            )
        );
    }

    #[test]
    fn the_path_pins_stream_generate_content_for_the_catalog_model() {
        let request = build(&entry(), &Draft::new()).expect("builds");
        assert_eq!(
            request.path.as_str(),
            "/v1beta/models/gemini-3-pro:streamGenerateContent"
        );
        let url = request.url().expect("assembles");
        assert_eq!(
            url.as_str(),
            "https://generativelanguage.googleapis.com\
             /v1beta/models/gemini-3-pro:streamGenerateContent?alt=sse"
        );
    }

    #[test]
    fn a_model_slug_that_could_escape_the_pinned_path_is_refused() {
        let mut entry = entry();
        entry.model = BoundedString::new("../../v1beta/models/other").expect("fits");
        assert_eq!(
            build(&entry, &Draft::new()),
            Err(RequestBuildError::Encoding {
                reason: "the model slug is not a bare Gemini model id"
            })
        );
    }

    #[test]
    fn the_alt_sse_query_parameter_is_always_present() {
        // Without it Gemini answers with a JSON array rather than SSE, so it is
        // compiled in rather than optional (D-14).
        let entry = entry();
        let mut tooled = Draft::new();
        tooled.tools = vec![weather_tool()];
        let mut reasoning = Draft::new();
        reasoning.reasoning = ReasoningRequest::Enabled {
            budget_tokens: None,
            effort: Some(ReasoningEffort::High),
        };
        let mut structured = Draft::new();
        structured.structured_output = Some(StructuredOutputRequest::JsonObject);

        for draft in [Draft::new(), tooled, reasoning, structured] {
            let request = build(&entry, &draft).expect("builds");
            assert_eq!(request.query.len(), 1);
            assert_eq!(request.query[0].0, ALT_PARAMETER);
            assert_eq!(request.query[0].1.as_str(), ALT_SSE);
            assert!(
                request
                    .url()
                    .expect("assembles")
                    .as_str()
                    .contains("alt=sse"),
                "the assembled URL dropped ?alt=sse"
            );
        }
    }

    #[test]
    fn no_query_parameter_named_key_is_ever_constructed() {
        // The `?key=` credential form is forbidden: it would put a customer key
        // in a URL that reaches proxies, access logs and error strings (D-15).
        let entry = entry();
        let mut tooled = Draft::new();
        tooled.tools = vec![weather_tool()];
        tooled.tool_choice = ToolChoice::Required;
        let mut everything = Draft::new();
        everything.system = vec![SystemBlock {
            text: text("Be brief."),
            cacheable: false,
        }];
        everything.tools = vec![weather_tool()];
        everything.temperature_milli = Some(700);
        everything.top_p_milli = Some(900);
        everything.stop_sequences = vec![BoundedString::new("STOP").expect("fits")];
        everything.reasoning = ReasoningRequest::Enabled {
            budget_tokens: None,
            effort: Some(ReasoningEffort::Max),
        };
        everything.structured_output = Some(StructuredOutputRequest::JsonObject);

        for draft in [Draft::new(), tooled, everything] {
            let request = build(&entry, &draft).expect("builds");
            assert!(
                request.query.iter().all(|(name, _)| *name != "key"),
                "a query parameter named `key` was constructed"
            );
            let url = request.url().expect("assembles");
            assert!(!url.as_str().contains("key="), "{url}");
            assert_eq!(request.auth, AuthScheme::GoogleApiKeyHeader);
            assert_eq!(request.auth.header_name(), "x-goog-api-key");
            // The request type has no field able to hold a credential, so the
            // rendering has nothing to leak.
            assert!(!format!("{request:?}").contains("AIza"));
        }
    }

    // -----------------------------------------------------------------------
    // request goldens
    // -----------------------------------------------------------------------

    #[test]
    fn a_text_request_matches_its_golden_body() {
        assert_eq!(
            body_of(&entry(), &Draft::new()),
            r#"{"contents":[{"parts":[{"text":"Hello"}],"role":"user"}],"generationConfig":{"maxOutputTokens":1024}}"#
        );
    }

    #[test]
    fn a_tool_request_matches_its_golden_body() {
        let mut draft = Draft::new();
        draft.system = vec![SystemBlock {
            text: text("Be brief."),
            cacheable: false,
        }];
        draft.messages = vec![user("Weather?")];
        draft.tools = vec![weather_tool()];
        draft.tool_choice = ToolChoice::Named {
            name: tool_name("get_weather"),
        };
        draft.max_output_tokens = 256;
        draft.temperature_milli = Some(700);
        crate::golden::assert_json_eq(
            &body_of(&entry(), &draft),
            concat!(
                r#"{"contents":[{"parts":[{"text":"Weather?"}],"role":"user"}],"#,
                r#""generationConfig":{"maxOutputTokens":256,"temperature":0.7},"#,
                r#""systemInstruction":{"parts":[{"text":"Be brief."}]},"#,
                r#""toolConfig":{"functionCallingConfig":{"allowedFunctionNames":["get_weather"],"mode":"ANY"}},"#,
                r#""tools":[{"functionDeclarations":[{"description":"Look up the weather.","#,
                r#""name":"get_weather","parameters":{"properties":{"city":{"type":"string"}},"type":"object"}}]}]}"#
            ),
        );
    }

    fn schema_draft() -> Draft {
        let mut draft = Draft::new();
        draft.messages = vec![user("Give me JSON.")];
        draft.max_output_tokens = 512;
        draft.structured_output = Some(StructuredOutputRequest::JsonSchema {
            name: aex_wire::ResourceName::parse("answer").expect("name"),
            schema: json(r#"{"type":"object"}"#),
            strict: true,
        });
        draft
    }

    #[test]
    fn the_flat_schema_encoding_matches_its_golden_body() {
        let mut entry = entry();
        entry.structured_output = StructuredOutputPolicy::JsonSchema {
            encoding: SchemaEncoding::GeminiGenerationConfigFlat,
            strict_default: true,
        };
        assert_eq!(
            body_of(&entry, &schema_draft()),
            concat!(
                r#"{"contents":[{"parts":[{"text":"Give me JSON."}],"role":"user"}],"#,
                r#""generationConfig":{"maxOutputTokens":512,"responseMimeType":"application/json","#,
                r#""responseSchema":{"type":"object"}}}"#
            )
        );
    }

    #[test]
    fn the_nested_schema_encoding_matches_its_golden_body() {
        let mut entry = entry();
        entry.structured_output = StructuredOutputPolicy::JsonSchema {
            encoding: SchemaEncoding::GeminiResponseFormatNested,
            strict_default: true,
        };
        assert_eq!(
            body_of(&entry, &schema_draft()),
            concat!(
                r#"{"contents":[{"parts":[{"text":"Give me JSON."}],"role":"user"}],"#,
                r#""generationConfig":{"maxOutputTokens":512,"responseFormat":{"text":"#,
                r#"{"mimeType":"application/json","schema":{"type":"object"}}}}}"#
            )
        );
    }

    fn replay_draft() -> Draft {
        let mut draft = Draft::new();
        draft.messages = vec![
            user("Weather?"),
            turn(
                Role::Assistant,
                vec![
                    CanonicalBlock::Reasoning(ReasoningBlock {
                        body: ReasoningBody::Redacted,
                        token: Some(google_signature("sig-abc")),
                    }),
                    CanonicalBlock::ToolUse {
                        id: call_id("call-1"),
                        name: tool_name("get_weather"),
                        input: json(r#"{"city":"Berlin"}"#),
                    },
                ],
            ),
            turn(
                Role::User,
                vec![CanonicalBlock::ToolResult {
                    call: call_id("call-1"),
                    content: vec![ToolResultPart::Json {
                        value: json(r#"{"tempC":11}"#),
                    }],
                    is_error: false,
                }],
            ),
        ];
        draft.tools = vec![weather_tool()];
        draft.max_output_tokens = 2048;
        draft.reasoning = ReasoningRequest::Enabled {
            budget_tokens: None,
            effort: Some(ReasoningEffort::High),
        };
        draft
    }

    #[test]
    fn a_reasoning_and_replay_request_matches_its_golden_body() {
        assert_eq!(
            body_of(&entry(), &replay_draft()),
            concat!(
                r#"{"contents":[{"parts":[{"text":"Weather?"}],"role":"user"},"#,
                r#"{"parts":[{"functionCall":{"args":{"city":"Berlin"},"id":"call-1","#,
                r#""name":"get_weather"},"thoughtSignature":"sig-abc"}],"role":"model"},"#,
                r#"{"parts":[{"functionResponse":{"id":"call-1","name":"get_weather","#,
                r#""response":{"output":{"tempC":11}}}}],"role":"user"}],"#,
                r#""generationConfig":{"maxOutputTokens":2048,"thinkingConfig":"#,
                r#"{"includeThoughts":true,"thinkingLevel":"high"}},"#,
                r#""toolConfig":{"functionCallingConfig":{"mode":"AUTO"}},"#,
                r#""tools":[{"functionDeclarations":[{"description":"Look up the weather.","#,
                r#""name":"get_weather","parameters":{"properties":{"city":{"type":"string"}},"type":"object"}}]}]}"#
            )
        );
    }

    #[test]
    fn a_thought_signature_is_echoed_on_the_first_function_call_only() {
        let mut draft = replay_draft();
        draft.messages[1] = turn(
            Role::Assistant,
            vec![
                CanonicalBlock::Reasoning(ReasoningBlock {
                    body: ReasoningBody::Redacted,
                    token: Some(google_signature("sig-abc")),
                }),
                CanonicalBlock::ToolUse {
                    id: call_id("call-1"),
                    name: tool_name("get_weather"),
                    input: json(r#"{"city":"Berlin"}"#),
                },
                CanonicalBlock::ToolUse {
                    id: call_id("call-2"),
                    name: tool_name("get_weather"),
                    input: json(r#"{"city":"Paris"}"#),
                },
            ],
        );
        draft.messages.truncate(2);
        let rendered = body_of(&entry(), &draft);
        assert_eq!(
            rendered.matches("thoughtSignature").count(),
            1,
            "in parallel calls only the first part carries one: {rendered}"
        );
        assert!(
            rendered.contains(r#""id":"call-2","name":"get_weather"}}"#),
            "{rendered}"
        );
    }

    #[test]
    fn a_required_thought_signature_that_is_absent_is_rejected_before_dispatch() {
        // Gemini 3 answers 400 when the material is not echoed on the first
        // function call of a step, so the omission fails before a socket exists.
        let mut draft = replay_draft();
        draft.messages.truncate(2);
        draft.messages[1] = turn(
            Role::Assistant,
            vec![
                CanonicalBlock::Reasoning(ReasoningBlock {
                    body: ReasoningBody::Redacted,
                    token: None,
                }),
                CanonicalBlock::ToolUse {
                    id: call_id("call-1"),
                    name: tool_name("get_weather"),
                    input: json(r#"{"city":"Berlin"}"#),
                },
            ],
        );
        assert_eq!(
            build(&entry(), &draft),
            Err(RequestBuildError::ReasoningTokenRequired)
        );
    }

    #[test]
    fn reasoning_material_from_another_provider_is_never_replayed_here() {
        let mut draft = replay_draft();
        draft.messages.truncate(2);
        draft.messages[1] = turn(
            Role::Assistant,
            vec![CanonicalBlock::Reasoning(ReasoningBlock {
                body: ReasoningBody::Redacted,
                token: Some(ReasoningToken {
                    provenance: ProviderId::Anthropic,
                    bytes: bytes::Bytes::from_static(b"signature"),
                }),
            })],
        );
        assert_eq!(
            build(&entry(), &draft),
            Err(RequestBuildError::ReasoningProvenanceMismatch {
                expected: ProviderId::Google,
                found: ProviderId::Anthropic,
            })
        );
    }

    #[test]
    fn a_tool_result_answering_an_unmade_call_is_refused() {
        let mut draft = Draft::new();
        draft.messages = vec![turn(
            Role::User,
            vec![CanonicalBlock::ToolResult {
                call: call_id("call-9"),
                content: Vec::new(),
                is_error: true,
            }],
        )];
        assert_eq!(
            build(&entry(), &draft),
            Err(RequestBuildError::Encoding {
                reason: "a tool result answers a call this conversation never made"
            })
        );
    }

    // -----------------------------------------------------------------------
    // capability gates, every one firing before a socket exists
    // -----------------------------------------------------------------------

    #[test]
    fn a_tool_choice_the_entry_does_not_declare_is_refused() {
        let mut entry = entry();
        entry.capabilities = CapabilitySet::from_slice(&[Capability::Tools]);
        let mut draft = Draft::new();
        draft.tools = vec![weather_tool()];
        draft.tool_choice = ToolChoice::Required;
        assert_eq!(
            build(&entry, &draft),
            Err(RequestBuildError::CapabilityUnavailable {
                capability: Capability::ToolChoiceRequired
            })
        );
    }

    #[test]
    fn a_named_tool_choice_without_tools_is_refused() {
        let mut draft = Draft::new();
        draft.tool_choice = ToolChoice::Named {
            name: tool_name("get_weather"),
        };
        assert!(matches!(
            build(&entry(), &draft),
            Err(RequestBuildError::ToolChoiceUnsupported { .. })
        ));
    }

    #[test]
    fn a_tool_name_outside_the_gemini_grammar_is_refused() {
        let mut draft = Draft::new();
        let mut tool = weather_tool();
        tool.name = tool_name("9lives");
        draft.tools = vec![tool];
        assert!(matches!(
            build(&entry(), &draft),
            Err(RequestBuildError::ToolNameInvalid { .. })
        ));
    }

    #[test]
    fn more_tools_than_the_pair_accepts_is_refused() {
        let mut entry = entry();
        entry.limits.max_tools = 1;
        let mut draft = Draft::new();
        let mut second = weather_tool();
        second.name = tool_name("get_time");
        draft.tools = vec![weather_tool(), second];
        assert_eq!(
            build(&entry, &draft),
            Err(RequestBuildError::ToolLimit { max: 1 })
        );
    }

    #[test]
    fn forbidding_parallel_tool_calls_is_refused_rather_than_dropped() {
        let mut draft = Draft::new();
        draft.tools = vec![weather_tool()];
        draft.parallel_tools = false;
        assert_eq!(
            build(&entry(), &draft),
            Err(RequestBuildError::Encoding {
                reason: "google has no field that forbids parallel function calls"
            })
        );
    }

    #[test]
    fn an_over_long_stop_sequence_list_is_refused() {
        let mut entry = entry();
        entry.limits.max_stop_sequences = 2;
        let mut draft = Draft::new();
        draft.stop_sequences = (0..3)
            .map(|index| BoundedString::new(format!("S{index}")).expect("fits"))
            .collect();
        assert_eq!(
            build(&entry, &draft),
            Err(RequestBuildError::StopSequenceLimit { max: 2 })
        );
    }

    #[test]
    fn an_output_ceiling_outside_the_pairs_range_is_refused() {
        let entry = entry();
        let mut draft = Draft::new();
        draft.max_output_tokens = entry.limits.max_output_tokens + 1;
        assert_eq!(
            build(&entry, &draft),
            Err(RequestBuildError::OutputTokensOutOfRange {
                min: entry.limits.min_output_tokens,
                max: entry.limits.max_output_tokens,
            })
        );
    }

    #[test]
    fn a_sampling_field_the_pair_does_not_honour_is_refused() {
        let mut entry = entry();
        entry.sampling = SamplingSupport::TemperatureOnly;
        let mut draft = Draft::new();
        draft.top_p_milli = Some(900);
        assert_eq!(
            build(&entry, &draft),
            Err(RequestBuildError::SamplingUnsupported { field: "top_p" })
        );

        let mut out_of_range = Draft::new();
        out_of_range.temperature_milli = Some(u16::MAX);
        assert_eq!(
            build(&entry, &out_of_range),
            Err(RequestBuildError::SamplingUnsupported {
                field: "temperature"
            })
        );
    }

    #[test]
    fn a_pair_whose_reasoning_excludes_sampling_refuses_both_together() {
        let mut entry = entry();
        entry.reasoning.excludes_sampling = true;
        let mut draft = Draft::new();
        draft.temperature_milli = Some(500);
        draft.reasoning = ReasoningRequest::Enabled {
            budget_tokens: None,
            effort: Some(ReasoningEffort::Low),
        };
        assert_eq!(
            build(&entry, &draft),
            Err(RequestBuildError::SamplingWithReasoning {
                field: "temperature"
            })
        );
    }

    #[test]
    fn an_explicit_cache_breakpoint_is_refused_because_gemini_caching_is_implicit() {
        let mut draft = Draft::new();
        draft.cache_breakpoints = vec![CacheBreakpoint::AfterSystem];
        assert_eq!(
            build(&entry(), &draft),
            Err(RequestBuildError::CapabilityUnavailable {
                capability: Capability::PromptCacheExplicit
            })
        );
    }

    #[test]
    fn a_thinking_level_and_a_thinking_budget_cannot_both_be_sent() {
        let mut draft = Draft::new();
        draft.reasoning = ReasoningRequest::Enabled {
            budget_tokens: Some(2048),
            effort: Some(ReasoningEffort::High),
        };
        assert_eq!(
            build(&entry(), &draft),
            Err(RequestBuildError::Encoding {
                reason: "google takes either thinkingLevel or thinkingBudget, never both"
            })
        );
    }

    #[test]
    fn a_reasoning_budget_outside_the_pairs_range_is_refused() {
        let entry = entry();
        let mut draft = Draft::new();
        draft.reasoning = ReasoningRequest::Enabled {
            budget_tokens: Some(1),
            effort: None,
        };
        assert_eq!(
            build(&entry, &draft),
            Err(RequestBuildError::ReasoningBudgetOutOfRange {
                min: entry.limits.min_reasoning_tokens.unwrap_or(0),
                max: entry.limits.max_reasoning_tokens.unwrap_or(u32::MAX),
            })
        );
    }

    #[test]
    fn a_pair_whose_reasoning_is_always_on_refuses_to_disable_it() {
        let mut entry = entry();
        entry.reasoning.mode = ReasoningMode::AlwaysOn;
        let mut draft = Draft::new();
        draft.reasoning = ReasoningRequest::Disabled;
        assert_eq!(
            build(&entry, &draft),
            Err(RequestBuildError::Encoding {
                reason: "this pair cannot disable reasoning"
            })
        );
    }

    #[test]
    fn disabling_reasoning_sends_a_zero_budget_rather_than_nothing() {
        let mut draft = Draft::new();
        draft.reasoning = ReasoningRequest::Disabled;
        assert!(
            body_of(&entry(), &draft)
                .contains(r#""thinkingConfig":{"includeThoughts":false,"thinkingBudget":0}"#)
        );
    }

    #[test]
    fn a_non_strict_schema_request_is_refused_because_gemini_always_constrains() {
        let mut draft = schema_draft();
        draft.structured_output = Some(StructuredOutputRequest::JsonSchema {
            name: aex_wire::ResourceName::parse("answer").expect("name"),
            schema: json(r#"{"type":"object"}"#),
            strict: false,
        });
        assert!(matches!(
            build(&entry(), &draft),
            Err(RequestBuildError::StructuredOutputUnsupported { .. })
        ));
    }

    #[test]
    fn a_schema_request_against_a_json_object_only_pair_is_refused() {
        let mut entry = entry();
        entry.structured_output = StructuredOutputPolicy::JsonObjectOnly;
        assert!(matches!(
            build(&entry, &schema_draft()),
            Err(RequestBuildError::StructuredOutputUnsupported { .. })
        ));
    }

    #[test]
    fn a_body_over_the_pairs_bound_is_refused() {
        let mut entry = entry();
        entry.limits.request_body_max_bytes = 32;
        assert_eq!(
            build(&entry, &Draft::new()),
            Err(RequestBuildError::BodyTooLarge { limit: 32 })
        );
    }

    // -----------------------------------------------------------------------
    // stream decoding
    // -----------------------------------------------------------------------

    fn frame(payload: &str) -> SseEvent<'_> {
        SseEvent {
            name: None,
            data: payload.as_bytes(),
            id: None,
        }
    }

    /// Feeds a script, requiring every frame to decode.
    fn run(script: &[&str]) -> (DialectState, Vec<FrameOutcome>) {
        let adapter = GoogleAdapter;
        let budget = StreamBudget::default();
        let mut state = GoogleAdapter::fresh_state();
        let mut outcomes = Vec::with_capacity(script.len());
        for payload in script {
            outcomes.push(
                adapter
                    .decode(&mut state, &frame(payload), &budget)
                    .unwrap_or_else(|error| panic!("frame `{payload}` failed: {error}")),
            );
        }
        (state, outcomes)
    }

    /// Feeds a script, requiring the last frame to fail.
    fn run_err(script: &[&str]) -> FrameDecodeError {
        let adapter = GoogleAdapter;
        let budget = StreamBudget::default();
        let mut state = GoogleAdapter::fresh_state();
        let mut last = None;
        for payload in script {
            last = Some(adapter.decode(&mut state, &frame(payload), &budget));
        }
        match last {
            Some(Err(error)) => error,
            _ => panic!("the script decoded without the expected failure"),
        }
    }

    const HELLO: &str = r#"{"candidates":[{"content":{"parts":[{"text":"Hel"}],"role":"model"}}],"responseId":"resp-1"}"#;
    const WORLD: &str = r#"{"candidates":[{"content":{"parts":[{"text":"lo"}],"role":"model"}}]}"#;
    const STOP: &str = r#"{"candidates":[{"content":{"parts":[]},"finishReason":"STOP"}],"usageMetadata":{"promptTokenCount":9,"candidatesTokenCount":2,"totalTokenCount":11}}"#;

    #[test]
    fn the_first_frame_with_candidates_starts_the_response_exactly_once() {
        let (state, outcomes) = run(&[HELLO, WORLD, STOP]);
        assert_eq!(
            outcomes,
            vec![
                FrameOutcome::ResponseStarted,
                FrameOutcome::Progress,
                FrameOutcome::Terminal
            ]
        );
        assert!(state.response_started);
        assert!(state.terminal);
    }

    #[test]
    fn a_frame_carrying_only_usage_is_not_proof_that_the_provider_is_generating() {
        let usage = r#"{"usageMetadata":{"promptTokenCount":9,"candidatesTokenCount":0,"totalTokenCount":9}}"#;
        let (state, outcomes) = run(&[usage]);
        assert_eq!(outcomes, vec![FrameOutcome::Ignored]);
        assert!(!state.response_started);
        assert_eq!(state.usage.input_tokens, 9);
    }

    #[test]
    fn text_accumulates_across_frames_into_one_block() {
        let (state, _) = run(&[HELLO, WORLD, STOP]);
        let sealed = GoogleAdapter.finish(state).expect("seals");
        assert_eq!(sealed.stop_reason, StopReason::EndTurn);
        assert_eq!(
            sealed.blocks,
            vec![CanonicalBlock::Text {
                text: text("Hello"),
                annotations: Vec::new(),
            }]
        );
        assert_eq!(
            sealed
                .provider_request_id
                .as_ref()
                .map(BoundedString::as_str),
            Some("resp-1")
        );
    }

    #[test]
    fn a_thought_part_accumulates_as_reasoning_rather_than_as_text() {
        let thinking = r#"{"candidates":[{"content":{"parts":[{"text":"weigh","thought":true},{"text":"answer"}]}}]}"#;
        let (state, _) = run(&[thinking, STOP]);
        let sealed = GoogleAdapter.finish(state).expect("seals");
        assert_eq!(
            sealed.blocks,
            vec![
                CanonicalBlock::Reasoning(ReasoningBlock {
                    body: ReasoningBody::Text {
                        text: BoundedString::new("weigh").expect("fits")
                    },
                    token: None,
                }),
                CanonicalBlock::Text {
                    text: text("answer"),
                    annotations: Vec::new(),
                }
            ]
        );
    }

    #[test]
    fn a_function_call_arrives_whole_and_keeps_the_id_gemini_minted() {
        let call = r#"{"candidates":[{"content":{"parts":[{"functionCall":{"name":"get_weather","args":{"city":"Berlin"},"id":"fc-1"}}]}}]}"#;
        let terminal = r#"{"candidates":[{"content":{"parts":[]},"finishReason":"STOP"}]}"#;
        let (state, _) = run(&[call, terminal]);
        let sealed = GoogleAdapter.finish(state).expect("seals");
        assert_eq!(sealed.stop_reason, StopReason::ToolUse);
        assert_eq!(
            sealed.blocks,
            vec![CanonicalBlock::ToolUse {
                id: call_id("fc-1"),
                name: tool_name("get_weather"),
                input: json(r#"{"city":"Berlin"}"#),
            }]
        );
    }

    #[test]
    fn a_function_call_without_an_id_is_a_protocol_violation() {
        let call = r#"{"candidates":[{"content":{"parts":[{"functionCall":{"name":"get_weather","args":{}}}]}}]}"#;
        assert_eq!(
            run_err(&[call]),
            FrameDecodeError::MalformedField {
                field: "functionCall.id"
            }
        );
    }

    #[test]
    fn a_thought_signature_is_captured_as_google_reasoning_material() {
        let call = concat!(
            r#"{"candidates":[{"content":{"parts":[{"functionCall":"#,
            r#"{"name":"get_weather","args":{},"id":"fc-1"},"thoughtSignature":"sig-abc"}]}}]}"#
        );
        let terminal = r#"{"candidates":[{"content":{"parts":[]},"finishReason":"STOP"}]}"#;
        let (state, _) = run(&[call, terminal]);
        let sealed = GoogleAdapter.finish(state).expect("seals");
        assert_eq!(
            sealed.blocks[0],
            CanonicalBlock::Reasoning(ReasoningBlock {
                // No thought text was requested, so the material is all there is.
                body: ReasoningBody::Redacted,
                token: Some(google_signature("sig-abc")),
            })
        );
        assert_eq!(sealed.stop_reason, StopReason::ToolUse);
    }

    #[test]
    fn only_the_first_thought_signature_of_a_step_is_kept() {
        let calls = concat!(
            r#"{"candidates":[{"content":{"parts":["#,
            r#"{"functionCall":{"name":"get_weather","args":{},"id":"fc-1"},"thoughtSignature":"first"},"#,
            r#"{"functionCall":{"name":"get_weather","args":{},"id":"fc-2"},"thoughtSignature":"second"}]}}]}"#
        );
        let terminal = r#"{"candidates":[{"content":{"parts":[]},"finishReason":"STOP"}]}"#;
        let (state, _) = run(&[calls, terminal]);
        let sealed = GoogleAdapter.finish(state).expect("seals");
        assert_eq!(
            sealed.blocks[0],
            CanonicalBlock::Reasoning(ReasoningBlock {
                body: ReasoningBody::Redacted,
                token: Some(google_signature("first")),
            })
        );
    }

    #[test]
    fn a_stream_that_ends_without_a_terminal_finish_reason_is_a_failure() {
        // There is no `[DONE]` sentinel, so a stream that simply stops is not a
        // shorter success.
        let (state, _) = run(&[HELLO, WORLD]);
        assert_eq!(
            GoogleAdapter.finish(state),
            Err(FrameDecodeError::OutOfOrder {
                reason: "the stream ended without a terminal finishReason"
            })
        );
    }

    #[test]
    fn a_terminal_stream_with_no_content_at_all_is_a_failure() {
        let empty = r#"{"candidates":[{"content":{"parts":[]},"finishReason":"STOP"}]}"#;
        let (state, _) = run(&[empty]);
        assert_eq!(
            GoogleAdapter.finish(state),
            Err(FrameDecodeError::OutOfOrder {
                reason: "the stream ended with no content at all"
            })
        );
    }

    #[test]
    fn the_done_sentinel_is_not_a_google_frame() {
        let adapter = GoogleAdapter;
        let budget = StreamBudget::default();
        let mut state = GoogleAdapter::fresh_state();
        let event = SseEvent {
            name: None,
            data: b"[DONE]",
            id: None,
        };
        assert!(matches!(
            adapter.decode(&mut state, &event, &budget),
            Err(FrameDecodeError::UnknownEvent { .. })
        ));
    }

    #[test]
    fn a_named_sse_event_is_not_a_google_frame() {
        let adapter = GoogleAdapter;
        let budget = StreamBudget::default();
        let mut state = GoogleAdapter::fresh_state();
        let event = SseEvent {
            name: Some("message_start"),
            data: HELLO.as_bytes(),
            id: None,
        };
        assert!(matches!(
            adapter.decode(&mut state, &event, &budget),
            Err(FrameDecodeError::UnknownEvent { .. })
        ));
    }

    #[test]
    fn an_unknown_top_level_frame_member_is_a_protocol_violation() {
        let error = run_err(&[r#"{"candidates":[],"interactionId":"abc"}"#]);
        assert!(matches!(
            error,
            FrameDecodeError::UnknownEvent { ref event } if event.as_str() == "interactionId"
        ));
    }

    #[test]
    fn a_provider_hosted_tool_part_is_a_protocol_violation() {
        // Those tools are never sent, so receiving one means the request was
        // altered in flight (D-25).
        for member in ["executableCode", "codeExecutionResult", "inlineData"] {
            let payload =
                format!(r#"{{"candidates":[{{"content":{{"parts":[{{"{member}":{{}}}}]}}}}]}}"#);
            assert!(
                matches!(
                    run_err(&[payload.as_str()]),
                    FrameDecodeError::UnknownEvent { ref event } if event.as_str() == member
                ),
                "{member} decoded instead of being refused"
            );
        }
    }

    #[test]
    fn a_frame_that_is_not_json_is_refused() {
        assert_eq!(run_err(&["not json at all"]), FrameDecodeError::NotJson);
    }

    #[test]
    fn more_than_one_candidate_is_a_protocol_violation() {
        let two = r#"{"candidates":[{"content":{"parts":[]}},{"content":{"parts":[]}}]}"#;
        assert_eq!(
            run_err(&[two]),
            FrameDecodeError::MalformedField {
                field: "candidates"
            }
        );
    }

    #[test]
    fn a_blocked_prompt_is_a_content_filter_failure() {
        let blocked = r#"{"promptFeedback":{"blockReason":"SAFETY"}}"#;
        let (_, outcomes) = run(&[blocked]);
        match &outcomes[0] {
            FrameOutcome::Failed(failure) => {
                assert_eq!(failure.kind(), ProviderFailureKind::ContentFiltered);
                assert_eq!(
                    failure
                        .detail
                        .provider_code
                        .as_ref()
                        .map(BoundedString::as_str),
                    Some("SAFETY")
                );
            }
            other => panic!("expected a content-filter failure, got {other:?}"),
        }
    }

    // -----------------------------------------------------------------------
    // the finish-reason table
    // -----------------------------------------------------------------------

    #[test]
    fn the_finish_reason_table_is_exhaustive_over_the_documented_enum() {
        let documented = [
            "STOP",
            "MAX_TOKENS",
            "SAFETY",
            "PROHIBITED_CONTENT",
            "BLOCKLIST",
            "SPII",
            "IMAGE_SAFETY",
            "RECITATION",
            "LANGUAGE",
            "OTHER",
            "FINISH_REASON_UNSPECIFIED",
            "MALFORMED_FUNCTION_CALL",
            "UNEXPECTED_TOOL_CALL",
            "TOO_MANY_TOOL_CALLS",
        ];
        assert_eq!(FINISH_REASONS.len(), documented.len());
        for token in documented {
            assert!(
                FINISH_REASONS.iter().any(|(name, _)| *name == token),
                "{token} is not in the table"
            );
        }

        for (token, class) in FINISH_REASONS {
            let payload = format!(
                r#"{{"candidates":[{{"content":{{"parts":[{{"text":"x"}}]}},"finishReason":"{token}"}}]}}"#
            );
            let (state, outcomes) = run(&[payload.as_str()]);
            match class {
                FinishClass::Stop => {
                    assert_eq!(outcomes[0], FrameOutcome::Terminal, "{token}");
                    let sealed = GoogleAdapter.finish(state).expect("seals");
                    assert_eq!(sealed.stop_reason, StopReason::EndTurn, "{token}");
                }
                FinishClass::MaxTokens => {
                    assert_eq!(outcomes[0], FrameOutcome::Terminal, "{token}");
                    let sealed = GoogleAdapter.finish(state).expect("seals");
                    assert_eq!(sealed.stop_reason, StopReason::MaxOutputTokens, "{token}");
                }
                FinishClass::Failure(expected) => match &outcomes[0] {
                    FrameOutcome::Failed(failure) => {
                        assert_eq!(failure.kind(), expected, "{token}");
                        assert!(!state.terminal, "{token} must not seal a turn");
                    }
                    other => panic!("{token} decoded as {other:?} rather than a failure"),
                },
            }
        }
    }

    #[test]
    fn stop_becomes_tool_use_when_function_calls_are_present() {
        let call = r#"{"candidates":[{"content":{"parts":[{"functionCall":{"name":"get_weather","args":{},"id":"fc-1"}}],"role":"model"},"finishReason":"STOP"}]}"#;
        let (state, outcomes) = run(&[call]);
        assert_eq!(outcomes[0], FrameOutcome::Terminal);
        assert_eq!(
            GoogleAdapter.finish(state).expect("seals").stop_reason,
            StopReason::ToolUse
        );
    }

    #[test]
    fn an_undocumented_finish_reason_is_malformed_rather_than_ignored() {
        let payload =
            r#"{"candidates":[{"content":{"parts":[{"text":"x"}]},"finishReason":"NEW_REASON"}]}"#;
        assert_eq!(
            run_err(&[payload]),
            FrameDecodeError::MalformedField {
                field: "finishReason"
            }
        );
    }

    // -----------------------------------------------------------------------
    // usage
    // -----------------------------------------------------------------------

    #[test]
    fn the_usage_golden_adds_thoughts_to_candidates() {
        // `thoughtsTokenCount` is EXCLUDED from `candidatesTokenCount`, so the
        // canonical output count is their sum and reasoning is its subset.
        let payload = concat!(
            r#"{"candidates":[{"content":{"parts":[{"text":"x"}]},"finishReason":"STOP"}],"#,
            r#""usageMetadata":{"promptTokenCount":1000,"cachedContentTokenCount":200,"#,
            r#""candidatesTokenCount":300,"thoughtsTokenCount":700,"#,
            r#""toolUsePromptTokenCount":50,"totalTokenCount":2250}}"#
        );
        let (state, _) = run(&[payload]);
        let usage = state.usage;
        assert_eq!(usage.input_tokens, 800);
        assert_eq!(usage.cache_read_input_tokens, 200);
        assert_eq!(usage.cache_write_input_tokens, 0);
        assert_eq!(usage.output_tokens, 1000);
        assert_eq!(usage.reasoning_tokens, 700);
        assert_eq!(usage.tool_use_prompt_tokens, 50);
        assert_eq!(usage.provider_total_tokens, Some(2250));
        assert_eq!(usage.completeness, UsageCompleteness::Exact);
        assert!(usage.is_consistent());
    }

    #[test]
    fn a_missing_usage_member_is_recorded_as_missing_rather_than_invented() {
        let payload = concat!(
            r#"{"candidates":[{"content":{"parts":[{"text":"x"}]},"finishReason":"STOP"}],"#,
            r#""usageMetadata":{"promptTokenCount":10}}"#
        );
        let (state, _) = run(&[payload]);
        assert_eq!(
            state.usage.completeness,
            UsageCompleteness::Partial {
                missing: UsageFieldSet::EMPTY
                    .with(UsageField::OutputTokens)
                    .with(UsageField::ProviderTotalTokens),
            }
        );
        assert_eq!(state.usage.output_tokens, 0);
    }

    #[test]
    fn a_stream_with_no_usage_at_all_records_absence() {
        let (state, _) = run(&[
            HELLO,
            r#"{"candidates":[{"content":{"parts":[]},"finishReason":"STOP"}]}"#,
        ]);
        assert_eq!(state.usage.completeness, UsageCompleteness::Absent);
    }

    // -----------------------------------------------------------------------
    // failure classification
    // -----------------------------------------------------------------------

    fn classify(status: u16, body: &str) -> crate::error::ProviderFailure {
        let headers = reqwest::header::HeaderMap::new();
        GoogleAdapter.classify_http(
            status,
            &HeaderView::new(&headers),
            &BoundedBody::new(body.as_bytes().to_vec(), false),
        )
    }

    #[test]
    fn the_snake_case_error_shape_classifies() {
        let cases = [
            (400, "invalid_request", ProviderFailureKind::InvalidRequest),
            (401, "authentication", ProviderFailureKind::Authentication),
            (
                403,
                "permission_denied",
                ProviderFailureKind::Authentication,
            ),
            (404, "not_found", ProviderFailureKind::ModelNotFound),
            (429, "rate_limit_exceeded", ProviderFailureKind::RateLimited),
            (429, "quota_exceeded", ProviderFailureKind::Quota),
            (500, "api_error", ProviderFailureKind::ServerError),
            (503, "service_unavailable", ProviderFailureKind::Overloaded),
        ];
        for (status, code, expected) in cases {
            let body = format!(r#"{{"error":{{"code":"{code}","message":"nope"}}}}"#);
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
    }

    #[test]
    fn an_undocumented_snake_case_code_falls_back_to_the_status() {
        // The reference states any other code is the snake_case status text.
        let failure = classify(
            502,
            r#"{"error":{"code":"bad_gateway","message":"upstream"}}"#,
        );
        assert_eq!(failure.kind(), ProviderFailureKind::ServerError);
        assert_eq!(
            failure
                .detail
                .provider_code
                .as_ref()
                .map(BoundedString::as_str),
            Some("bad_gateway")
        );
    }

    #[test]
    fn the_grpc_error_shape_classifies() {
        let body = concat!(
            r#"{"error":{"code":429,"message":"Resource has been exhausted.","#,
            r#""status":"RESOURCE_EXHAUSTED","details":[{"@type":"type.googleapis.com/google.rpc.RetryInfo","#,
            r#""retryDelay":"30s"}]}}"#
        );
        let failure = classify(429, body);
        assert_eq!(failure.kind(), ProviderFailureKind::RateLimited);
        assert_eq!(
            failure
                .detail
                .provider_code
                .as_ref()
                .map(BoundedString::as_str),
            Some("RESOURCE_EXHAUSTED")
        );
        assert_eq!(
            failure.rate_limit.retry_after,
            Some(core::time::Duration::from_secs(30))
        );
        assert_eq!(failure.rate_limit.source, RateLimitSource::ErrorBody);
    }

    #[test]
    fn a_retry_delay_in_the_error_details_is_the_only_backpressure_google_publishes() {
        let without = classify(
            429,
            r#"{"error":{"code":"rate_limit_exceeded","message":"slow"}}"#,
        );
        assert!(without.rate_limit.is_absent());

        let fractional = concat!(
            r#"{"error":{"code":503,"status":"UNAVAILABLE","#,
            r#""details":[{"retryDelay":"1.500s"}]}}"#
        );
        let failure = classify(503, fractional);
        assert_eq!(
            failure.rate_limit.retry_after,
            Some(core::time::Duration::from_millis(1500))
        );
    }

    #[test]
    fn google_publishes_no_rate_limit_headers_at_all() {
        // A positive record, not an absence of observation: even a header that
        // looks like backpressure is not part of this dialect.
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert("retry-after", "30".parse().expect("value"));
        headers.insert("x-ratelimit-remaining", "0".parse().expect("value"));
        let feedback = GoogleAdapter.rate_limit_feedback(&HeaderView::new(&headers));
        assert!(feedback.is_absent());
        assert_eq!(feedback.retry_after, None);
        assert_eq!(feedback.source, RateLimitSource::NotProvided);
    }

    #[test]
    fn a_credential_shaped_run_in_an_error_body_is_redacted() {
        let key = "AIzaSyD3aBcDeFgHiJkLmNoPqRsTuVwXyZ012345";
        let body = format!(r#"{{"error":{{"code":"authentication","message":"bad key {key}"}}}}"#);
        let failure = classify(401, &body);
        assert!(!failure.detail.message.as_str().contains(key));
        assert!(failure.detail.message.as_str().contains("[redacted]"));
    }

    #[test]
    fn a_truncated_error_body_still_classifies_by_status() {
        let headers = reqwest::header::HeaderMap::new();
        let failure = GoogleAdapter.classify_http(
            503,
            &HeaderView::new(&headers),
            &BoundedBody::new(br#"{"error":{"code":"serv"#.to_vec(), true),
        );
        assert_eq!(failure.kind(), ProviderFailureKind::Overloaded);
        assert_eq!(
            failure.detail.message.as_str(),
            "the provider error body exceeded its bound"
        );
    }

    #[test]
    fn the_request_id_comes_from_the_response_body_not_a_header() {
        let headers = reqwest::header::HeaderMap::new();
        let (state, _) = run(&[HELLO, WORLD, STOP]);
        assert_eq!(
            GoogleAdapter
                .request_id(&HeaderView::new(&headers), &state)
                .as_ref()
                .map(BoundedString::as_str),
            Some("resp-1")
        );
        assert_eq!(
            GoogleAdapter.request_id(&HeaderView::new(&headers), &GoogleAdapter::fresh_state()),
            None
        );
    }
}
