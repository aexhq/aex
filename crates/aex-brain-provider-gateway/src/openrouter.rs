//! `OpenRouter`'s customer-key chat-completions authority.
//!
//! The endpoint and dialect are compiled. The adapter never accepts an
//! arbitrary base URL or an upstream provider credential; the shared transport
//! attaches only the customer's `OpenRouter` key as a sensitive bearer header.

use core::time::Duration;

use aex_model_catalog::QualifiedModel;
use aex_model_catalog::canonical::CanonicalModelRequest;
use aex_model_catalog::document::{Dialect, EndpointPin};
use aex_model_catalog::primitives::ProviderRequestId;
use aex_wire::provider::ProviderId;
use serde_json::Value;

use crate::adapter::{
    BoundedBody, DialectState, FrameDecodeError, FrameOutcome, HeaderView, ProviderAdapter,
    RequestBuildError, SealedResponse,
};
use crate::budget::StreamBudget;
use crate::error::{ProviderFailure, ProviderFailureKind, RateLimitFeedback, RedactedDetail};
use crate::moonshotai::{
    GatewayChatAuthority, build_gateway_request, decode_gateway_event, finish_gateway_stream,
    fresh_state,
};
use crate::redact::redact;
use crate::sse::SseEvent;
use crate::transport::WireRequest;

const AUTHORITY: GatewayChatAuthority = GatewayChatAuthority {
    provider: ProviderId::Openrouter,
    dialect: Dialect::OpenRouterChat,
    endpoint: EndpointPin::OpenRouterApiV1,
    path: "/api/v1/chat/completions",
};

/// Stateless `OpenRouter` adapter.
#[derive(Debug, Clone, Copy, Default)]
pub struct OpenRouterAdapter;

impl ProviderAdapter for OpenRouterAdapter {
    fn provider(&self) -> ProviderId {
        ProviderId::Openrouter
    }

    fn build_request(
        &self,
        model: &QualifiedModel,
        request: &CanonicalModelRequest,
    ) -> Result<WireRequest, RequestBuildError> {
        build_gateway_request(AUTHORITY, model, request)
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
        decode_gateway_event(state, event, budget, classify)
    }

    fn finish(&self, state: DialectState) -> Result<SealedResponse, FrameDecodeError> {
        finish_gateway_stream(state)
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
            .and_then(|value| value.get("metadata"))
            .and_then(|value| value.get("error_type"))
            .and_then(Value::as_str)
            .or_else(|| {
                error
                    .and_then(|value| value.get("type"))
                    .and_then(Value::as_str)
            })
            .or_else(|| {
                error
                    .and_then(|value| value.get("code"))
                    .and_then(Value::as_str)
            });
        let message = error
            .and_then(|value| value.get("message"))
            .and_then(Value::as_str)
            .unwrap_or("OpenRouter returned no readable error body");
        let kind = classify(status, code);
        let mut detail = RedactedDetail::new(kind, redact::<512>(message, &[])).with_status(status);
        if let Some(code) = code {
            detail = detail.with_code(redact::<64>(code, &[]).as_str());
        }
        ProviderFailure {
            detail,
            rate_limit: self.rate_limit_feedback(headers),
        }
    }

    fn rate_limit_feedback(&self, headers: &HeaderView<'_>) -> RateLimitFeedback {
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
            .get("x-generation-id")
            .map(ProviderRequestId::truncating)
            .or_else(|| state.request_id.clone())
    }
}

fn classify(status: u16, code: Option<&str>) -> ProviderFailureKind {
    match code {
        Some("authentication" | "invalid_api_key") => ProviderFailureKind::Authentication,
        Some("insufficient_credits" | "quota_exceeded") => ProviderFailureKind::Quota,
        Some("rate_limit_exceeded") => ProviderFailureKind::RateLimited,
        Some("provider_overloaded") => ProviderFailureKind::Overloaded,
        Some("provider_unavailable") => ProviderFailureKind::ServerError,
        Some("context_length_exceeded") => ProviderFailureKind::ContextOverflow,
        Some("content_filter" | "moderation") => ProviderFailureKind::ContentFiltered,
        Some("not_found" | "model_not_found") => ProviderFailureKind::ModelNotFound,
        Some("invalid_request" | "invalid_prompt" | "unprocessable") => {
            ProviderFailureKind::InvalidRequest
        }
        Some("timeout") => ProviderFailureKind::Timeout,
        _ => match status {
            400 | 408 | 412 | 413 | 422 => ProviderFailureKind::InvalidRequest,
            401 | 403 => ProviderFailureKind::Authentication,
            402 => ProviderFailureKind::Quota,
            404 => ProviderFailureKind::ModelNotFound,
            429 => ProviderFailureKind::RateLimited,
            503 => ProviderFailureKind::Overloaded,
            504 => ProviderFailureKind::Timeout,
            _ => ProviderFailureKind::ServerError,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::{OpenRouterAdapter, classify};
    use crate::adapter::{FrameOutcome, ProviderAdapter};
    use crate::budget::StreamBudget;
    use crate::error::ProviderFailureKind;
    use crate::sse::SseEvent;
    use aex_model_catalog::canonical::{
        CanonicalBlock, CanonicalMessage, CanonicalModelRequest, CorrelationId, ReasoningRequest,
        Role, ToolChoice,
    };
    use aex_model_catalog::document::{Capability, CapabilitySet};
    use aex_model_catalog::fixture;
    use aex_wire::{ContentHash, provider::ProviderId};

    fn request() -> CanonicalModelRequest {
        let model = fixture::qualified_entry(
            ProviderId::Openrouter,
            "anthropic/claude-sonnet-4.6",
            CapabilitySet::from_slice(&[
                Capability::TextIn,
                Capability::TextOut,
                Capability::Streaming,
            ]),
        );
        CanonicalModelRequest {
            selection: model,
            system: Vec::new(),
            messages: vec![CanonicalMessage {
                role: Role::User,
                blocks: vec![CanonicalBlock::Text {
                    text: fixture::bounded("hello"),
                    annotations: Vec::new(),
                }],
            }],
            tools: Vec::new(),
            tool_choice: ToolChoice::Auto,
            parallel_tools: true,
            max_output_tokens: 128,
            temperature_milli: None,
            top_p_milli: None,
            stop_sequences: Vec::new(),
            reasoning: ReasoningRequest::ProviderDefault,
            structured_output: None,
            cache_breakpoints: Vec::new(),
            correlation: CorrelationId::from_effect([1; 16]),
            request_hash: ContentHash::of(b"openrouter-golden"),
        }
    }

    #[test]
    fn identity_and_typed_gateway_failures_are_distinct() {
        assert_eq!(OpenRouterAdapter.provider(), ProviderId::Openrouter);
        assert_eq!(
            classify(503, Some("provider_overloaded")),
            ProviderFailureKind::Overloaded
        );
        assert_eq!(
            classify(402, Some("insufficient_credits")),
            ProviderFailureKind::Quota
        );
    }

    #[test]
    fn request_and_stream_goldens_pin_origin_identity_and_route_metadata() {
        let request = request();
        let wire = OpenRouterAdapter
            .build_request(&request.selection, &request)
            .expect("request builds");
        assert_eq!(
            wire.url().expect("url").as_str(),
            "https://openrouter.ai/api/v1/chat/completions"
        );
        let body: serde_json::Value = serde_json::from_slice(&wire.body).expect("JSON body");
        assert_eq!(body["model"], "anthropic/claude-sonnet-4.6");
        assert_eq!(body["max_tokens"], 128);
        assert!(body.get("api_key").is_none());

        let mut state = OpenRouterAdapter.new_state(&request.selection);
        let content = br#"{"id":"gen-1","object":"chat.completion.chunk","model":"anthropic/claude-sonnet-4.6","provider":"Anthropic","choices":[{"index":0,"delta":{"content":"hello"},"finish_reason":"stop"}]}"#;
        assert_eq!(
            OpenRouterAdapter
                .decode(
                    &mut state,
                    &SseEvent {
                        name: None,
                        data: content,
                        id: None,
                    },
                    &StreamBudget::default(),
                )
                .expect("content decodes"),
            FrameOutcome::ResponseStarted
        );
        assert_eq!(
            OpenRouterAdapter
                .decode(
                    &mut state,
                    &SseEvent {
                        name: None,
                        data: b"[DONE]",
                        id: None,
                    },
                    &StreamBudget::default(),
                )
                .expect("sentinel decodes"),
            FrameOutcome::Terminal
        );
        let sealed = OpenRouterAdapter.finish(state).expect("stream seals");
        let route = sealed.gateway_route.expect("route metadata is bounded");
        assert_eq!(
            route.reported_model.expect("model").as_str(),
            "anthropic/claude-sonnet-4.6"
        );
        assert_eq!(
            route.reported_provider.expect("provider").as_str(),
            "Anthropic"
        );
    }

    #[test]
    fn stream_errors_are_typed_and_route_changes_fail_closed() {
        let request = request();
        let budget = StreamBudget::default();
        let mut failed = OpenRouterAdapter.new_state(&request.selection);
        let error = br#"{"error":{"code":429,"message":"slow down","metadata":{"error_type":"rate_limit_exceeded"}}}"#;
        let outcome = OpenRouterAdapter
            .decode(
                &mut failed,
                &SseEvent {
                    name: None,
                    data: error,
                    id: None,
                },
                &budget,
            )
            .expect("typed in-stream error decodes");
        let FrameOutcome::Failed(failure) = outcome else {
            panic!("the error must not be mistaken for progress");
        };
        assert_eq!(failure.kind(), ProviderFailureKind::RateLimited);

        let mut changed = OpenRouterAdapter.new_state(&request.selection);
        let initial = br#"{"object":"chat.completion.chunk","model":"anthropic/claude-sonnet-4.6","provider":"Anthropic","choices":[{"index":0,"delta":{"content":"a"},"finish_reason":null}]}"#;
        OpenRouterAdapter
            .decode(
                &mut changed,
                &SseEvent {
                    name: None,
                    data: initial,
                    id: None,
                },
                &budget,
            )
            .expect("the initial route is accepted");
        let changed_route = br#"{"object":"chat.completion.chunk","model":"anthropic/claude-sonnet-4.6","provider":"Other","choices":[{"index":0,"delta":{"content":"b"},"finish_reason":"stop"}]}"#;
        assert!(
            OpenRouterAdapter
                .decode(
                    &mut changed,
                    &SseEvent {
                        name: None,
                        data: changed_route,
                        id: None,
                    },
                    &budget,
                )
                .is_err(),
            "a changed route must fail closed"
        );
    }
}
