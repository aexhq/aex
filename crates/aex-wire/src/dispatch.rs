//! The framework-free dispatch surface.
//!
//! A composition crate matches a request with [`match_route`](crate::routes::match_route),
//! fills a [`RawRequest`], and calls the generated `dispatch_*` for the route's
//! group. Everything between the raw bytes and the typed handler — strict body
//! decoding, strict query decoding, the replay identity, response encoding — is
//! generated from the one route table, so mounting a route is mechanical and
//! total rather than 146 hand-written adapters that can each be wrong
//! differently.
//!
//! Nothing here knows about `axum`, `hyper` or `lambda_http`. The composition
//! crate owns the framework; this module owns the contract.

use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::canonical::{intent_digest, to_jcs_bytes};
use crate::cursor::Cursor;
use crate::error::{ErrorCode, ErrorDetails, WireError, WireResult};
use crate::idempotency::{IdempotencyKind, IntentDigest, OperationIdentity, ReplayIdentity};
use crate::ids::{ContentHash, FilePath, ResourceName, SpanId, TraceId};
use crate::limits::LimitId;
use crate::models::{ErrorDetailsValidation, ProviderId};
use crate::routes::{PathBinding, RouteId, route};
use crate::server::{Accepted, NdjsonStream, RequestContext};
use crate::types::{Cents, DecimalU128, ETag, JsonPointer, Region, Timestamp};

/// The raw pieces of a matched request, before any typed decoding.
///
/// The body is borrowed, never owned: a Lambda host already holds the bytes and
/// a streaming host has already bounded them, so copying them here would only
/// hide where the bound was applied.
#[derive(Debug, Clone, Copy)]
pub struct RawRequest<'a> {
    /// The route [`match_route`](crate::routes::match_route) resolved.
    pub route: RouteId,
    /// The bound path parameters, in template order.
    pub path: PathBinding<'a>,
    /// The raw query string, without the leading `?`.
    pub query: &'a str,
    /// The raw request body; empty when the route declares none.
    pub body: &'a [u8],
}

/// The byte bounds a decoder enforces before it parses anything.
///
/// Each body class states its own fixed envelope ceiling.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RequestLimits {
    /// The ceiling for a `BodyClass::AexJson` body, in encoded bytes.
    pub max_json_body_bytes: usize,
    /// The ceiling for a `BodyClass::Otlp` body, in encoded bytes.
    pub max_otlp_body_bytes: usize,
}

impl RequestLimits {
    /// The `api.json_body` default: 65,536 encoded bytes.
    pub const DEFAULT_JSON_BODY_BYTES: usize = 65_536;
    /// The `api.otlp_body` default: 4 MiB.
    pub const DEFAULT_OTLP_BODY_BYTES: usize = 4 * 1024 * 1024;
    /// The live-file logical part ceiling: 4 MiB.
    pub const DEFAULT_BINARY_BODY_BYTES: usize = 4 * 1024 * 1024;

    /// The registry defaults.
    ///
    /// A workspace whose effective limits are higher supplies its own value;
    /// there is deliberately no way to dispatch without stating a bound.
    pub const DEFAULT: Self = Self {
        max_json_body_bytes: Self::DEFAULT_JSON_BODY_BYTES,
        max_otlp_body_bytes: Self::DEFAULT_OTLP_BODY_BYTES,
    };
}

/// The `Content-Type` of a rendered unary response.
pub const CONTENT_TYPE_JSON: &str = "application/json";
/// The `Content-Type` of a rendered frame stream.
pub const CONTENT_TYPE_NDJSON: &str = "application/x-ndjson";
/// The content type of a bounded opaque byte body.
pub const CONTENT_TYPE_BINARY: &str = "application/octet-stream";

/// A rendered unary response: the status the route declares, its headers, and
/// its encoded body.
///
/// A handler cannot choose a status, because it never returns one: the response
/// *type* it returns fixes the status through the route table.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawResponse {
    /// The HTTP status.
    pub status: u16,
    /// The media type, absent only for `204 No Content`.
    pub content_type: Option<&'static str>,
    /// The strong entity tag, when the route returns one.
    pub etag: Option<ETag>,
    /// The `Location` header, set on every `202 Accepted` admission.
    pub location: Option<String>,
    /// The encoded body; empty for `204 No Content`.
    pub body: Vec<u8>,
}

impl RawResponse {
    /// A JSON response at `status`.
    ///
    /// # Errors
    ///
    /// Returns [`ErrorCode::InternalError`] when the value will not serialize.
    /// That is a defect in the handler's own type, never caller input, so it is
    /// deliberately not a `400`.
    pub fn json<T: Serialize>(status: u16, value: &T) -> WireResult<Self> {
        let body = serde_json::to_vec(value).map_err(|reason| {
            WireError::new(ErrorCode::InternalError)
                .with_message(format!("response encoding failed: {reason}"))
        })?;
        Ok(Self {
            status,
            content_type: Some(CONTENT_TYPE_JSON),
            etag: None,
            location: None,
            body,
        })
    }

    /// One bounded opaque byte response.
    #[must_use]
    pub fn binary(status: u16, body: Vec<u8>) -> Self {
        Self {
            status,
            content_type: Some(CONTENT_TYPE_BINARY),
            etag: None,
            location: None,
            body,
        }
    }

    /// A `204 No Content` response.
    #[must_use]
    pub const fn no_content() -> Self {
        Self {
            status: 204,
            content_type: None,
            etag: None,
            location: None,
            body: Vec::new(),
        }
    }

    /// A `202 Accepted` admission, with its `Location`.
    ///
    /// # Errors
    ///
    /// Returns [`ErrorCode::InternalError`] when the operation will not
    /// serialize.
    pub fn accepted(accepted: &Accepted) -> WireResult<Self> {
        let location = accepted.location();
        let mut rendered = Self::json(202, &accepted.0)?;
        rendered.location = Some(location);
        Ok(rendered)
    }

    /// Attaches a strong entity tag.
    #[must_use]
    pub fn with_etag(mut self, etag: ETag) -> Self {
        self.etag = Some(etag);
        self
    }
}

/// The stream type of a route group that declares no NDJSON route.
///
/// Uninhabited on purpose: the generated dispatcher for such a group returns the
/// same [`DispatchOutcome`] shape as every other, and the type system proves it
/// can never take the streaming arm.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum NoStream {}

/// What a generated dispatcher produced.
#[derive(Debug, Clone)]
pub enum DispatchOutcome<F> {
    /// One encoded response body.
    Unary(RawResponse),
    /// A `200 OK` `application/x-ndjson` frame stream.
    Ndjson(NdjsonStream<F>),
}

impl<F> DispatchOutcome<F> {
    /// The unary response, when the route produced one.
    #[must_use]
    pub fn into_unary(self) -> Option<RawResponse> {
        match self {
            Self::Unary(response) => Some(response),
            Self::Ndjson(_) => None,
        }
    }
}

/// The replay identity a route requires, if any.
///
/// The composition crate resolves this at precedence stages 8 and 9, before the
/// tombstone, precondition and domain-state stages the handler runs inside.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RequestIdentity {
    /// The route declares no replay identity.
    None,
    /// The route requires an `Idempotency-Key`.
    Replay(ReplayIdentity),
    /// The route requires an `Aex-Operation-Id`.
    Operation(OperationIdentity),
}

/// The canonical intent of a raw request.
///
/// The digest is over RFC 8785 JCS bytes of the body, not the bytes as they
/// arrived, so two spellings of the same request replay as one intent. There is
/// one canonicalizer in the workspace and this uses it.
///
/// # Errors
///
/// Returns [`ErrorCode::InvalidRequest`] when a non-empty body is not JSON, or
/// when it is JSON the canonicalizer refuses.
pub fn canonical_intent(raw: &RawRequest<'_>) -> WireResult<IntentDigest> {
    if raw.body.is_empty() {
        return Ok(intent_digest(raw.route, &raw.path, None));
    }
    let value: serde_json::Value = serde_json::from_slice(raw.body)
        .map_err(|reason| invalid_body(format!("the body is not valid JSON: {reason}")))?;
    let canonical = to_jcs_bytes(&value)
        .map_err(|reason| invalid_body(format!("the body is not canonicalizable: {reason}")))?;
    Ok(intent_digest(raw.route, &raw.path, Some(&canonical)))
}

/// The replay identity of a raw request.
///
/// Strict in both directions: a route that requires an identity rejects a
/// request without one, and a route that requires none rejects a request that
/// supplies one. A silently ignored `Idempotency-Key` is worse than a rejected
/// request, because the caller believes it has a replay guarantee it does not.
///
/// # Errors
///
/// Returns [`ErrorCode::InvalidRequest`] when the presented headers do not match
/// what the route declares, or when the body cannot be canonicalized.
pub fn request_identity(
    context: &RequestContext,
    raw: &RawRequest<'_>,
) -> WireResult<RequestIdentity> {
    let descriptor = route(raw.route);
    match descriptor.idempotency {
        IdempotencyKind::None => {
            if context.idempotency_key.is_some() {
                return Err(invalid_body(format!(
                    "`{}` does not accept an `Idempotency-Key`",
                    descriptor.operation_id
                )));
            }
            if context.operation_id.is_some() {
                return Err(invalid_body(format!(
                    "`{}` does not accept an `Aex-Operation-Id`",
                    descriptor.operation_id
                )));
            }
            Ok(RequestIdentity::None)
        }
        IdempotencyKind::IdempotencyKey => {
            let key = context.idempotency_key.clone().ok_or_else(|| {
                invalid_body(format!(
                    "`{}` requires an `Idempotency-Key`",
                    descriptor.operation_id
                ))
            })?;
            Ok(RequestIdentity::Replay(ReplayIdentity {
                principal: context.principal,
                route: raw.route,
                key,
                intent: canonical_intent(raw)?,
            }))
        }
        IdempotencyKind::OperationId => {
            let operation_id = context.operation_id.ok_or_else(|| {
                invalid_body(format!(
                    "`{}` requires an `Aex-Operation-Id`",
                    descriptor.operation_id
                ))
            })?;
            Ok(RequestIdentity::Operation(OperationIdentity {
                principal: context.principal,
                route: raw.route,
                operation_id,
                intent: canonical_intent(raw)?,
            }))
        }
    }
}

/// Decodes a strict AEX JSON body.
///
/// The bound is enforced before the parser runs, so an oversized body costs one
/// length comparison rather than a full parse. Unknown members are rejected by
/// the generated model itself.
///
/// # Errors
///
/// Returns [`ErrorCode::PayloadTooLarge`] above the bound and
/// [`ErrorCode::InvalidRequest`] for anything the decoder refuses.
pub fn decode_body<T: DeserializeOwned>(
    raw: &RawRequest<'_>,
    limits: RequestLimits,
) -> WireResult<T> {
    if raw.body.len() > limits.max_json_body_bytes {
        return Err(too_large(raw.body.len(), limits.max_json_body_bytes));
    }
    serde_json::from_slice(raw.body)
        .map_err(|reason| invalid_body(format!("the request body was rejected: {reason}")))
}

/// Accepts the body of a route that declares none.
///
/// # Errors
///
/// Returns [`ErrorCode::InvalidRequest`] when bytes were sent anyway. A route
/// with no declared body that quietly ignores one is a route whose replay intent
/// is not what the caller thinks it is.
pub fn expect_no_body(raw: &RawRequest<'_>) -> WireResult<()> {
    if raw.body.is_empty() {
        return Ok(());
    }
    Err(invalid_body(format!(
        "`{}` declares no request body",
        route(raw.route).operation_id
    )))
}

/// Bounds an OTLP body without interpreting it.
///
/// The payload is the pinned standard OTLP revision, not an AEX schema, so the
/// contract layer bounds it and hands it on untouched.
///
/// # Errors
///
/// Returns [`ErrorCode::TelemetryPayloadTooLarge`] above the bound.
pub fn otlp_body<'a>(raw: &RawRequest<'a>, limits: RequestLimits) -> WireResult<&'a [u8]> {
    if raw.body.len() > limits.max_otlp_body_bytes {
        return Err(
            WireError::new(ErrorCode::TelemetryPayloadTooLarge).with_message(format!(
                "the telemetry body is {} bytes, over the {} byte bound",
                raw.body.len(),
                limits.max_otlp_body_bytes
            )),
        );
    }
    Ok(raw.body)
}

/// Bounds an opaque binary body without copying or interpreting it.
///
/// # Errors
///
/// Returns [`ErrorCode::PayloadTooLarge`] above the fixed logical-part bound.
pub fn binary_body<'a>(raw: &RawRequest<'a>, limits: RequestLimits) -> WireResult<&'a [u8]> {
    let _ = limits;
    if raw.body.len() > RequestLimits::DEFAULT_BINARY_BODY_BYTES {
        return Err(too_large(
            raw.body.len(),
            RequestLimits::DEFAULT_BINARY_BODY_BYTES,
        ));
    }
    Ok(raw.body)
}

/// Reads one bound path parameter.
///
/// # Errors
///
/// Returns [`ErrorCode::InvalidRequest`] when the segment is absent or does not
/// parse as the declared type.
pub fn path_param<T: FromParam>(raw: &RawRequest<'_>, name: &'static str) -> WireResult<T> {
    let text = raw.path.get(name).ok_or_else(|| {
        invalid_at(
            &format!("/{name}"),
            format!("`{name}` is not bound by the matched template"),
        )
    })?;
    T::from_param(text)
        .map_err(|reason| invalid_at(&format!("/{name}"), format!("`{name}`: {reason}")))
}

/// Reads one bound path parameter whose type is a **closed generated registry**.
///
/// A registry-typed segment names a resource rather than describing one, so a
/// value outside the registry is not a malformed request — it is a request for
/// something that does not exist, and `404 not_found` is the only answer that
/// separates "no such limit" from "your credential is wrong" and from "the limit
/// exists and I could not read it". That is the single meaning `not_found`
/// carries on `workspace_limit_get`, and it is decided here, before any read: an
/// absent row of a *registered* identifier is never a `404`.
///
/// The unbound-segment arm stays [`ErrorCode::InvalidRequest`]: a template that
/// bound no such parameter is a router defect, not a customer's unknown name.
///
/// # Errors
///
/// Returns [`ErrorCode::NotFound`] when the segment is outside the registry, and
/// [`ErrorCode::InvalidRequest`] when the matched template bound no such segment.
pub fn path_param_registry<T: FromParam>(
    raw: &RawRequest<'_>,
    name: &'static str,
) -> WireResult<T> {
    let text = raw.path.get(name).ok_or_else(|| {
        invalid_at(
            &format!("/{name}"),
            format!("`{name}` is not bound by the matched template"),
        )
    })?;
    T::from_param(text).map_err(|_| {
        WireError::new(ErrorCode::NotFound)
            .with_message(format!("`{name}` names no registered identifier"))
    })
}

/// Rejects a handler failure the route does not declare.
///
/// The route table names every code an operation may answer with, and a
/// generated client decodes against exactly that set. Letting an undeclared code
/// reach the wire would make the published contract a description of what the
/// server usually does rather than what it does, so the boundary refuses it.
///
/// # Errors
///
/// Passes a declared failure through unchanged, and replaces an undeclared one
/// with [`ErrorCode::InternalError`] naming the violation.
pub fn declared<T>(id: RouteId, outcome: WireResult<T>) -> WireResult<T> {
    match outcome {
        Ok(value) => Ok(value),
        Err(failure) => {
            let descriptor = route(id);
            if descriptor.declares(failure.code) {
                return Err(failure);
            }
            Err(
                WireError::new(ErrorCode::InternalError).with_message(format!(
                    "`{}` answered `{}`, which it does not declare",
                    descriptor.operation_id,
                    failure.code.as_str()
                )),
            )
        }
    }
}

/// A `400 invalid_request` naming the whole body.
fn invalid_body(reason: String) -> WireError {
    invalid_at("", reason)
}

/// A `400 invalid_request` naming an exact position.
fn invalid_at(pointer: &str, reason: String) -> WireError {
    let pointer = JsonPointer::parse(pointer).unwrap_or_else(|_| JsonPointer::root());
    WireError::new(ErrorCode::InvalidRequest).with_details(ErrorDetails::Validation(
        ErrorDetailsValidation { pointer, reason },
    ))
}

/// A `413 payload_too_large` naming both sides of the comparison.
fn too_large(found: usize, bound: usize) -> WireError {
    WireError::new(ErrorCode::PayloadTooLarge).with_message(format!(
        "the body is {found} bytes, over the {bound} byte bound"
    ))
}

// ---------------------------------------------------------------------------
// Path and query parameter decoding
// ---------------------------------------------------------------------------

/// Why one parameter value was refused.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct ParamError(String);

impl ParamError {
    /// A refusal naming its own reason.
    #[must_use]
    pub fn new(reason: impl Into<String>) -> Self {
        Self(reason.into())
    }
}

/// A value the contract admits in a path segment or a query value.
///
/// There is deliberately no blanket implementation: a type reaches the wire as a
/// parameter only because the contract says it does, and a new one is a
/// generated implementation rather than an accident of some other trait.
pub trait FromParam: Sized {
    /// Parses one already percent-decoded value.
    ///
    /// # Errors
    ///
    /// Returns a [`ParamError`] naming why the value was refused.
    fn from_param(text: &str) -> Result<Self, ParamError>;
}

/// Implements [`FromParam`] through an existing validating constructor.
macro_rules! from_param_via_parse {
    ($($ty:ty),* $(,)?) => {
        $(impl FromParam for $ty {
            fn from_param(text: &str) -> Result<Self, ParamError> {
                Self::parse(text).map_err(|reason| ParamError::new(reason.to_string()))
            }
        })*
    };
}

from_param_via_parse!(
    Cents,
    ContentHash,
    Cursor,
    DecimalU128,
    ETag,
    FilePath,
    ResourceName,
    SpanId,
    Timestamp,
    TraceId,
);

/// Implements [`FromParam`] through `core::str::FromStr`.
macro_rules! from_param_via_from_str {
    ($($ty:ty),* $(,)?) => {
        $(impl FromParam for $ty {
            fn from_param(text: &str) -> Result<Self, ParamError> {
                text.parse().map_err(|reason: <$ty as core::str::FromStr>::Err| {
                    ParamError::new(reason.to_string())
                })
            }
        })*
    };
}

from_param_via_from_str!(u32, u64, i64, bool);

impl FromParam for String {
    fn from_param(text: &str) -> Result<Self, ParamError> {
        if let Some(offset) = text.bytes().position(|byte| byte < 0x20 || byte == 0x7f) {
            return Err(ParamError::new(format!(
                "a control character at byte {offset} is not admissible"
            )));
        }
        Ok(text.to_owned())
    }
}

/// Implements [`FromParam`] for a closed registry enumeration.
///
/// There is no blanket implementation over the registry enums: coherence cannot
/// prove a blanket does not overlap the newtype implementations above, and a
/// registry that must be enumerated one line at a time is one whose additions
/// are visible in a diff.
macro_rules! from_param_via_registry {
    ($($ty:ty => $noun:literal),* $(,)?) => {
        $(impl FromParam for $ty {
            fn from_param(text: &str) -> Result<Self, ParamError> {
                Self::ALL
                    .iter()
                    .copied()
                    .find(|candidate| candidate.as_str() == text)
                    .ok_or_else(|| ParamError::new(format!("`{text}` is not a known {}", $noun)))
            }
        })*
    };
}

from_param_via_registry!(
    LimitId => "limit",
    ProviderId => "provider",
    Region => "region",
);

/// A strict reader over a raw query string.
///
/// Strict means what it says: a key the route does not declare is a `400`, a
/// repeated key is a `400`, and a malformed percent escape is a `400`. Silently
/// dropping an unrecognized filter would answer a question the caller did not
/// ask.
#[derive(Debug, Clone)]
pub struct QueryReader {
    /// The declared pairs, in arrival order.
    pairs: Vec<(String, String)>,
}

impl QueryReader {
    /// Parses the query string of `id`.
    ///
    /// # Errors
    ///
    /// Returns [`ErrorCode::InvalidRequest`] for an undeclared key, a repeated
    /// key, or a malformed percent escape.
    pub fn parse(id: RouteId, query: &str) -> WireResult<Self> {
        let declared = route(id).query_params;
        let mut pairs: Vec<(String, String)> = Vec::new();
        for field in query.split('&') {
            if field.is_empty() {
                continue;
            }
            let (raw_name, raw_value) = field.split_once('=').unwrap_or((field, ""));
            let name = percent_decode(raw_name).map_err(|reason| {
                invalid_body(format!("a query parameter name is malformed: {reason}"))
            })?;
            if !declared.contains(&name.as_str()) {
                return Err(invalid_body(format!(
                    "`{}` does not accept the query parameter `{name}`",
                    route(id).operation_id
                )));
            }
            if pairs.iter().any(|(bound, _)| bound == &name) {
                return Err(invalid_body(format!(
                    "the query parameter `{name}` appears more than once"
                )));
            }
            let value = percent_decode(raw_value).map_err(|reason| {
                invalid_body(format!(
                    "the query parameter `{name}` is malformed: {reason}"
                ))
            })?;
            pairs.push((name, value));
        }
        Ok(Self { pairs })
    }

    /// The raw value bound to `name`.
    #[must_use]
    pub fn raw(&self, name: &str) -> Option<&str> {
        self.pairs
            .iter()
            .find(|(bound, _)| bound == name)
            .map(|(_, value)| value.as_str())
    }

    /// A declared optional parameter.
    ///
    /// # Errors
    ///
    /// Returns [`ErrorCode::InvalidRequest`] when the value does not parse.
    pub fn optional<T: FromParam>(&self, name: &'static str) -> WireResult<Option<T>> {
        match self.raw(name) {
            None => Ok(None),
            Some(text) => T::from_param(text)
                .map(Some)
                .map_err(|reason| invalid_query(name, &reason)),
        }
    }

    /// A declared required parameter.
    ///
    /// # Errors
    ///
    /// Returns [`ErrorCode::InvalidRequest`] when the value is absent or does
    /// not parse.
    pub fn required<T: FromParam>(&self, name: &'static str) -> WireResult<T> {
        let text = self.raw(name).ok_or_else(|| {
            invalid_at(
                &format!("/{name}"),
                format!("the query parameter `{name}` is required"),
            )
        })?;
        T::from_param(text).map_err(|reason| invalid_query(name, &reason))
    }

    /// A declared optional parameter inside its schema bounds.
    ///
    /// # Errors
    ///
    /// Returns [`ErrorCode::InvalidRequest`] when the value does not parse or
    /// falls outside `min ..= max`.
    pub fn optional_bounded<T>(&self, name: &'static str, min: T, max: T) -> WireResult<Option<T>>
    where
        T: FromParam + Copy + PartialOrd + core::fmt::Display,
    {
        let Some(value) = self.optional::<T>(name)? else {
            return Ok(None);
        };
        if value < min || value > max {
            return Err(invalid_at(
                &format!("/{name}"),
                format!("`{name}` is {value}, outside {min}..={max}"),
            ));
        }
        Ok(Some(value))
    }

    /// A declared required parameter inside its schema bounds.
    ///
    /// # Errors
    ///
    /// Returns [`ErrorCode::InvalidRequest`] when the value is absent, does not
    /// parse, or falls outside `min ..= max`.
    pub fn required_bounded<T>(&self, name: &'static str, min: T, max: T) -> WireResult<T>
    where
        T: FromParam + Copy + PartialOrd + core::fmt::Display,
    {
        let value = self.required::<T>(name)?;
        if value < min || value > max {
            return Err(invalid_at(
                &format!("/{name}"),
                format!("`{name}` is {value}, outside {min}..={max}"),
            ));
        }
        Ok(value)
    }
}

/// A `400 invalid_request` naming one query parameter.
fn invalid_query(name: &str, reason: &ParamError) -> WireError {
    invalid_at(
        &format!("/{name}"),
        format!("the query parameter `{name}` was rejected: {reason}"),
    )
}

/// Decodes one `application/x-www-form-urlencoded` token.
///
/// # Errors
///
/// Returns a [`ParamError`] for a truncated or non-hexadecimal escape, or for a
/// result that is not UTF-8.
pub fn percent_decode(text: &str) -> Result<String, ParamError> {
    let bytes = text.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            b'+' => {
                out.push(b' ');
                index += 1;
            }
            b'%' => {
                let high = bytes
                    .get(index + 1)
                    .copied()
                    .and_then(hex_nibble)
                    .ok_or_else(|| {
                        ParamError::new(format!("a truncated escape at byte {index}"))
                    })?;
                let low = bytes
                    .get(index + 2)
                    .copied()
                    .and_then(hex_nibble)
                    .ok_or_else(|| {
                        ParamError::new(format!("a truncated escape at byte {index}"))
                    })?;
                out.push((high << 4) | low);
                index += 3;
            }
            byte => {
                out.push(byte);
                index += 1;
            }
        }
    }
    String::from_utf8(out).map_err(|_| ParamError::new("the decoded value is not UTF-8"))
}

/// One hexadecimal digit as a nibble.
const fn hex_nibble(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

/// A route that reached the wrong group's dispatcher.
///
/// Every generated dispatcher is total over `RouteId`, so a route from another
/// group is a composition defect that names itself rather than a handler quietly
/// running against the wrong request.
#[must_use]
pub fn wrong_group(found: RouteId, group: &str) -> WireError {
    WireError::new(ErrorCode::InternalError)
        .with_message(format!("`{}` is not a `{group}` route", found.as_str()))
}
