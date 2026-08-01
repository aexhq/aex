//! Total edge failure mapping into the closed wire error vocabulary.

use aex_wire::error::{ApiError, ErrorCode, WireError};

use crate::context::RequestContext;
use crate::cursor::CursorError;
use crate::envelope::EnvelopeError;
use crate::idempotency::IdentityError;
use crate::limits::LimitError;

/// Every failure produced inside this composition crate.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum EdgeError {
    /// Provider envelope or content type failed.
    #[error(transparent)]
    Envelope(#[from] EnvelopeError),
    /// No current credential-bound assertion exists.
    #[error("request is not authenticated")]
    Unauthenticated,
    /// The central authority could not establish identity.
    #[error("authentication authority is unavailable")]
    AuthenticationUnavailable,
    /// Caller cannot select this workspace.
    #[error("caller is not authorized for the workspace")]
    Forbidden,
    /// Immutable placement differs from this host.
    #[error("workspace is bound to another region")]
    WrongWorkspaceRegion,
    /// Generated scope is absent.
    #[error("required scope is absent")]
    InsufficientScope,
    /// Account is paused and the route is not exempt.
    #[error("account is paused")]
    AccountPaused,
    /// Account state could not be established.
    #[error("account state is unavailable")]
    AccountStateUnavailable,
    /// Dynamic body limit failed.
    #[error(transparent)]
    Limit(#[from] LimitError),
    /// Durable operation header failed.
    #[error(transparent)]
    Identity(#[from] IdentityError),
    /// Cursor failed authentication, binding, or expiry.
    #[error(transparent)]
    Cursor(#[from] CursorError),
    /// Replay key was reused for another intent.
    #[error("idempotency key conflicts with a stored intent")]
    IdempotencyConflict,
    /// Durable operation id was reused for another intent.
    #[error("operation id conflicts with a stored intent")]
    OperationIdempotencyConflict,
    /// Conditional mutation precondition failed.
    #[error("precondition failed")]
    PreconditionFailed,
    /// Durable authority was temporarily unavailable.
    #[error("authority commit is unavailable")]
    CommitUnavailable,
    /// A non-recoverable internal invariant failed.
    #[error("internal edge invariant failed")]
    Internal,
}

/// Renders a request-bound public error without preserving upstream text.
pub trait IntoWireError {
    /// Maps through an exhaustive match; a new edge variant breaks this method.
    fn into_wire(self, context: &RequestContext) -> ApiError;
}

impl IntoWireError for EdgeError {
    fn into_wire(self, context: &RequestContext) -> ApiError {
        let code = match self {
            Self::Envelope(EnvelopeError::TooLarge { .. })
            | Self::Limit(LimitError::EnvelopeTooLarge { .. }) => ErrorCode::PayloadTooLarge,
            Self::Envelope(EnvelopeError::UnsupportedContentType)
            | Self::Identity(
                IdentityError::MissingOperationId | IdentityError::InvalidOperationId,
            ) => ErrorCode::InvalidRequest,
            Self::Unauthenticated => ErrorCode::Unauthenticated,
            Self::AuthenticationUnavailable => ErrorCode::AuthenticationUnavailable,
            Self::Forbidden => ErrorCode::Forbidden,
            Self::WrongWorkspaceRegion => ErrorCode::WrongWorkspaceRegion,
            Self::InsufficientScope => ErrorCode::InsufficientScope,
            Self::AccountPaused => ErrorCode::AccountPaused,
            Self::AccountStateUnavailable => ErrorCode::AccountStateUnavailable,
            Self::Limit(LimitError::InvalidConfiguration) | Self::Internal => {
                ErrorCode::InternalError
            }
            Self::Limit(LimitError::JsonBodyTooLarge { .. }) => ErrorCode::LimitExceeded,
            Self::Cursor(
                CursorError::WeakKey { .. }
                | CursorError::Malformed
                | CursorError::NotBound
                | CursorError::Expired,
            ) => ErrorCode::InvalidCursor,
            Self::IdempotencyConflict => ErrorCode::IdempotencyConflict,
            Self::OperationIdempotencyConflict => ErrorCode::OperationIdempotencyConflict,
            Self::PreconditionFailed => ErrorCode::PreconditionFailed,
            Self::CommitUnavailable => ErrorCode::UpstreamError,
        };
        let (_, body, _) =
            WireError::new(code).into_response_parts(&context.request_id, context.operation_id);
        body
    }
}
