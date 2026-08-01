//! The provider-failure vocabulary.
//!
//! Plan 08 §3.6 places `ProviderFailureKind` in the gateway's `error` module.
//! It is defined here instead, because a catalog entry's `error_map` is *data*
//! that names these kinds: putting the vocabulary above the document that
//! references it keeps the catalog self-describing and keeps the gateway from
//! being a dependency of the pure crate. `aex-brain-provider-gateway`
//! re-exports every item below, so the plan's `error::ProviderFailureKind` path
//! still resolves. Recorded as decision D-29 in `references/rewrite/providers.md`.

use serde::{Deserialize, Serialize};

/// The fifteen operational failure kinds an adapter can distinguish.
///
/// Plan 07's `ProviderFailureClass` has four members; [`ProviderFailureClass`]
/// below mirrors it and [`ProviderFailureKind::class`] is the single
/// translation site (D-27).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderFailureKind {
    /// Connection, TLS or socket failure.
    Transport,
    /// A budget deadline elapsed.
    Timeout,
    /// The provider rate-limited the request.
    RateLimited,
    /// The provider is overloaded but the request is otherwise fine.
    Overloaded,
    /// The credential was rejected.
    Authentication,
    /// The account has exhausted a quota that retrying cannot restore.
    Quota,
    /// The account has a billing problem.
    Billing,
    /// The request itself is invalid.
    InvalidRequest,
    /// The provider does not know this model.
    ModelNotFound,
    /// The prompt exceeds the model's context window.
    ContextOverflow,
    /// The provider blocked the generation on content grounds.
    ContentFiltered,
    /// The provider spoke something outside its documented protocol.
    ProtocolViolation,
    /// An unclassified provider-side error.
    ServerError,
    /// The caller cancelled.
    Cancelled,
    /// The provider ran out of a resource it names explicitly, such as
    /// DeepSeek's `insufficient_system_resource`.
    InsufficientProviderResource,
}

impl ProviderFailureKind {
    /// Every kind, for exhaustive mapping tests.
    pub const ALL: [Self; 15] = [
        Self::Transport,
        Self::Timeout,
        Self::RateLimited,
        Self::Overloaded,
        Self::Authentication,
        Self::Quota,
        Self::Billing,
        Self::InvalidRequest,
        Self::ModelNotFound,
        Self::ContextOverflow,
        Self::ContentFiltered,
        Self::ProtocolViolation,
        Self::ServerError,
        Self::Cancelled,
        Self::InsufficientProviderResource,
    ];

    /// The single translation site onto plan 07's four port-facing classes.
    ///
    /// `TODO(cross-stream): aex-brain-application may widen
    /// ProviderFailureClass to these fifteen kinds; until then this function is
    /// the only place the mapping exists.`
    #[must_use]
    pub const fn class(self) -> ProviderFailureClass {
        match self {
            Self::Transport
            | Self::Timeout
            | Self::ServerError
            | Self::InsufficientProviderResource => ProviderFailureClass::Transient,
            Self::RateLimited | Self::Overloaded => ProviderFailureClass::Overloaded,
            Self::Authentication
            | Self::Quota
            | Self::Billing
            | Self::InvalidRequest
            | Self::ModelNotFound
            | Self::ContextOverflow
            | Self::ContentFiltered
            | Self::ProtocolViolation => ProviderFailureClass::Permanent,
            Self::Cancelled => ProviderFailureClass::Cancelled,
        }
    }

    /// Whether an in-call retry may ever be considered for this kind. The
    /// status and frame-count conditions of D-20 still apply on top.
    #[must_use]
    pub const fn is_in_call_retryable(self) -> bool {
        matches!(self, Self::RateLimited | Self::Overloaded)
    }
}

/// Plan 07's port-facing failure class, implemented verbatim.
///
/// `TODO(cross-stream): replaced by
/// aex_brain_application::ports::ProviderFailureClass at merge.`
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderFailureClass {
    /// Retrying later may succeed.
    Transient,
    /// Retrying will not succeed without a change.
    Permanent,
    /// The provider asked for backpressure.
    Overloaded,
    /// The caller cancelled.
    Cancelled,
}
