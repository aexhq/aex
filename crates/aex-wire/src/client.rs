//! The low-level generated client and the transport seam it runs over.
//!
//! `aex-wire` has no HTTP client dependency and never will: the caller supplies
//! a [`Transport`], and the generated [`WireClient`](crate::client::WireClient)
//! turns one operation into one request and one typed answer. `aex-cli`, the
//! dashboard bootstrap and every internal caller therefore share one
//! implementation of the wire rather than three that drift.
//!
//! # Invariants
//!
//! - no client method retries, sleeps, or reads a clock — policy belongs to the
//!   caller, and a library that retried on its behalf would silently duplicate a
//!   non-idempotent admission;
//! - every request the client builds is buildable without executing it, so a
//!   streaming caller can take the request and run its own transport;
//! - a non-success status is decoded into the published error envelope, never
//!   into the success type.

use std::fmt;

pub use crate::generated::client::*;

use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::cursor::Cursor;
use crate::error::{ApiErrorBody, WireError};
use crate::idempotency::IdempotencyKey;
use crate::ids::OperationId;
use crate::ids::{ContentHash, FilePath, ResourceName, SpanId, TraceId};
use crate::limits::LimitId;
use crate::models::ProviderId;
use crate::routes::{RouteId, route};
use crate::types::{Cents, DecimalU128, ETag, HttpMethod, JsonPointer, Region, Timestamp};

/// The `Idempotency-Key` header name.
pub const HEADER_IDEMPOTENCY_KEY: &str = "Idempotency-Key";
/// The `Aex-Operation-Id` header name.
pub const HEADER_OPERATION_ID: &str = "Aex-Operation-Id";
/// The `If-Match` header name.
pub const HEADER_IF_MATCH: &str = "If-Match";
/// The `Accept` header name.
pub const HEADER_ACCEPT: &str = "Accept";
/// The `Content-Type` header name.
pub const HEADER_CONTENT_TYPE: &str = "Content-Type";

/// An `https://` API origin with no trailing slash and no path.
///
/// Parsed rather than accepted as a string, because a base URL carrying a path
/// would silently produce `/api/api/...` for every operation.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct BaseUrl(String);

impl BaseUrl {
    /// Largest accepted byte length.
    pub const MAX_BYTES: usize = 512;

    /// Parses an API origin.
    ///
    /// # Errors
    ///
    /// Returns a [`BaseUrlError`] when the scheme is not `https`, the authority
    /// is empty, a path or query is present, or the value is too long.
    pub fn parse(text: &str) -> Result<Self, BaseUrlError> {
        if text.len() > Self::MAX_BYTES {
            return Err(BaseUrlError::TooLong {
                found: text.len(),
                max: Self::MAX_BYTES,
            });
        }
        let Some(authority) = text.strip_prefix("https://") else {
            return Err(BaseUrlError::NotHttps);
        };
        let authority = authority.strip_suffix('/').unwrap_or(authority);
        if authority.is_empty() {
            return Err(BaseUrlError::EmptyAuthority);
        }
        if authority.contains('/') || authority.contains('?') || authority.contains('#') {
            return Err(BaseUrlError::NotBareOrigin);
        }
        Ok(Self(format!("https://{authority}")))
    }

    /// The origin, without a trailing slash.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for BaseUrl {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

/// Why an API origin was refused.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum BaseUrlError {
    /// The scheme was not `https`.
    #[error("an API origin must be `https://`")]
    NotHttps,
    /// Nothing followed the scheme.
    #[error("an API origin must name a host")]
    EmptyAuthority,
    /// A path, query or fragment was present.
    #[error("an API origin carries no path, query or fragment")]
    NotBareOrigin,
    /// The value was longer than [`BaseUrl::MAX_BYTES`].
    #[error("an API origin is at most {max} bytes, found {found}")]
    TooLong {
        /// The length that was offered.
        found: usize,
        /// The ceiling.
        max: usize,
    },
}

/// One built request, ready for a transport.
///
/// Every field is decided by the route table plus the caller's arguments, so two
/// callers of the same operation cannot produce structurally different requests.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WireRequest {
    /// Which operation this is.
    pub route: RouteId,
    /// The method the route declares.
    pub method: HttpMethod,
    /// The bound path, rooted at `/api`.
    pub path: String,
    /// The encoded query string, without the leading `?`; empty when there is
    /// none.
    pub query: String,
    /// Headers the operation requires, in a fixed order.
    pub headers: Vec<(&'static str, String)>,
    /// The encoded body, when the route declares one.
    pub body: Option<Vec<u8>>,
}

impl WireRequest {
    /// The absolute URL of this request against `base`.
    #[must_use]
    pub fn url(&self, base: &BaseUrl) -> String {
        if self.query.is_empty() {
            format!("{}{}", base.as_str(), self.path)
        } else {
            format!("{}{}?{}", base.as_str(), self.path, self.query)
        }
    }

    /// The header value bound to `name`, if the request carries one.
    #[must_use]
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(bound, _)| *bound == name)
            .map(|(_, value)| value.as_str())
    }
}

/// What a transport returned.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WireResponse {
    /// The HTTP status.
    pub status: u16,
    /// The strong entity tag, when the response carried one.
    pub etag: Option<ETag>,
    /// The response body.
    pub body: Vec<u8>,
}

/// The seam between the generated client and an actual HTTP stack.
///
/// Deliberately one method over owned bytes: the contract crate must not gain a
/// TLS stack, a connection pool or a runtime, and every caller that needs
/// incremental streaming builds the request with the generated `*_request`
/// function and runs it itself.
pub trait Transport: Send + Sync {
    /// Executes one request.
    ///
    /// # Errors
    ///
    /// Returns a [`TransportError`] for anything below the HTTP status: DNS,
    /// connection, TLS, or a body the transport could not read.
    fn execute(
        &self,
        request: WireRequest,
    ) -> impl core::future::Future<Output = Result<WireResponse, TransportError>> + Send;
}

/// A failure below the HTTP status line.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("transport failure: {reason}")]
pub struct TransportError {
    /// What the transport reported.
    pub reason: String,
}

impl TransportError {
    /// A failure naming its own reason.
    #[must_use]
    pub fn new(reason: impl Into<String>) -> Self {
        Self {
            reason: reason.into(),
        }
    }
}

/// Why a client call did not produce its typed answer.
#[derive(Debug, thiserror::Error)]
pub enum ClientError {
    /// The service answered with the published error envelope.
    #[error("`{route}` answered {status}: {}", .error.message)]
    Api {
        /// The HTTP status.
        status: u16,
        /// The decoded envelope.
        error: Box<ApiErrorBody>,
        /// Which operation was called.
        route: RouteId,
    },
    /// The request never reached a status.
    #[error(transparent)]
    Transport(#[from] TransportError),
    /// The answer did not match the contract.
    #[error("`{route}` answered a body this operation does not declare at {pointer}: {source}")]
    Decode {
        /// Which operation was called.
        route: RouteId,
        /// Where the body diverged.
        pointer: JsonPointer,
        /// What the decoder reported.
        source: serde_json::Error,
    },
    /// A value the caller supplied cannot be sent.
    #[error("`{route}` could not encode its request: {reason}")]
    Encode {
        /// Which operation was called.
        route: RouteId,
        /// What the encoder reported.
        reason: String,
    },
}

impl ClientError {
    /// The wire error code, when the service produced one this client knows.
    #[must_use]
    pub fn code(&self) -> Option<crate::error::ErrorCode> {
        match self {
            Self::Api { error, .. } => match error.code {
                crate::error::ObservedErrorCode::Known(code) => Some(code),
                crate::error::ObservedErrorCode::Unrecognized(_) => None,
            },
            Self::Transport(_) | Self::Decode { .. } | Self::Encode { .. } => None,
        }
    }
}

/// The low-level client: one method per operation over an injected transport.
#[derive(Debug, Clone)]
pub struct WireClient<T: Transport> {
    /// The caller's HTTP stack.
    transport: T,
    /// The plane origin every request is built against.
    base: BaseUrl,
}

impl<T: Transport> WireClient<T> {
    /// A client for `base` over `transport`.
    pub const fn new(transport: T, base: BaseUrl) -> Self {
        Self { transport, base }
    }

    /// The origin this client calls.
    #[must_use]
    pub const fn base(&self) -> &BaseUrl {
        &self.base
    }

    /// The injected transport.
    #[must_use]
    pub const fn transport(&self) -> &T {
        &self.transport
    }

    /// Executes a built request.
    ///
    /// # Errors
    ///
    /// Returns [`ClientError::Transport`] when the request never reached a
    /// status.
    pub async fn send(&self, request: WireRequest) -> Result<WireResponse, ClientError> {
        Ok(self.transport.execute(request).await?)
    }
}

/// Encodes a request body.
///
/// # Errors
///
/// Returns [`ClientError::Encode`] when the value will not serialize.
pub fn encode_body<T: Serialize>(route: RouteId, value: &T) -> Result<Vec<u8>, ClientError> {
    serde_json::to_vec(value).map_err(|reason| ClientError::Encode {
        route,
        reason: reason.to_string(),
    })
}

/// Decodes a typed success body, or the published error envelope.
///
/// # Errors
///
/// Returns [`ClientError::Api`] for any status other than the one the route
/// declares, and [`ClientError::Decode`] when the success body does not match.
pub fn decode_response<T: DeserializeOwned>(
    route: RouteId,
    response: &WireResponse,
) -> Result<T, ClientError> {
    let expected = crate::routes::route(route).success_status;
    if response.status != expected {
        return Err(decode_api_error(route, response));
    }
    serde_json::from_slice(&response.body).map_err(|source| ClientError::Decode {
        route,
        pointer: JsonPointer::root(),
        source,
    })
}

/// Decodes a typed success body together with its strong entity tag.
///
/// # Errors
///
/// Returns [`ClientError::Api`] for an unexpected status, and
/// [`ClientError::Decode`] when the body does not match or the entity tag the
/// route promises is absent.
pub fn decode_response_with_etag<T: DeserializeOwned>(
    route: RouteId,
    response: &WireResponse,
) -> Result<crate::server::WithETag<T>, ClientError> {
    let value: T = decode_response(route, response)?;
    let etag = response.etag.clone().ok_or_else(|| ClientError::Encode {
        route,
        reason: "the response carried no `ETag`, which this operation declares".to_owned(),
    })?;
    Ok(crate::server::WithETag { value, etag })
}

/// Accepts a bodyless success.
///
/// # Errors
///
/// Returns [`ClientError::Api`] for any status other than the declared one.
pub fn decode_no_content(route: RouteId, response: &WireResponse) -> Result<(), ClientError> {
    let expected = crate::routes::route(route).success_status;
    if response.status != expected {
        return Err(decode_api_error(route, response));
    }
    Ok(())
}

/// A buffered `application/x-ndjson` answer.
///
/// The frames are decoded lazily, one line at a time, so a malformed frame names
/// itself rather than discarding the page.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NdjsonFrames<F> {
    /// The raw body.
    body: Vec<u8>,
    /// The frame type this body carries.
    frame: core::marker::PhantomData<fn() -> F>,
}

impl<F: DeserializeOwned> NdjsonFrames<F> {
    /// Every frame, in arrival order.
    ///
    /// # Errors
    ///
    /// Each item is `Err` when that one line does not decode.
    pub fn frames(&self) -> impl Iterator<Item = Result<F, serde_json::Error>> + '_ {
        self.body
            .split(|byte| *byte == b'\n')
            .filter(|line| !line.iter().all(u8::is_ascii_whitespace))
            .map(serde_json::from_slice::<F>)
    }

    /// The raw body.
    #[must_use]
    pub fn as_bytes(&self) -> &[u8] {
        &self.body
    }
}

/// Accepts a frame-stream answer.
///
/// # Errors
///
/// Returns [`ClientError::Api`] for any status other than the declared one.
pub fn decode_ndjson<F>(
    route: RouteId,
    response: WireResponse,
) -> Result<NdjsonFrames<F>, ClientError> {
    let expected = crate::routes::route(route).success_status;
    if response.status != expected {
        return Err(decode_api_error(route, &response));
    }
    Ok(NdjsonFrames {
        body: response.body,
        frame: core::marker::PhantomData,
    })
}

/// Turns a non-success answer into the published envelope.
///
/// A body that is not the envelope is still a failure, never a success: the
/// status already said so, and inventing a code would hide an unreachable
/// service behind a decode error.
fn decode_api_error(route: RouteId, response: &WireResponse) -> ClientError {
    match serde_json::from_slice::<crate::error::ApiError>(&response.body) {
        Ok(envelope) => ClientError::Api {
            status: response.status,
            error: Box::new(envelope.error),
            route,
        },
        Err(source) => ClientError::Decode {
            route,
            pointer: JsonPointer::root(),
            source,
        },
    }
}

/// Renders the `Idempotency-Key` header of a route that requires one.
#[must_use]
pub fn idempotency_header(key: &IdempotencyKey) -> (&'static str, String) {
    (HEADER_IDEMPOTENCY_KEY, key.as_str().to_owned())
}

/// Renders the `Aex-Operation-Id` header of a route that requires one.
#[must_use]
pub fn operation_header(operation_id: OperationId) -> (&'static str, String) {
    (HEADER_OPERATION_ID, operation_id.to_string())
}

/// Renders the `If-Match` header a route accepts or requires.
#[must_use]
pub fn if_match_header(etag: &ETag) -> (&'static str, String) {
    (HEADER_IF_MATCH, etag.as_str().to_owned())
}

/// The `Accept` header for `route`, decided by its declared transport.
#[must_use]
pub fn accept_header(id: RouteId) -> (&'static str, String) {
    let media = match route(id).transport {
        crate::routes::TransportKind::Unary => crate::dispatch::CONTENT_TYPE_JSON,
        crate::routes::TransportKind::Ndjson => crate::dispatch::CONTENT_TYPE_NDJSON,
    };
    (HEADER_ACCEPT, media.to_owned())
}

/// The `Content-Type` header of a request that carries a body.
#[must_use]
pub fn content_type_header() -> (&'static str, String) {
    (
        HEADER_CONTENT_TYPE,
        crate::dispatch::CONTENT_TYPE_JSON.to_owned(),
    )
}

// ---------------------------------------------------------------------------
// Parameter encoding
// ---------------------------------------------------------------------------

/// A value the contract admits in a path segment or a query value.
///
/// The mirror of [`FromParam`](crate::dispatch::FromParam): every type that can
/// be decoded from a parameter can be encoded back into one, and a round-trip
/// property test holds the two together.
pub trait ToParam {
    /// The wire spelling of this value.
    fn to_param(&self) -> String;
}

/// Implements [`ToParam`] through `core::fmt::Display`.
macro_rules! to_param_via_display {
    ($($ty:ty),* $(,)?) => {
        $(impl ToParam for $ty {
            fn to_param(&self) -> String {
                self.to_string()
            }
        })*
    };
}

to_param_via_display!(
    Cents,
    ContentHash,
    DecimalU128,
    ETag,
    FilePath,
    LimitId,
    ProviderId,
    ResourceName,
    SpanId,
    Timestamp,
    TraceId,
    bool,
    i64,
    u32,
    u64,
);

impl ToParam for String {
    fn to_param(&self) -> String {
        self.clone()
    }
}

impl ToParam for Cursor {
    fn to_param(&self) -> String {
        self.as_str().to_owned()
    }
}

impl ToParam for Region {
    fn to_param(&self) -> String {
        self.as_str().to_owned()
    }
}

/// The headers `id` requires, in a fixed order.
///
/// Header policy lives here rather than in 146 generated call sites: `Accept`
/// follows the declared transport, `Content-Type` follows the declared body
/// class, and the three identity headers are present exactly when the route
/// declares them.
#[must_use]
pub fn request_headers(
    id: RouteId,
    idempotency_key: Option<&IdempotencyKey>,
    operation_id: Option<OperationId>,
    if_match: Option<&ETag>,
) -> Vec<(&'static str, String)> {
    let mut headers = vec![accept_header(id)];
    if route(id).body_class != crate::routes::BodyClass::None {
        headers.push(content_type_header());
    }
    if let Some(key) = idempotency_key {
        headers.push(idempotency_header(key));
    }
    if let Some(operation_id) = operation_id {
        headers.push(operation_header(operation_id));
    }
    if let Some(etag) = if_match {
        headers.push(if_match_header(etag));
    }
    headers
}

/// A path template with its parameters bound in template order.
///
/// The template comes from the one route table, so a client can never call a
/// path the server does not serve, and every segment is percent-encoded so a
/// value cannot change the shape of the path it sits in.
#[derive(Debug, Clone)]
pub struct PathWriter {
    /// The route whose template is being bound.
    id: RouteId,
    /// Bound values, in template order.
    bound: Vec<String>,
}

impl PathWriter {
    /// A writer over the template of `id`.
    #[must_use]
    pub const fn new(id: RouteId) -> Self {
        Self {
            id,
            bound: Vec::new(),
        }
    }

    /// Binds the next path parameter.
    pub fn bind<T: ToParam + ?Sized>(&mut self, value: &T) {
        self.bound.push(value.to_param());
    }

    /// The bound path.
    ///
    /// # Errors
    ///
    /// Returns [`ClientError::Encode`] when the number of bound values is not
    /// the number the template declares. That is a generator defect rather than
    /// caller input, and it fails rather than emitting a path with a literal
    /// `{sessionId}` in it.
    pub fn finish(self) -> Result<String, ClientError> {
        let descriptor = route(self.id);
        if self.bound.len() != descriptor.path_params.len() {
            return Err(ClientError::Encode {
                route: self.id,
                reason: format!(
                    "the template binds {} parameters, {} were supplied",
                    descriptor.path_params.len(),
                    self.bound.len()
                ),
            });
        }
        let mut path = String::with_capacity(descriptor.template.len() + 32);
        let mut next = self.bound.iter();
        for (index, segment) in descriptor.template.split('/').enumerate() {
            if index > 0 {
                path.push('/');
            }
            if segment.starts_with('{') && segment.ends_with('}') {
                let value = next.next().ok_or_else(|| ClientError::Encode {
                    route: self.id,
                    reason: "a path parameter was not bound".to_owned(),
                })?;
                path.push_str(&percent_encode(value));
            } else {
                path.push_str(segment);
            }
        }
        Ok(path)
    }
}

/// A growing `application/x-www-form-urlencoded` query string.
#[derive(Debug, Clone, Default)]
pub struct QueryWriter {
    /// The text so far, without a leading `?`.
    text: String,
}

impl QueryWriter {
    /// An empty query.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            text: String::new(),
        }
    }

    /// Appends one required parameter.
    pub fn put<T: ToParam + ?Sized>(&mut self, name: &str, value: &T) {
        if !self.text.is_empty() {
            self.text.push('&');
        }
        self.text.push_str(&percent_encode(name));
        self.text.push('=');
        self.text.push_str(&percent_encode(&value.to_param()));
    }

    /// Appends one optional parameter, omitting it when absent.
    pub fn put_option<T: ToParam>(&mut self, name: &str, value: Option<&T>) {
        if let Some(value) = value {
            self.put(name, value);
        }
    }

    /// The finished query string, without a leading `?`.
    #[must_use]
    pub fn finish(self) -> String {
        self.text
    }
}

/// Percent-encodes one token.
///
/// Conservative on purpose: everything outside the RFC 3986 unreserved set is
/// escaped, so a value can never change the shape of the query it sits in.
#[must_use]
pub fn percent_encode(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for byte in text.as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                out.push(char::from(*byte));
            }
            other => {
                out.push('%');
                out.push(char::from(upper_hex(other >> 4)));
                out.push(char::from(upper_hex(other & 0x0f)));
            }
        }
    }
    out
}

/// One nibble as an uppercase hexadecimal digit.
const fn upper_hex(nibble: u8) -> u8 {
    if nibble < 10 {
        b'0' + nibble
    } else {
        b'A' + (nibble - 10)
    }
}

/// A `WireError` a caller can raise without owning the server vocabulary.
///
/// Present so a transport-side guard (an oversize response, a refused redirect)
/// can name a wire code rather than inventing one.
#[must_use]
pub fn client_side(code: crate::error::ErrorCode, reason: impl Into<String>) -> WireError {
    WireError::new(code).with_message(reason)
}
