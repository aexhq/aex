//! What a handler receives and what it may return.
//!
//! `aex-wire` deliberately does not depend on `axum`. The HTTP composition
//! crates bind these shapes to a framework; this module only fixes what the
//! shapes are, so both planes render a `201`, a `202`, a `204` and an entity tag
//! identically without re-typing the rule.

pub use crate::generated::server::{RouteGroup, *};

use crate::cursor::Cursor;
use crate::error::ApiError;
use crate::idempotency::{IdempotencyKey, PrincipalScope};
use crate::ids::{OperationId, SessionId, Uuid7};
use crate::models::{DeletingSession, Operation, Session, SessionTombstone};
use crate::routes::RouteId;
use crate::scopes::ScopeSet;
use crate::types::{ETag, RequestId};

/// What a caller said it will accept.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum AcceptKind {
    /// `application/json`.
    Json,
    /// `application/x-ndjson`.
    Ndjson,
    /// `application/pdf`; only the statement read offers it.
    Pdf,
}

/// Everything the middleware established before the handler ran.
///
/// A handler never re-derives any of this. Every field is settled by an earlier
/// precedence stage, so a handler that reached this point has already passed
/// authentication, placement, scope, account state, body limits and replay
/// identity.
#[derive(Debug, Clone)]
pub struct RequestContext {
    /// The diagnostic request identifier echoed in every error envelope.
    pub request_id: RequestId,
    /// Which route matched.
    pub route: RouteId,
    /// Who is asking.
    pub principal: PrincipalScope,
    /// The browser session the actor presented, when the credential was one.
    ///
    /// [`PrincipalScope`] deliberately narrows an actor to *who* they are,
    /// because that is all replay identity may depend on — two credentials of
    /// one person must share a replay scope. A handful of ceremonies need
    /// *which credential* as well: approving a device authorization records the
    /// live session that proved the approver was current, and closing a session
    /// closes the one that was presented. Neither can be re-derived, because a
    /// person may hold several sessions at once.
    ///
    /// `None` for an account token, a workspace key and an anonymous caller —
    /// so a handler that requires a browser session must refuse `None` rather
    /// than substitute anything for it.
    pub actor_session_id: Option<Uuid7>,
    /// What the credential actually carries.
    pub granted_scopes: ScopeSet,
    /// The replay key, when the route requires one.
    pub idempotency_key: Option<IdempotencyKey>,
    /// The caller-minted operation id, when the route requires one.
    pub operation_id: Option<OperationId>,
    /// The precondition the caller supplied.
    pub if_match: Option<ETag>,
    /// What the caller said it will accept.
    pub accept: AcceptKind,
}

/// A `201 Created` response.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Created<T>(pub T);

/// A `202 Accepted` response with `Location: /api/operations/{id}`.
#[derive(Debug, Clone, PartialEq)]
pub struct Accepted(pub Operation);

impl Accepted {
    /// The `Location` header value for this admission.
    #[must_use]
    pub fn location(&self) -> String {
        format!("/api/operations/{}", self.0.id)
    }
}

/// A response that carries a strong entity tag.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WithETag<T> {
    /// The resource.
    pub value: T,
    /// Its current entity tag.
    pub etag: ETag,
}

/// A `204 No Content` response.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NoContent;

/// An `application/x-ndjson` response.
///
/// The frame stream itself is the composition crate's; this wrapper exists so a
/// handler signature states the transport rather than leaving it to a header the
/// middleware might forget.
#[derive(Debug, Clone)]
pub struct NdjsonStream<F>(pub F);

/// What `GET /api/sessions/{sessionId}` resolves to.
///
/// The discriminator is the HTTP status, not a body member: an active session is
/// `200`, and both terminal states are `410`. That is why this is a server-side
/// union rather than a wire schema — the wire never sees a tag.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
#[serde(untagged)]
pub enum SessionReadResult {
    /// A live session; rendered at `200`.
    Active(Box<Session>),
    /// A session whose deletion is running; rendered at `410`.
    Deleting(DeletingSession),
    /// A deleted session; rendered at `410`.
    Deleted(SessionTombstone),
}

impl SessionReadResult {
    /// The status this result renders at.
    #[must_use]
    pub const fn http_status(&self) -> u16 {
        match self {
            Self::Active(_) => 200,
            Self::Deleting(_) | Self::Deleted(_) => 410,
        }
    }

    /// The session this result is about, whatever its state.
    #[must_use]
    pub const fn session_id(&self) -> SessionId {
        match self {
            Self::Active(session) => session.id,
            Self::Deleting(session) => session.id,
            Self::Deleted(tombstone) => tombstone.id,
        }
    }
}

/// A rendered error response: status, envelope, and an optional retry hint.
#[derive(Debug, Clone, PartialEq)]
pub struct ErrorResponse {
    /// The HTTP status.
    pub status: u16,
    /// The one error envelope.
    pub body: ApiError,
    /// The `Retry-After` hint, when the code carries one.
    pub retry_after: Option<core::time::Duration>,
}

/// A continuation the handler produced for the next page.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NextPage(pub Option<Cursor>);
