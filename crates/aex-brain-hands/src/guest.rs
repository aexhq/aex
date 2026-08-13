//! Authenticated, bounded HTTP transport from Brain to one exact Hands generation.
//!
//! The AWS endpoint token stays in trusted memory and is attached only after the
//! endpoint URL and bounded JSON request have been validated. Responses are
//! streamed into the same exact ceiling even when no content length is present.

use core::time::Duration;

use aex_brain_app::ports::{HandsError, ProviderFailureKind, RedactedDetail};
use aex_brain_domain::effect::{DispatchProof, DispatchStage};
use aex_hands_control_aws::{AGENT_PORT, EndpointToken};
use aex_hands_protocol::rpc::{
    Fence, GenerationBinding, GuestRequest, GuestResponse, MAX_GUEST_BODY_BYTES, PROTOCOL_V1, Verb,
};
use aex_wire::ids::GenerationId;
use reqwest::header::{ACCEPT, ACCEPT_ENCODING, CONTENT_TYPE, HeaderValue};
use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::CONNECTION_IDLE_MS;

const AUTH_HEADER: &str = "X-aws-proxy-auth";
const PORT_HEADER: &str = "X-aws-proxy-port";
const JSON_MEDIA_TYPE: &str = "application/json";
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);

/// One provider endpoint and memory-only token bound to an exact generation fence.
#[derive(Clone)]
pub struct AuthenticatedGuestEndpoint {
    /// The generation the endpoint must serve.
    pub generation: GenerationId,
    /// The lowest fence Brain will accept back.
    pub fence: Fence,
    endpoint: String,
    token: EndpointToken,
}

impl AuthenticatedGuestEndpoint {
    /// Binds provider endpoint evidence to one exact generation.
    ///
    /// # Errors
    ///
    /// Returns a pre-dispatch error when the token is not scoped to exactly the
    /// Hands agent port. URL syntax is checked when a verb is constructed, before
    /// the token is attached to an HTTP request.
    pub fn new(
        generation: GenerationId,
        fence: Fence,
        endpoint: impl Into<String>,
        token: EndpointToken,
    ) -> Result<Self, HandsError> {
        if token.allowed_ports != [AGENT_PORT] {
            return Err(pre_dispatch(
                ProviderFailureKind::Authentication,
                "the guest token is not scoped to exactly the Hands agent port",
            ));
        }
        Ok(Self {
            generation,
            fence,
            endpoint: endpoint.into(),
            token,
        })
    }

    /// The provider endpoint address, without its memory-only credential.
    #[must_use]
    pub fn address(&self) -> &str {
        &self.endpoint
    }

    /// When the memory-only endpoint credential expires.
    #[must_use]
    pub const fn lease_expires_at(&self) -> aex_wire::types::Timestamp {
        self.token.expires_at
    }
}

impl core::fmt::Debug for AuthenticatedGuestEndpoint {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("AuthenticatedGuestEndpoint")
            .field("generation", &self.generation)
            .field("fence", &self.fence)
            .field("endpoint", &self.endpoint)
            .field("token", &self.token)
            .finish()
    }
}

/// Reusable HTTPS/HTTP2 client for authenticated Hands guest RPC.
///
/// One instance is intended per Brain process. `reqwest` negotiates HTTP/2 over
/// ALPN and falls back to HTTP/1.1; its pool is retained only for the bounded
/// idle window. Durable runtime state and tenant identity do not live here.
#[derive(Clone)]
pub struct HttpGuestTransport {
    client: reqwest::Client,
}

impl HttpGuestTransport {
    /// Builds the reusable, redirect-free client.
    ///
    /// # Errors
    ///
    /// Returns a pre-dispatch error when the TLS client cannot be constructed.
    pub fn new() -> Result<Self, HandsError> {
        let client = reqwest::Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(CONNECT_TIMEOUT)
            .pool_idle_timeout(Duration::from_millis(CONNECTION_IDLE_MS))
            .no_gzip()
            .no_brotli()
            .build()
            .map_err(|_| {
                pre_dispatch(
                    ProviderFailureKind::Transport,
                    "the Hands HTTPS client could not be constructed",
                )
            })?;
        Ok(Self { client })
    }

    /// Sends one typed request and returns one typed, exact-generation reply.
    ///
    /// Request construction failures prove `NotSent`. Once execution starts, a
    /// transport failure is conservatively `PossiblySent`; after a response head
    /// arrives, body and protocol failures are `ResponseStarted`.
    ///
    /// # Errors
    ///
    /// Returns [`HandsError::Transport`] for request construction, transport,
    /// HTTP status, body-bound, or typed-payload failures.
    pub async fn call<Request, Response>(
        &self,
        endpoint: &AuthenticatedGuestEndpoint,
        verb: Verb,
        request: &Request,
        timeout: Duration,
    ) -> Result<Response, HandsError>
    where
        Request: Serialize + ?Sized,
        Response: DeserializeOwned,
    {
        let request = self.build_request(endpoint, verb, request, timeout)?;
        let response = self.client.execute(request).await.map_err(|_| {
            dispatched(
                ProviderFailureKind::Transport,
                "the Hands HTTPS request failed before a response head arrived",
            )
        })?;
        let status = response.status();
        if !status.is_success() {
            return Err(http_status(status.as_u16()));
        }
        let bytes = read_bounded_response(response).await?;
        decode_reply(&bytes, endpoint)
    }

    fn build_request<Request: Serialize + ?Sized>(
        &self,
        endpoint: &AuthenticatedGuestEndpoint,
        verb: Verb,
        request: &Request,
        timeout: Duration,
    ) -> Result<reqwest::Request, HandsError> {
        let envelope = GuestRequest {
            binding: GenerationBinding {
                schema_version: PROTOCOL_V1,
                generation: endpoint.generation,
                fence: endpoint.fence,
            },
            request,
        };
        let payload = serde_json::to_vec(&envelope).map_err(|_| {
            pre_dispatch(
                ProviderFailureKind::InvalidRequest,
                "the Hands request could not be encoded",
            )
        })?;
        if payload.len() > MAX_GUEST_BODY_BYTES {
            return Err(pre_dispatch(
                ProviderFailureKind::InvalidRequest,
                "the Hands request exceeds the JSON body ceiling",
            ));
        }
        let url = verb_url(&endpoint.endpoint, verb)?;
        let auth = HeaderValue::from_str(endpoint.token.expose()).map_err(|_| {
            pre_dispatch(
                ProviderFailureKind::Authentication,
                "the Hands endpoint token is not a valid HTTP header value",
            )
        })?;
        self.client
            .post(url)
            .header(AUTH_HEADER, auth)
            .header(PORT_HEADER, AGENT_PORT)
            .header(CONTENT_TYPE, JSON_MEDIA_TYPE)
            .header(ACCEPT, JSON_MEDIA_TYPE)
            .header(ACCEPT_ENCODING, "identity")
            .timeout(timeout)
            .body(payload)
            .build()
            .map_err(|_| {
                pre_dispatch(
                    ProviderFailureKind::InvalidRequest,
                    "the Hands HTTPS request could not be constructed",
                )
            })
    }
}

impl core::fmt::Debug for HttpGuestTransport {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("HttpGuestTransport")
            .finish_non_exhaustive()
    }
}

fn verb_url(endpoint: &str, verb: Verb) -> Result<reqwest::Url, HandsError> {
    let endpoint = endpoint.trim();
    let rendered = if endpoint.contains("://") {
        endpoint.to_owned()
    } else {
        format!("https://{endpoint}")
    };
    let mut url = reqwest::Url::parse(&rendered).map_err(|_| {
        pre_dispatch(
            ProviderFailureKind::InvalidRequest,
            "the provider returned a malformed Hands endpoint",
        )
    })?;
    let clean_root = url.path().is_empty() || url.path() == "/";
    if url.scheme() != "https"
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.port().is_some_and(|port| port != 443)
        || !clean_root
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(pre_dispatch(
            ProviderFailureKind::InvalidRequest,
            "the provider returned a non-canonical HTTPS Hands endpoint",
        ));
    }
    url.set_path(verb.path());
    Ok(url)
}

async fn read_bounded_response(mut response: reqwest::Response) -> Result<Vec<u8>, HandsError> {
    if response
        .content_length()
        .is_some_and(|length| length > MAX_GUEST_BODY_BYTES as u64)
    {
        return Err(response_started(
            DispatchStage::Terminal,
            ProviderFailureKind::ProtocolViolation,
            "the Hands response content length exceeds the JSON body ceiling",
        ));
    }

    let capacity = response
        .content_length()
        .and_then(|length| usize::try_from(length).ok())
        .unwrap_or(0)
        .min(MAX_GUEST_BODY_BYTES);
    let mut bytes = Vec::with_capacity(capacity);
    while let Some(chunk) = response.chunk().await.map_err(|_| {
        response_started(
            DispatchStage::Streaming,
            ProviderFailureKind::Transport,
            "the Hands response stream failed",
        )
    })? {
        if bytes.len().saturating_add(chunk.len()) > MAX_GUEST_BODY_BYTES {
            return Err(response_started(
                DispatchStage::Terminal,
                ProviderFailureKind::ProtocolViolation,
                "the Hands response exceeds the JSON body ceiling",
            ));
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}

fn decode_reply<Response: DeserializeOwned>(
    bytes: &[u8],
    endpoint: &AuthenticatedGuestEndpoint,
) -> Result<Response, HandsError> {
    let envelope: GuestResponse<Response> = serde_json::from_slice(bytes).map_err(|_| {
        response_started(
            DispatchStage::Terminal,
            ProviderFailureKind::ProtocolViolation,
            "the Hands response payload was not the expected type",
        )
    })?;
    if envelope.binding.schema_version != PROTOCOL_V1 {
        return Err(response_started(
            DispatchStage::Terminal,
            ProviderFailureKind::ProtocolViolation,
            "the Hands response used an unsupported protocol version",
        ));
    }
    if envelope.binding.generation != endpoint.generation {
        return Err(response_started(
            DispatchStage::Terminal,
            ProviderFailureKind::ProtocolViolation,
            "the Hands response named another generation",
        ));
    }
    if envelope.binding.fence < endpoint.fence {
        return Err(response_started(
            DispatchStage::Terminal,
            ProviderFailureKind::ProtocolViolation,
            "the Hands response fence is stale",
        ));
    }
    Ok(envelope.response)
}

fn pre_dispatch(kind: ProviderFailureKind, message: &str) -> HandsError {
    transport(
        DispatchStage::PreDispatch,
        DispatchProof::NotSent,
        kind,
        message,
        None,
    )
}

fn dispatched(kind: ProviderFailureKind, message: &str) -> HandsError {
    transport(
        DispatchStage::Dispatched,
        DispatchProof::PossiblySent,
        kind,
        message,
        None,
    )
}

fn response_started(stage: DispatchStage, kind: ProviderFailureKind, message: &str) -> HandsError {
    transport(stage, DispatchProof::ResponseStarted, kind, message, None)
}

fn http_status(status: u16) -> HandsError {
    let kind = match status {
        400 => ProviderFailureKind::InvalidRequest,
        401 | 403 => ProviderFailureKind::Authentication,
        429 => ProviderFailureKind::RateLimited,
        500..=599 => ProviderFailureKind::ServerError,
        _ => ProviderFailureKind::ProtocolViolation,
    };
    transport(
        DispatchStage::Terminal,
        DispatchProof::ResponseStarted,
        kind,
        "the Hands endpoint returned a non-success status",
        Some(status),
    )
}

fn transport(
    stage: DispatchStage,
    proof: DispatchProof,
    kind: ProviderFailureKind,
    message: &str,
    status: Option<u16>,
) -> HandsError {
    let mut detail = RedactedDetail::internal(kind, message);
    detail.http_status = status;
    HandsError::Transport {
        stage,
        proof,
        detail,
    }
}

#[cfg(test)]
mod tests {
    use super::{AuthenticatedGuestEndpoint, HttpGuestTransport, decode_reply};
    use aex_brain_app::ports::HandsError;
    use aex_brain_domain::effect::{DispatchProof, DispatchStage};
    use aex_hands_control_aws::EndpointToken;
    use aex_hands_protocol::rpc::{
        Fence, GenerationBinding, GuestRequest, GuestResponse, PROTOCOL_V1, Verb,
    };
    use aex_wire::ids::{GenerationId, PrefixedId as _, Uuid7};
    use aex_wire::types::Timestamp;
    use serde_json::{Value, json};

    fn generation(seed: u8) -> GenerationId {
        GenerationId::from_uuid7(Uuid7::compose(1, [seed; 10]))
    }

    fn endpoint() -> AuthenticatedGuestEndpoint {
        AuthenticatedGuestEndpoint::new(
            generation(1),
            Fence(4),
            "guest.lambda-microvm.eu-west-1.on.aws",
            EndpointToken::new(
                "fixture-token",
                Timestamp::from_unix_millis(60_000).expect("timestamp"),
            ),
        )
        .expect("endpoint")
    }

    #[test]
    fn the_request_is_bounded_json_authenticated_and_port_scoped() {
        let transport = HttpGuestTransport::new().expect("transport");
        let endpoint = endpoint();
        let request = transport
            .build_request(
                &endpoint,
                Verb::Status,
                &json!({"operation":"fixture"}),
                core::time::Duration::from_secs(3),
            )
            .expect("request");
        assert_eq!(
            request.url().as_str(),
            "https://guest.lambda-microvm.eu-west-1.on.aws/aex/hands/v1/status"
        );
        assert_eq!(
            request
                .headers()
                .get("X-aws-proxy-auth")
                .and_then(|value| value.to_str().ok()),
            Some("fixture-token")
        );
        assert_eq!(
            request
                .headers()
                .get("X-aws-proxy-port")
                .and_then(|value| value.to_str().ok()),
            Some("8080")
        );
        assert_eq!(
            request
                .headers()
                .get("content-type")
                .and_then(|value| value.to_str().ok()),
            Some("application/json")
        );
        let body = request
            .body()
            .and_then(reqwest::Body::as_bytes)
            .expect("in-memory body");
        let envelope: GuestRequest<Value> = serde_json::from_slice(body).expect("JSON envelope");
        assert_eq!(envelope.binding.schema_version, PROTOCOL_V1);
        assert_eq!(envelope.binding.generation, endpoint.generation);
        assert_eq!(envelope.binding.fence, endpoint.fence);
        assert_eq!(envelope.request, json!({"operation":"fixture"}));
    }

    #[test]
    fn endpoint_validation_happens_before_the_token_can_be_dispatched() {
        let transport = HttpGuestTransport::new().expect("transport");
        let mut endpoint = endpoint();
        endpoint.endpoint = "http://attacker.invalid/collect".to_owned();
        assert!(matches!(
            transport.build_request(
                &endpoint,
                Verb::Status,
                &json!({}),
                core::time::Duration::from_secs(1),
            ),
            Err(HandsError::Transport {
                stage: DispatchStage::PreDispatch,
                proof: DispatchProof::NotSent,
                ..
            })
        ));
        assert!(!format!("{endpoint:?}").contains("fixture-token"));
    }

    #[test]
    fn a_typed_json_reply_decodes_and_malformed_json_is_refused() {
        let endpoint = endpoint();
        let payload = serde_json::to_vec(&GuestResponse {
            binding: GenerationBinding {
                schema_version: PROTOCOL_V1,
                generation: endpoint.generation,
                fence: endpoint.fence,
            },
            response: json!({"status":"ok"}),
        })
        .expect("json");
        let reply = decode_reply::<Value>(&payload, &endpoint).expect("reply");
        assert_eq!(reply, json!({"status":"ok"}));
        assert!(matches!(
            decode_reply::<Value>(b"not-json", &endpoint),
            Err(HandsError::Transport {
                stage: DispatchStage::Terminal,
                proof: DispatchProof::ResponseStarted,
                ..
            })
        ));
    }

    #[test]
    fn a_foreign_generation_or_stale_response_fence_is_refused() {
        let endpoint = endpoint();
        for binding in [
            GenerationBinding {
                schema_version: PROTOCOL_V1,
                generation: generation(2),
                fence: endpoint.fence,
            },
            GenerationBinding {
                schema_version: PROTOCOL_V1,
                generation: endpoint.generation,
                fence: Fence(endpoint.fence.0 - 1),
            },
        ] {
            let payload = serde_json::to_vec(&GuestResponse {
                binding,
                response: json!({"status":"ok"}),
            })
            .expect("json");
            assert!(matches!(
                decode_reply::<Value>(&payload, &endpoint),
                Err(HandsError::Transport {
                    stage: DispatchStage::Terminal,
                    proof: DispatchProof::ResponseStarted,
                    ..
                })
            ));
        }
    }
}
