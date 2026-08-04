//! Vercel AI Gateway's customer-key chat-completions authority.
//!
//! Only the fixed AI Gateway origin is reachable. AEX stores and sends the
//! customer's AI Gateway key; any upstream BYOK configuration remains inside
//! the customer's Vercel account and never crosses the AEX request boundary.

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
    provider: ProviderId::VercelAiGateway,
    dialect: Dialect::VercelAiGatewayChat,
    endpoint: EndpointPin::VercelAiGatewayV1,
    path: "/v1/chat/completions",
};

/// Stateless Vercel AI Gateway adapter.
#[derive(Debug, Clone, Copy, Default)]
pub struct VercelAiGatewayAdapter;

impl ProviderAdapter for VercelAiGatewayAdapter {
    fn provider(&self) -> ProviderId {
        ProviderId::VercelAiGateway
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
            .and_then(|value| value.get("code"))
            .and_then(Value::as_str)
            .or_else(|| {
                error
                    .and_then(|value| value.get("type"))
                    .and_then(Value::as_str)
            });
        let message = error
            .and_then(|value| value.get("message"))
            .and_then(Value::as_str)
            .unwrap_or("Vercel AI Gateway returned no readable error body");
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
            .get("x-ai-gateway-request-id")
            .or_else(|| headers.get("x-request-id"))
            .map(ProviderRequestId::truncating)
            .or_else(|| state.request_id.clone())
    }
}

fn classify(status: u16, code: Option<&str>) -> ProviderFailureKind {
    match code {
        Some("invalid_api_key" | "authentication_error") => ProviderFailureKind::Authentication,
        Some("insufficient_quota" | "billing_error") => ProviderFailureKind::Billing,
        Some("rate_limit_exceeded") => ProviderFailureKind::RateLimited,
        Some("model_not_found") => ProviderFailureKind::ModelNotFound,
        Some("context_length_exceeded") => ProviderFailureKind::ContextOverflow,
        Some("content_filter") => ProviderFailureKind::ContentFiltered,
        Some("invalid_request_error") => ProviderFailureKind::InvalidRequest,
        _ => match status {
            400 | 408 | 413 | 422 => ProviderFailureKind::InvalidRequest,
            401 | 403 => ProviderFailureKind::Authentication,
            402 => ProviderFailureKind::Billing,
            404 => ProviderFailureKind::ModelNotFound,
            429 => ProviderFailureKind::RateLimited,
            502 => ProviderFailureKind::ServerError,
            503 => ProviderFailureKind::Overloaded,
            504 => ProviderFailureKind::Timeout,
            _ => ProviderFailureKind::ServerError,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::{VercelAiGatewayAdapter, classify};
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
            ProviderId::VercelAiGateway,
            "openai/gpt-5.4",
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
            correlation: CorrelationId::from_effect([2; 16]),
            request_hash: ContentHash::of(b"vercel-gateway-golden"),
        }
    }

    #[test]
    fn identity_and_error_namespace_are_gateway_owned() {
        assert_eq!(
            VercelAiGatewayAdapter.provider(),
            ProviderId::VercelAiGateway
        );
        assert_eq!(
            classify(429, Some("rate_limit_exceeded")),
            ProviderFailureKind::RateLimited
        );
        assert_eq!(classify(402, None), ProviderFailureKind::Billing);
    }

    #[test]
    fn request_and_stream_goldens_pin_origin_identity_and_route_metadata() {
        let request = request();
        let wire = VercelAiGatewayAdapter
            .build_request(&request.selection, &request)
            .expect("request builds");
        assert_eq!(
            wire.url().expect("url").as_str(),
            "https://ai-gateway.vercel.sh/v1/chat/completions"
        );
        let body: serde_json::Value = serde_json::from_slice(&wire.body).expect("JSON body");
        assert_eq!(body["model"], "openai/gpt-5.4");
        assert_eq!(body["max_tokens"], 128);
        assert!(body.get("api_key").is_none());

        let mut state = VercelAiGatewayAdapter.new_state(&request.selection);
        let content = br#"{"id":"req-1","object":"chat.completion.chunk","model":"openai/gpt-5.4","provider_metadata":{"gateway":{"provider":"openai"}},"choices":[{"index":0,"delta":{"content":"hello"},"finish_reason":"stop"}]}"#;
        assert_eq!(
            VercelAiGatewayAdapter
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
            VercelAiGatewayAdapter
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
        let sealed = VercelAiGatewayAdapter.finish(state).expect("stream seals");
        let route = sealed.gateway_route.expect("route metadata is bounded");
        assert_eq!(
            route.reported_model.expect("model").as_str(),
            "openai/gpt-5.4"
        );
        assert_eq!(
            route.reported_provider.expect("provider").as_str(),
            "openai"
        );
    }
}
