//! The public error vocabulary.
//!
//! [`ErrorCode`] is generated and closed: adding a variant is a compile error at
//! every exhaustive match, which is exactly what should happen when the platform
//! learns a new way to fail. A *client* must still tolerate a newer server, so
//! what a decoder observes is [`ObservedErrorCode`], which has one extra arm the
//! server can never produce.

use std::fmt;

pub use crate::generated::errors::ErrorCode;
pub use crate::generated::models::{
    ApiError, ApiErrorBody, ErrorDetails, ErrorDetailsGap, ErrorDetailsLimit,
    ErrorDetailsOperation, ErrorDetailsQuota, ErrorDetailsRegion, ErrorDetailsRequiredScope,
    ErrorDetailsRetry, ErrorDetailsValidation,
};
use crate::ids::OperationId;
use crate::types::RequestId;

/// Which failure family a code belongs to.
///
/// The CLI maps this onto its stable exit classes, so it is deliberately coarser
/// than [`ErrorCode`] and changes far less often.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize, serde::Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum ErrorClass {
    /// The credential is missing, invalid, or insufficient.
    Auth,
    /// The resource does not exist, or no longer does.
    NotFound,
    /// The request lost a race or replayed a different intent.
    Conflict,
    /// An `If-Match` or generation precondition did not hold.
    Precondition,
    /// The request itself is malformed.
    Validation,
    /// A limit or quota was exceeded.
    Quota,
    /// The resource is in the wrong state for this request.
    State,
    /// A dependency is temporarily unreachable.
    Unavailable,
    /// Something the platform did not expect.
    Internal,
}

/// The error-precedence stages, in evaluation order.
///
/// The middleware stack evaluates ascending, so a code's stage says exactly
/// where in the pipeline it may be emitted. Authentication, authorization and
/// pause checks all precede idempotent replay, which is why a paused caller is
/// never re-shown a grant.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize, serde::Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum PrecedenceStage {
    /// Provider transport envelope and static content type.
    TransportEnvelope = 1,
    /// Establish a valid credential-bound central authorization assertion.
    Authentication = 2,
    /// Resolve organization and workspace authorization.
    Authorization = 3,
    /// Compare immutable placement on a regional host.
    Placement = 4,
    /// Enforce route scopes and organization role.
    Scope = 5,
    /// Establish the current account-state revision.
    AccountState = 6,
    /// Enforce the body bound, then parse, validate and canonicalize.
    BodyLimitAndParse = 7,
    /// Look up `Aex-Operation-Id` before any tombstone or domain check.
    OperationIdentity = 8,
    /// Apply the same rule to `Idempotency-Key`.
    IdempotencyIdentity = 9,
    /// Authorize the target parent, then check tombstones.
    TombstoneAndParent = 10,
    /// Evaluate `If-Match` and generation preconditions.
    Precondition = 11,
    /// Evaluate domain state such as idle, approval resolution, export readiness.
    DomainState = 12,
    /// Commit.
    Commit = 13,
}

impl PrecedenceStage {
    /// Every stage, in evaluation order.
    pub const ALL: [Self; 13] = [
        Self::TransportEnvelope,
        Self::Authentication,
        Self::Authorization,
        Self::Placement,
        Self::Scope,
        Self::AccountState,
        Self::BodyLimitAndParse,
        Self::OperationIdentity,
        Self::IdempotencyIdentity,
        Self::TombstoneAndParent,
        Self::Precondition,
        Self::DomainState,
        Self::Commit,
    ];

    /// The one-based evaluation order.
    #[must_use]
    pub const fn order(self) -> u8 {
        self as u8
    }
}

impl fmt::Display for ErrorCode {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// What a decoder may observe in an error envelope.
///
/// [`ObservedErrorCode::Unrecognized`] exists only because the evolution policy
/// classifies a newly declared error code as additive. The server never emits
/// it, and a test asserts that every code the server can produce round-trips as
/// [`ObservedErrorCode::Known`].
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ObservedErrorCode {
    /// A code this build knows.
    Known(ErrorCode),
    /// A code a newer server declared. Treat it by its HTTP status.
    Unrecognized(Box<str>),
}

impl ObservedErrorCode {
    /// The wire spelling, whether or not the code is known.
    #[must_use]
    pub fn as_str(&self) -> &str {
        match self {
            Self::Known(code) => code.as_str(),
            Self::Unrecognized(text) => text,
        }
    }

    /// The known code, if this build recognizes it.
    #[must_use]
    pub const fn known(&self) -> Option<ErrorCode> {
        match self {
            Self::Known(code) => Some(*code),
            Self::Unrecognized(_) => None,
        }
    }
}

impl From<ErrorCode> for ObservedErrorCode {
    fn from(code: ErrorCode) -> Self {
        Self::Known(code)
    }
}

impl fmt::Display for ObservedErrorCode {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl serde::Serialize for ObservedErrorCode {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> serde::Deserialize<'de> for ObservedErrorCode {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text =
            <std::borrow::Cow<'de, str> as serde::Deserialize<'de>>::deserialize(deserializer)?;
        Ok(ErrorCode::parse(text.as_ref())
            .map_or_else(|| Self::Unrecognized(text.as_ref().into()), Self::Known))
    }
}

/// What a handler returns. The code is closed, so a handler cannot invent one.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
#[error("{code}: {}", message.as_deref().unwrap_or(code.default_message()))]
pub struct WireError {
    /// The closed error code.
    pub code: ErrorCode,
    /// An override for the default message; never an upstream provider body.
    pub message: Option<String>,
    /// Typed remediation detail.
    pub details: Option<ErrorDetails>,
    /// How long the caller should wait before retrying.
    pub retry_after: Option<core::time::Duration>,
}

impl WireError {
    /// A bare error with the code's default message.
    #[must_use]
    pub const fn new(code: ErrorCode) -> Self {
        Self {
            code,
            message: None,
            details: None,
            retry_after: None,
        }
    }

    /// Attaches typed remediation detail.
    #[must_use]
    pub fn with_details(mut self, details: ErrorDetails) -> Self {
        self.details = Some(details);
        self
    }

    /// Overrides the default message.
    #[must_use]
    pub fn with_message(mut self, message: impl Into<String>) -> Self {
        self.message = Some(message.into());
        self
    }

    /// Attaches a retry hint.
    #[must_use]
    pub const fn with_retry_after(mut self, retry_after: core::time::Duration) -> Self {
        self.retry_after = Some(retry_after);
        self
    }

    /// Renders the status, envelope and retry hint.
    ///
    /// This is the only sanctioned way to turn a handler failure into a
    /// response, so the envelope shape is decided in exactly one place.
    #[must_use]
    pub fn into_response_parts(
        self,
        request_id: &RequestId,
        operation_id: Option<OperationId>,
    ) -> (u16, ApiError, Option<core::time::Duration>) {
        let status = self.code.http_status();
        let retry_after = self.retry_after;
        let body = ApiError {
            error: ApiErrorBody {
                code: ObservedErrorCode::Known(self.code),
                message: self
                    .message
                    .unwrap_or_else(|| self.code.default_message().to_owned()),
                request_id: request_id.as_str().to_owned(),
                retryable: self.code.retryable(),
                operation_id,
                details: self.details,
            },
        };
        (status, body, retry_after)
    }
}

/// What every handler returns.
pub type WireResult<T> = Result<T, WireError>;
