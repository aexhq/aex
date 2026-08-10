//! The provider-adapter trait every dialect module implements (plan 08 §3.2).
//!
//! `build_request` is **pure**: no I/O, no clock, no credential. It rejects
//! anything the catalog entry does not declare before a socket exists, which is
//! where "unknown or unsupported fails before dispatch" is enforced.

use aex_model_catalog::QualifiedModel;
use aex_model_catalog::canonical::{
    CanonicalBlock, CanonicalModelRequest, GatewayRoute, NormalizedUsage, StopReason,
    StructuredOutputRequest, ToolChoice,
};
use aex_model_catalog::document::Capability;
use aex_model_catalog::primitives::{ProviderRequestId, ToolCallId, ToolName};
use aex_wire::provider::ProviderId;

use crate::budget::{BudgetOverrun, StreamBudget};
use crate::error::{ProviderFailure, RateLimitFeedback};
use crate::sse::SseEvent;
use crate::transport::WireRequest;

/// Why a request could not be built. Every arm fires before any socket exists,
/// so every one is `DispatchProof::NotSent`.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RequestBuildError {
    /// The entry does not declare a capability the request needs.
    #[error("the pair does not declare `{}`", capability.as_str())]
    CapabilityUnavailable {
        /// Which capability.
        capability: Capability,
    },
    /// The provider does not support the requested tool-choice mode.
    #[error("the provider does not support this tool choice")]
    ToolChoiceUnsupported {
        /// What was asked for.
        requested: Box<ToolChoice>,
    },
    /// The provider does not support the requested structured-output form.
    #[error("the provider does not support this structured-output form")]
    StructuredOutputUnsupported {
        /// What was asked for.
        requested: Box<StructuredOutputRequest>,
    },
    /// `DeepSeek` documents that the word `json` must appear in the prompt for
    /// `json_object` mode. A caller obligation, surfaced rather than silently
    /// patched into the prompt.
    #[error("this provider requires the word `json` in the prompt for JSON-object mode")]
    StructuredOutputPromptRequirement,
    /// The provider has no stop-sequence field at all.
    #[error("the provider has no stop-sequence field")]
    StopSequencesUnsupported,
    /// Too many stop sequences.
    #[error("the provider accepts at most {max} stop sequences")]
    StopSequenceLimit {
        /// The bound.
        max: u8,
    },
    /// Too many tools.
    #[error("the provider accepts at most {max} tools")]
    ToolLimit {
        /// The bound.
        max: u16,
    },
    /// A tool name the provider's grammar rejects.
    #[error("the provider rejects tool name `{name}`")]
    ToolNameInvalid {
        /// The offending name.
        name: ToolName,
    },
    /// A sampling field the pair does not honour. Never silently dropped.
    #[error("the pair does not honour `{field}`")]
    SamplingUnsupported {
        /// Which field.
        field: &'static str,
    },
    /// A sampling field that reasoning mode forbids.
    #[error("reasoning mode forbids `{field}` on this pair")]
    SamplingWithReasoning {
        /// Which field.
        field: &'static str,
    },
    /// The output ceiling is outside the pair's range.
    #[error("max_output_tokens must be within {min}..={max}")]
    OutputTokensOutOfRange {
        /// The floor.
        min: u32,
        /// The ceiling.
        max: u32,
    },
    /// The reasoning budget is outside the pair's range.
    #[error("the reasoning budget must be within {min}..={max}")]
    ReasoningBudgetOutOfRange {
        /// The floor.
        min: u32,
        /// The ceiling.
        max: u32,
    },
    /// Reasoning material from another provider.
    #[error("reasoning material from `{found}` cannot be replayed to `{expected}`")]
    ReasoningProvenanceMismatch {
        /// Whose material this pair needs.
        expected: ProviderId,
        /// Whose material was carried.
        found: ProviderId,
    },
    /// Round-trip reasoning material is required and absent.
    #[error("this pair requires reasoning round-trip material and none was carried")]
    ReasoningTokenRequired,
    /// The estimated prompt exceeds the declared context window.
    #[error("the estimated prompt of {estimated} tokens exceeds the {limit}-token window")]
    ContextOverflowEstimated {
        /// The window.
        limit: u32,
        /// The estimate.
        estimated: u32,
    },
    /// The serialized body exceeds the pair's request bound.
    #[error("the request body exceeds the {limit}-byte bound")]
    BodyTooLarge {
        /// The bound.
        limit: u32,
    },
    /// A value could not be canonicalized or bounded.
    #[error("a request member could not be encoded: {reason}")]
    Encoding {
        /// What went wrong, in this crate's own words.
        reason: &'static str,
    },
}

/// What a decoded frame did to the dialect state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FrameOutcome {
    /// The frame carried nothing the canonical model cares about — a `ping`, a
    /// keep-alive comment, a lifecycle echo.
    ///
    /// An ignored frame is explicitly **not** proof that the provider is
    /// generating: only a decoded dialect frame moves the effect to
    /// `response_started`, because that is a durable write which must mean
    /// what it says.
    Ignored,
    /// The first frame that proves the provider is generating.
    ResponseStarted,
    /// Content arrived.
    Progress,
    /// The dialect's terminal frame arrived.
    Terminal,
    /// The provider reported a failure mid-stream.
    Failed(Box<ProviderFailure>),
}

/// Why a frame could not be decoded.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum FrameDecodeError {
    /// The payload is not `JSON`.
    #[error("the frame payload is not JSON")]
    NotJson,
    /// The dialect does not know this event type.
    ///
    /// This is how a provider-hosted tool event — `web_search_call.*`,
    /// `mcp_*`, Z.AI `web_search` — becomes a `ProtocolViolation`: those tools
    /// are never sent, so receiving one means the request was altered in flight
    /// (D-25).
    #[error("`{event}` is not a frame this dialect sends")]
    UnknownEvent {
        /// The offending type.
        event: aex_model_catalog::BoundedString<64>,
    },
    /// A field the dialect requires is missing or the wrong shape.
    #[error("frame field `{field}` is missing or malformed")]
    MalformedField {
        /// Which field.
        field: &'static str,
    },
    /// Frames arrived in an order the dialect cannot produce.
    #[error("frame order violates the dialect state machine: {reason}")]
    OutOfOrder {
        /// What was violated.
        reason: &'static str,
    },
    /// Tool-call argument fragments did not reassemble into valid `JSON`.
    #[error("tool arguments for `{call}` did not reassemble into valid JSON")]
    ToolArgumentsNotJson {
        /// Which call.
        call: ToolCallId,
    },
    /// A bound was crossed while decoding.
    #[error("a stream bound was crossed: {0}")]
    Budget(#[from] BudgetOverrun),
}

/// A finished, decoded response, before sealing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SealedResponse {
    /// The canonical block set, in order.
    pub blocks: Vec<CanonicalBlock>,
    /// The terminal stop reason.
    pub stop_reason: StopReason,
    /// Normalized token accounting.
    pub usage: NormalizedUsage,
    /// The provider's own request id, where it published one.
    pub provider_request_id: Option<ProviderRequestId>,
    /// Bounded gateway route metadata, absent for direct providers.
    pub gateway_route: Option<GatewayRoute>,
}

/// A read-only view of response headers, so an adapter cannot mutate them.
#[derive(Debug, Clone, Copy)]
pub struct HeaderView<'a>(&'a reqwest::header::HeaderMap);

impl<'a> HeaderView<'a> {
    /// Wraps a header map.
    #[must_use]
    pub const fn new(map: &'a reqwest::header::HeaderMap) -> Self {
        Self(map)
    }

    /// One header value as text, where it is valid `ASCII`.
    #[must_use]
    pub fn get(&self, name: &str) -> Option<&'a str> {
        self.0.get(name).and_then(|value| value.to_str().ok())
    }

    /// One header parsed as an unsigned integer.
    #[must_use]
    pub fn get_u64(&self, name: &str) -> Option<u64> {
        self.get(name).and_then(|text| text.trim().parse().ok())
    }
}

/// A bounded error body. Read to the budget's ceiling and no further.
#[derive(Clone, PartialEq, Eq)]
pub struct BoundedBody {
    bytes: Vec<u8>,
    truncated: bool,
}

impl core::fmt::Debug for BoundedBody {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("BoundedBody")
            .field("len", &self.bytes.len())
            .field("truncated", &self.truncated)
            .finish()
    }
}

impl BoundedBody {
    /// Wraps bytes already read under a bound.
    #[must_use]
    pub fn new(bytes: Vec<u8>, truncated: bool) -> Self {
        Self { bytes, truncated }
    }

    /// The bytes.
    #[must_use]
    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes
    }

    /// The bytes as text, where they are valid UTF-8.
    #[must_use]
    pub fn as_str(&self) -> Option<&str> {
        core::str::from_utf8(&self.bytes).ok()
    }

    /// The body parsed as `JSON`, where it parses.
    #[must_use]
    pub fn as_json(&self) -> Option<serde_json::Value> {
        serde_json::from_slice(&self.bytes).ok()
    }

    /// Whether the bound cut the body short.
    #[must_use]
    pub const fn is_truncated(&self) -> bool {
        self.truncated
    }

    /// An empty body, for a response that carried none.
    #[must_use]
    pub fn empty() -> Self {
        Self {
            bytes: Vec::new(),
            truncated: false,
        }
    }
}

/// One provider dialect.
///
/// Six implementations, one per launch provider. The trait is object-safe so
/// the router holds `&dyn ProviderAdapter` and selects by pinned pair through a
/// total match with exactly one call site per arm.
pub trait ProviderAdapter: Send + Sync + 'static {
    /// Which provider this adapter speaks for.
    fn provider(&self) -> ProviderId;

    /// Builds the wire request. Pure: no I/O, no clock, no credential.
    ///
    /// # Errors
    ///
    /// Returns [`RequestBuildError`] for anything the catalog entry does not
    /// declare, before a socket exists.
    fn build_request(
        &self,
        model: &QualifiedModel,
        request: &CanonicalModelRequest,
    ) -> Result<WireRequest, RequestBuildError>;

    /// A fresh decoder state for one stream.
    fn new_state(&self, model: &QualifiedModel) -> DialectState;

    /// Decodes one event into the state. Bounded, incremental, non-blocking.
    ///
    /// # Errors
    ///
    /// Returns [`FrameDecodeError`] for a malformed, unknown or out-of-order
    /// frame, and for a bound crossed while decoding.
    fn decode(
        &self,
        state: &mut DialectState,
        event: &SseEvent<'_>,
        budget: &StreamBudget,
    ) -> Result<FrameOutcome, FrameDecodeError>;

    /// Finishes a stream, producing the canonical block set.
    ///
    /// # Errors
    ///
    /// Returns [`FrameDecodeError`] when the stream ended without a terminal
    /// frame or with an unbalanced block set.
    fn finish(&self, state: DialectState) -> Result<SealedResponse, FrameDecodeError>;

    /// Classifies a non-2xx response.
    fn classify_http(
        &self,
        status: u16,
        headers: &HeaderView<'_>,
        body: &BoundedBody,
    ) -> ProviderFailure;

    /// Reads whatever backpressure the provider published.
    fn rate_limit_feedback(&self, headers: &HeaderView<'_>) -> RateLimitFeedback;

    /// The provider's own request id, from a header or from the decoded state.
    fn request_id(
        &self,
        headers: &HeaderView<'_>,
        state: &DialectState,
    ) -> Option<ProviderRequestId>;
}

/// The dialect-neutral part of a decoder's state.
///
/// Every dialect accumulates the same shapes — ordered blocks, per-index
/// tool-argument fragments, a usage tally, a finish token — so the state lives
/// here and each adapter drives it. That is what lets the block-assembly and
/// budget rules be written once rather than eight times.
#[derive(Debug, Default)]
pub struct DialectState {
    /// Blocks completed so far, in arrival order.
    pub blocks: Vec<CanonicalBlock>,
    /// Text accumulating for the open text block, by block index.
    pub open_text: std::collections::BTreeMap<u16, String>,
    /// Reasoning accumulating for the open reasoning block, by block index.
    pub open_reasoning: std::collections::BTreeMap<u16, String>,
    /// Round-trip reasoning material captured for the open reasoning block.
    pub open_reasoning_token: std::collections::BTreeMap<u16, Vec<u8>>,
    /// Tool calls accumulating, by the provider's own fragment index.
    pub open_tools: std::collections::BTreeMap<u16, PartialToolCall>,
    /// The provider's finish token, once it arrives.
    pub finish_token: Option<String>,
    /// Usage as reported so far.
    pub usage: NormalizedUsage,
    /// The provider's own request id, where the body carries it.
    pub request_id: Option<ProviderRequestId>,
    /// Bounded gateway route metadata accumulated from response frames.
    pub gateway_route: Option<GatewayRoute>,
    /// Whether a terminal frame has been decoded.
    pub terminal: bool,
    /// Whether any dialect frame has been decoded, which is what proves the
    /// provider is generating.
    pub response_started: bool,
    /// Running byte and count ledger.
    pub ledger: crate::budget::BudgetLedger,
}

/// A tool call being reassembled from fragments.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct PartialToolCall {
    /// The provider's call id, which arrives on the first fragment.
    pub id: Option<ToolCallId>,
    /// The tool name, which arrives on the first fragment.
    pub name: Option<String>,
    /// The accumulated argument text, which is generally not valid `JSON` until
    /// the last fragment.
    pub arguments: String,
}

impl DialectState {
    /// A fresh state.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Marks the first decoded dialect frame.
    ///
    /// Returns [`FrameOutcome::ResponseStarted`] exactly once per stream and
    /// [`FrameOutcome::Progress`] thereafter, so the durable
    /// `mark_response_started` write happens once and means what it says.
    pub fn mark_started(&mut self) -> FrameOutcome {
        if self.response_started {
            return FrameOutcome::Progress;
        }
        self.response_started = true;
        FrameOutcome::ResponseStarted
    }
}

#[cfg(test)]
mod tests {
    use aex_brain_app::ports::ToolAdvertisement;
    use aex_model_catalog::canonical::{
        CanonicalBlock, CanonicalMessage, CanonicalModelRequest, CanonicalToolDef, CorrelationId,
        ReasoningRequest, Role, ToolChoice,
    };
    use aex_model_catalog::document::{
        Capability, CapabilitySet, ModelEntry, NamePattern, ToolArgumentEncoding, ToolEncoding,
        ToolPolicy,
    };
    use aex_model_catalog::fixture;
    use aex_model_catalog::primitives::{BoundedString, ToolName};
    use aex_wire::provider::ProviderId;
    use aex_wire::{CanonicalJson, ContentHash};

    use super::{BoundedBody, DialectState, FrameOutcome, HeaderView, ProviderAdapter};
    use crate::adapter::RequestBuildError;
    use crate::deepseek::DeepSeekAdapter;
    use crate::google::GoogleAdapter;
    use crate::moonshotai::MoonshotAdapter;
    use crate::zai::ZaiAdapter;

    #[test]
    fn the_first_frame_starts_the_response_exactly_once() {
        let mut state = DialectState::new();
        assert_eq!(state.mark_started(), FrameOutcome::ResponseStarted);
        assert_eq!(state.mark_started(), FrameOutcome::Progress);
        assert_eq!(state.mark_started(), FrameOutcome::Progress);
    }

    #[test]
    fn a_bounded_body_reports_its_own_truncation() {
        let body = BoundedBody::new(b"{\"error\":".to_vec(), true);
        assert!(body.is_truncated());
        assert!(body.as_json().is_none(), "a cut body must not parse");
        assert_eq!(body.as_str(), Some("{\"error\":"));
    }

    #[test]
    fn an_error_body_debug_rendering_never_carries_provider_bytes() {
        let body = BoundedBody::new(b"echoed sk-012345678901234567890123456789".to_vec(), false);
        let rendered = format!("{body:?}");
        assert!(rendered.contains("len"));
        assert!(rendered.contains("truncated: false"));
        assert!(!rendered.contains("sk-"));
    }

    #[test]
    fn an_empty_body_is_not_truncated() {
        let body = BoundedBody::empty();
        assert!(!body.is_truncated());
        assert!(body.as_bytes().is_empty());
    }

    #[test]
    fn header_view_reads_text_and_integers() {
        let mut map = reqwest::header::HeaderMap::new();
        map.insert("retry-after", "12".parse().expect("value"));
        map.insert("x-request-id", "req_abc".parse().expect("value"));
        let view = HeaderView::new(&map);
        assert_eq!(view.get_u64("retry-after"), Some(12));
        assert_eq!(view.get("x-request-id"), Some("req_abc"));
        assert_eq!(view.get("absent"), None);
        assert_eq!(view.get_u64("x-request-id"), None);
    }

    // -----------------------------------------------------------------------
    // the four dialects that cannot say "one tool at a time"
    //
    // Each module already pins its own refusal. What no test held was the class
    // — that these four are exactly the dialects with no such field, and that
    // the advertised surface a deployed task carries does not ask them for it.
    // The production composition advertises `web_fetch`, which is neither pure,
    // deterministic nor zero-weight, so while one flag answered both questions
    // every tool-bearing turn on four of the six providers failed at request
    // build. `ToolAdvertisement::allows_parallel_emission` is the value that
    // reaches `parallel_tools`, and it consults the model and nothing else.
    // -----------------------------------------------------------------------

    /// One capability set wide enough that only the parallel-tool gate can fire.
    fn tool_capable() -> CapabilitySet {
        CapabilitySet::from_slice(&[
            Capability::TextIn,
            Capability::TextOut,
            Capability::Streaming,
            Capability::Tools,
            Capability::ParallelTools,
            Capability::SystemInstruction,
        ])
    }

    /// The `web_fetch` row as the router advertises it: a real name, an object
    /// schema, nothing exotic.
    fn web_fetch() -> CanonicalToolDef {
        CanonicalToolDef {
            name: ToolName::parse("web_fetch").expect("a catalog tool name"),
            description: BoundedString::truncating("Fetch one URL."),
            input_schema: CanonicalJson::parse(
                r#"{"type":"object","properties":{"url":{"type":"string"}}}"#,
            )
            .expect("an object schema"),
            strict: false,
        }
    }

    /// The four entries, each shaped the way its own dialect requires.
    fn entries_that_cannot_forbid_parallel_calls() -> Vec<(&'static dyn ProviderAdapter, ModelEntry)>
    {
        let mut google = fixture::entry(ProviderId::Google, "gemini-3-pro", tool_capable());
        google.tool_policy = ToolPolicy {
            encoding: ToolEncoding::GeminiFunctionDeclarations,
            arguments: ToolArgumentEncoding::JsonObject,
            requires_stream_opt_in: false,
            max_name_bytes: 64,
            name_pattern: NamePattern::GeminiFunctionName,
        };
        vec![
            (
                &DeepSeekAdapter,
                fixture::entry(ProviderId::Deepseek, "deepseek-chat", tool_capable()),
            ),
            (&GoogleAdapter, google),
            (
                &ZaiAdapter,
                fixture::entry(ProviderId::Zai, "glm-5.2", tool_capable()),
            ),
            (
                &MoonshotAdapter,
                fixture::entry(ProviderId::Moonshotai, "kimi-k2", tool_capable()),
            ),
        ]
    }

    fn tool_bearing_request(entry: ModelEntry, parallel_tools: bool) -> CanonicalModelRequest {
        CanonicalModelRequest {
            selection: fixture::qualified(entry),
            system: Vec::new(),
            messages: vec![CanonicalMessage {
                role: Role::User,
                blocks: vec![CanonicalBlock::Text {
                    text: BoundedString::truncating("read this page"),
                    annotations: Vec::new(),
                }],
            }],
            tools: vec![web_fetch()],
            tool_choice: ToolChoice::Auto,
            parallel_tools,
            max_output_tokens: 1_024,
            temperature_milli: None,
            top_p_milli: None,
            stop_sequences: Vec::new(),
            // Leave reasoning to the provider: asking a pair to disable something
            // it never declared is its own refusal, and this case is about the
            // tool gate.
            reasoning: ReasoningRequest::ProviderDefault,
            structured_output: None,
            cache_breakpoints: Vec::new(),
            correlation: CorrelationId::from_effect([0u8; 16]),
            request_hash: ContentHash::of(b""),
        }
    }

    /// The advertised surface a deployed task carries builds on all four.
    #[test]
    fn a_non_pure_advertised_tool_no_longer_takes_four_dialects_out_of_service() {
        let advertised = ToolAdvertisement {
            definitions: vec![web_fetch()],
            // The deployed value. It bounds our own concurrency and must not
            // reach the wire.
            parallel_safe: false,
        };
        let parallel = advertised.allows_parallel_emission(tool_capable());
        assert!(
            parallel,
            "a model that declares parallel tools is told it may use them"
        );

        for (adapter, entry) in entries_that_cannot_forbid_parallel_calls() {
            let provider = entry.provider;
            let request = tool_bearing_request(entry, parallel);
            assert!(
                adapter.build_request(&request.selection, &request).is_ok(),
                "{provider} must build a tool-bearing request from the advertised surface"
            );
        }
    }

    /// And the refusal that made the split necessary is still exact.
    #[test]
    fn each_of_the_four_refuses_a_request_it_cannot_encode_rather_than_altering_it() {
        let expected = |provider: ProviderId| match provider {
            ProviderId::Deepseek => RequestBuildError::SamplingUnsupported {
                field: "parallel_tool_calls",
            },
            ProviderId::Google => RequestBuildError::Encoding {
                reason: "google has no field that forbids parallel function calls",
            },
            ProviderId::Zai => RequestBuildError::Encoding {
                reason: "Z.AI has no switch that disables parallel tool calls",
            },
            ProviderId::Moonshotai => RequestBuildError::Encoding {
                reason: "this dialect has no parallel-tool-calls switch",
            },
            other => panic!("`{other}` is not one of the four inexpressive dialects"),
        };

        for (adapter, entry) in entries_that_cannot_forbid_parallel_calls() {
            let provider = entry.provider;
            let request = tool_bearing_request(entry, false);
            assert_eq!(
                adapter.build_request(&request.selection, &request),
                Err(expected(provider)),
                "{provider} must refuse rather than send something it was not asked for"
            );
        }
    }
}
