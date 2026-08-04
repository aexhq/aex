//! The wire request, the closed auth tag, and the send gate (plan 08 §3.2–3.3).
//!
//! # Why a provider module cannot see a credential
//!
//! [`WireRequest`] has no field capable of holding one. Its `auth` is
//! [`AuthScheme`], an enum carrying only a compiled discriminant; the shared
//! core turns that tag into a sensitive header immediately before the single
//! `execute` call and never hands it back. Making leakage from an adapter a
//! compile-time impossibility rather than a review item is D-21.
//!
//! # The send gate
//!
//! [`SendGate`] is `#[must_use]`, constructible only inside this crate, and
//! consumed exactly once at the single send site. A failure raised while the
//! gate is still held is provably `NotSent`; everything after
//! [`SendGate::consume`] is `PossiblySent`, *including* `reqwest` connect
//! errors, because a pooled `HTTP`/2 connection may already have carried the
//! request head and no provider in this set offers a way to ask (D-13).

use aex_model_catalog::document::EndpointPin;
use aex_model_catalog::primitives::BoundedString;
use bytes::Bytes;

/// What a request asks the transport to send as its `Accept`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Accept {
    /// `text/event-stream`. Every launch dialect streams.
    TextEventStream,
}

impl Accept {
    /// The header value.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::TextEventStream => "text/event-stream",
        }
    }
}

/// How the shared core authenticates a request.
///
/// A **tag**, not a value. There is no variant carrying key material and no
/// constructor taking any, which is what makes credential leakage from a
/// provider module a type error rather than a review item.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthScheme {
    /// `Authorization: Bearer <key>` — `openai`, `deepseek`, `zai`,
    /// `moonshotai`.
    BearerAuthorization,
    /// `x-api-key: <key>` plus a pinned `anthropic-version` — `anthropic`.
    AnthropicApiKey {
        /// The pinned API version. Always `2023-06-01` (D-17).
        version: &'static str,
    },
    /// `x-goog-api-key: <key>` — `google`.
    ///
    /// The `?key=` query form is forbidden in this codebase: it would place the
    /// customer credential in a `URL` that reaches proxies, access logs and
    /// error strings (D-15).
    GoogleApiKeyHeader,
}

impl AuthScheme {
    /// The header name the key is written into.
    #[must_use]
    pub const fn header_name(self) -> &'static str {
        match self {
            Self::BearerAuthorization => "authorization",
            Self::AnthropicApiKey { .. } => "x-api-key",
            Self::GoogleApiKeyHeader => "x-goog-api-key",
        }
    }
}

/// A fully built provider request, minus the credential.
///
/// Every field is bounded, and the only field that can hold caller text is the
/// `JSON` `body`. `path` is assembled from an [`EndpointPin`] and a catalog
/// model slug, never from free-form input.
#[derive(Clone, PartialEq, Eq)]
pub struct WireRequest {
    /// The compiled origin.
    pub endpoint: EndpointPin,
    /// The path, built from the endpoint and the model.
    pub path: BoundedString<256>,
    /// Query parameters, from a compiled name set.
    pub query: Vec<(&'static str, BoundedString<64>)>,
    /// Non-secret headers only.
    pub headers: Vec<(&'static str, BoundedString<128>)>,
    /// Which credential the core must attach.
    pub auth: AuthScheme,
    /// The `JSON` body.
    pub body: Bytes,
    /// What to accept.
    pub accept: Accept,
}

impl core::fmt::Debug for WireRequest {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("WireRequest")
            .field("endpoint", &self.endpoint)
            .field("path", &self.path)
            .field("query", &self.query)
            .field("headers", &self.headers)
            .field("auth", &self.auth)
            .field("body_len", &self.body.len())
            .field("accept", &self.accept)
            .finish()
    }
}

impl WireRequest {
    /// The absolute `URL`, with the credential provably absent.
    ///
    /// # Errors
    ///
    /// Returns [`UrlError`] when the assembled `URL` does not parse, which a
    /// compiled origin plus a bounded path cannot normally produce.
    pub fn url(&self) -> Result<url::Url, UrlError> {
        let mut assembled = url::Url::parse(self.endpoint.origin())
            .map_err(|_| UrlError::Origin)?
            .join(self.path.as_str())
            .map_err(|_| UrlError::Path)?;
        if !self.query.is_empty() {
            let mut pairs = assembled.query_pairs_mut();
            for (name, value) in &self.query {
                pairs.append_pair(name, value.as_str());
            }
            drop(pairs);
        }
        Ok(assembled)
    }
}

/// Why `URL` assembly failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum UrlError {
    /// The compiled origin did not parse.
    #[error("the compiled origin did not parse")]
    Origin,
    /// The assembled path did not parse.
    #[error("the assembled path did not parse")]
    Path,
}

/// Permission to send exactly one request.
///
/// `#[must_use]` and constructible only inside this crate. Holding one means
/// nothing has left the process yet; consuming one is the single moment after
/// which `DispatchProof::NotSent` becomes unavailable.
#[derive(Debug)]
#[must_use = "an unconsumed SendGate means the dispatch never decided whether to send"]
pub struct SendGate(());

/// Proof that the single send site was reached.
///
/// Every error carrying one is `PossiblySent`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Dispatched(());

impl SendGate {
    /// Mints the one gate a dispatch attempt is allowed.
    ///
    /// `pub(crate)` on purpose: no provider module and no external caller can
    /// mint a second one, so "at most one request per attempt" is structural
    /// rather than conventional.
    pub(crate) const fn new() -> Self {
        Self(())
    }

    /// Consumes the gate. From here on, every failure is `PossiblySent`.
    #[must_use]
    pub fn consume(self) -> Dispatched {
        Dispatched(())
    }
}

impl Dispatched {
    /// Whether this value exists, which is the whole of its meaning.
    #[must_use]
    pub const fn is_dispatched(self) -> bool {
        true
    }
}

/// Whether a failure happened before or after the gate was consumed.
///
/// The type makes the question total: a dispatch holds either a [`SendGate`] or
/// a [`Dispatched`], never both and never neither.
#[derive(Debug)]
pub enum SendState {
    /// The request has not been handed to the transport.
    Held(SendGate),
    /// The request has been handed to the transport.
    Sent(Dispatched),
}

impl SendState {
    /// A fresh, unsent state.
    #[must_use]
    pub fn new() -> Self {
        Self::Held(SendGate::new())
    }

    /// The dispatch proof this state implies.
    #[must_use]
    pub const fn proof(&self) -> crate::wire_pending::DispatchProof {
        match self {
            Self::Held(_) => crate::wire_pending::DispatchProof::NotSent,
            Self::Sent(_) => crate::wire_pending::DispatchProof::PossiblySent,
        }
    }

    /// Whether the request has left the process.
    #[must_use]
    pub const fn has_sent(&self) -> bool {
        matches!(self, Self::Sent(_))
    }

    /// Consumes the gate, moving to `Sent`.
    ///
    /// # Errors
    ///
    /// Returns [`AlreadySent`] when the gate has already been consumed, which
    /// is the "no second generation" guard: an attempt that has sent cannot
    /// send again.
    pub fn send(&mut self) -> Result<Dispatched, AlreadySent> {
        match core::mem::replace(self, Self::Sent(Dispatched(()))) {
            Self::Held(gate) => Ok(gate.consume()),
            Self::Sent(dispatched) => {
                *self = Self::Sent(dispatched);
                Err(AlreadySent)
            }
        }
    }
}

impl Default for SendState {
    fn default() -> Self {
        Self::new()
    }
}

/// A second send was attempted on one dispatch attempt.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("this attempt has already sent its request; a second generation is not permitted")]
pub struct AlreadySent;

/// Why the single send site could not proceed, or did not survive.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ExecuteError {
    /// Assembly failed while the gate was still held. `NotSent`.
    #[error("the request could not be assembled: {0}")]
    Assembly(#[from] UrlError),
    /// A header this crate builds was rejected. `NotSent`.
    #[error("a request header was rejected before send")]
    Header,
    /// The credential could not be turned into a header. `NotSent`.
    #[error("the credential could not be attached: {0}")]
    Credential(#[from] crate::credential::CredentialResolveError),
    /// The gate had already been consumed. `NotSent` for the *second* attempt,
    /// which is the point: it never reaches the socket.
    #[error("{0}")]
    AlreadySent(#[from] AlreadySent),
    /// The transport failed at or after the send. **`PossiblySent`.**
    ///
    /// This arm deliberately covers `reqwest` connect errors: a pooled `HTTP`/2
    /// connection may already have carried the request head, and no provider in
    /// this set offers a way to ask (D-13).
    #[error("the transport failed after the request was handed over")]
    Transport {
        /// The transport's own message, already bounded and redacted.
        detail: BoundedString<256>,
    },
}

impl ExecuteError {
    /// What this failure proves about whether the provider saw the request.
    #[must_use]
    pub const fn proof(&self) -> crate::wire_pending::DispatchProof {
        match self {
            Self::Transport { .. } => crate::wire_pending::DispatchProof::PossiblySent,
            _ => crate::wire_pending::DispatchProof::NotSent,
        }
    }
}

/// The **single** site in this crate that hands a request to the network.
///
/// Everything before `state.send()` is provably `NotSent`; everything from that
/// line onward is `PossiblySent`. Keeping assembly, header construction and
/// credential attachment above the gate — and the `reqwest` call alone below it
/// — is what makes the proof mechanical rather than a claim.
///
/// # Errors
///
/// Returns [`ExecuteError`]. Use [`ExecuteError::proof`] rather than matching
/// arms: the mapping from arm to proof is defined once, here.
pub async fn execute(
    client: &reqwest::Client,
    request: &WireRequest,
    key: &crate::credential::ProviderApiKey,
    state: &mut SendState,
) -> Result<reqwest::Response, ExecuteError> {
    // ---- everything below is NotSent while the gate is held ----
    let url = request.url()?;
    let mut headers = reqwest::header::HeaderMap::new();
    headers.insert(
        reqwest::header::ACCEPT,
        reqwest::header::HeaderValue::from_static(request.accept.as_str()),
    );
    headers.insert(
        reqwest::header::CONTENT_TYPE,
        reqwest::header::HeaderValue::from_static("application/json"),
    );
    for (name, value) in &request.headers {
        let name = reqwest::header::HeaderName::from_static(name);
        let value = reqwest::header::HeaderValue::from_str(value.as_str())
            .map_err(|_| ExecuteError::Header)?;
        headers.insert(name, value);
    }
    if let AuthScheme::AnthropicApiKey { version } = request.auth {
        headers.insert(
            reqwest::header::HeaderName::from_static("anthropic-version"),
            reqwest::header::HeaderValue::from_str(version).map_err(|_| ExecuteError::Header)?,
        );
    }
    let (name, value) = key.sensitive_header(request.auth)?;
    headers.insert(name, value);

    let built = client.post(url).headers(headers).body(request.body.clone());

    // ---- the gate. Nothing above this line reached a socket. ----
    let _dispatched = state.send()?;

    built.send().await.map_err(|error| ExecuteError::Transport {
        detail: crate::redact::redact(&error.to_string(), &[key.expose_for_redaction()]),
    })
}

#[cfg(test)]
mod tests {
    use aex_model_catalog::document::EndpointPin;
    use aex_model_catalog::primitives::BoundedString;

    use super::{Accept, AuthScheme, SendState, WireRequest};
    use crate::wire_pending::DispatchProof;

    fn request(endpoint: EndpointPin, path: &str, auth: AuthScheme) -> WireRequest {
        WireRequest {
            endpoint,
            path: BoundedString::new(path).expect("path"),
            query: Vec::new(),
            headers: Vec::new(),
            auth,
            body: bytes::Bytes::from_static(b"{}"),
            accept: Accept::TextEventStream,
        }
    }

    #[test]
    fn an_unconsumed_gate_proves_not_sent() {
        let state = SendState::new();
        assert_eq!(state.proof(), DispatchProof::NotSent);
        assert!(!state.has_sent());
    }

    #[test]
    fn a_consumed_gate_proves_possibly_sent() {
        let mut state = SendState::new();
        state.send().expect("first send");
        assert_eq!(state.proof(), DispatchProof::PossiblySent);
        assert!(state.has_sent());
    }

    #[test]
    fn a_second_send_is_refused() {
        let mut state = SendState::new();
        state.send().expect("first send");
        state.send().expect_err("an attempt may not send twice");
        assert_eq!(state.proof(), DispatchProof::PossiblySent);
    }

    #[test]
    fn every_endpoint_origin_is_https_and_compiled() {
        for endpoint in EndpointPin::ALL {
            let origin = endpoint.origin();
            assert!(origin.starts_with("https://"), "{origin} is not HTTPS");
            let parsed = url::Url::parse(origin).expect("a compiled origin parses");
            assert_eq!(parsed.scheme(), "https");
            assert_eq!(parsed.port(), None, "{origin} pins a non-default port");
        }
    }

    #[test]
    fn arbitrary_and_non_launch_origins_are_not_reachable_from_any_pin() {
        let origins: Vec<&str> = EndpointPin::ALL.iter().map(|pin| pin.origin()).collect();
        for excluded in [
            "api.moonshot.cn",
            "platform.kimi.ai",
            "coding/paas",
            "api/anthropic",
            "aiplatform.googleapis.com",
        ] {
            assert!(
                !origins.iter().any(|origin| origin.contains(excluded)),
                "{excluded} is reachable"
            );
        }
    }

    #[test]
    fn a_url_is_assembled_from_the_pinned_origin() {
        let built = request(
            EndpointPin::AnthropicApi,
            "/v1/messages",
            AuthScheme::AnthropicApiKey {
                version: "2023-06-01",
            },
        );
        let url = built.url().expect("assembles");
        assert_eq!(url.as_str(), "https://api.anthropic.com/v1/messages");
    }

    #[test]
    fn a_query_parameter_is_appended_rather_than_interpolated() {
        let mut built = request(
            EndpointPin::GeminiV1Beta,
            "/v1beta/models/gemini-3-pro:streamGenerateContent",
            AuthScheme::GoogleApiKeyHeader,
        );
        built
            .query
            .push(("alt", BoundedString::new("sse").expect("value")));
        let url = built.url().expect("assembles");
        assert!(url.as_str().ends_with("?alt=sse"), "{url}");
        assert!(
            !url.as_str().contains("key="),
            "the ?key= form is forbidden"
        );
    }

    #[test]
    fn a_path_cannot_escape_its_pinned_origin() {
        let built = request(
            EndpointPin::OpenAiApi,
            "/v1/responses",
            AuthScheme::BearerAuthorization,
        );
        let url = built.url().expect("assembles");
        assert_eq!(url.host_str(), Some("api.openai.com"));
    }

    #[test]
    fn each_scheme_names_exactly_one_header() {
        assert_eq!(
            AuthScheme::BearerAuthorization.header_name(),
            "authorization"
        );
        assert_eq!(
            AuthScheme::AnthropicApiKey {
                version: "2023-06-01"
            }
            .header_name(),
            "x-api-key"
        );
        assert_eq!(
            AuthScheme::GoogleApiKeyHeader.header_name(),
            "x-goog-api-key"
        );
    }

    #[test]
    fn a_wire_request_debug_rendering_carries_only_the_auth_tag() {
        let mut built = request(
            EndpointPin::OpenAiApi,
            "/v1/responses",
            AuthScheme::BearerAuthorization,
        );
        built.body =
            bytes::Bytes::from_static(b"{\"input\":\"sk-012345678901234567890123456789\"}");
        let rendered = format!("{built:?}");
        assert!(rendered.contains("BearerAuthorization"));
        assert!(rendered.contains("body_len"));
        // Customer content may itself contain a credential, so body bytes are
        // never part of a diagnostic rendering.
        assert!(!rendered.contains("sk-"));
    }
}
