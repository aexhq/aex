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

use crate::primitives::BoundedString;

/// The fifteen operational failure kinds an adapter can distinguish.
///
/// The Brain application re-exports the four-member
/// [`ProviderFailureClass`] below, and [`ProviderFailureKind::class`] is the
/// single translation site (D-27).
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
    /// `DeepSeek`'s `insufficient_system_resource`.
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
    /// `TODO(cross-stream)`: `aex-brain-app` landed with the four classes
    /// rather than these fifteen kinds, so this function is still the only place
    /// the mapping exists.
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

/// The canonical port-facing failure class re-exported by the Brain.
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

/// Bounded provider failure detail that is safe to persist or log.
///
/// This type is the single redacted failure vocabulary shared by the Brain
/// port and all eight provider-authority adapters. It deliberately accepts only bounded
/// strings; adapters must redact provider bytes before constructing it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RedactedDetail {
    /// The operational failure kind.
    pub kind: ProviderFailureKind,
    /// The HTTP status, when a response head was received.
    pub http_status: Option<u16>,
    /// The provider's bounded, redacted error code.
    pub provider_code: Option<BoundedString<64>>,
    /// The bounded, redacted diagnostic.
    pub message: BoundedString<512>,
}

impl RedactedDetail {
    /// Builds a detail from bytes the caller has already redacted.
    #[must_use]
    pub const fn new(kind: ProviderFailureKind, message: BoundedString<512>) -> Self {
        Self {
            kind,
            http_status: None,
            provider_code: None,
            message,
        }
    }

    /// Builds a detail from a static/internal diagnostic that contains no
    /// provider response bytes or credential material.
    #[must_use]
    pub fn internal(kind: ProviderFailureKind, message: &str) -> Self {
        Self::new(kind, BoundedString::truncating(message))
    }

    /// Attaches the observed HTTP status.
    #[must_use]
    pub const fn with_status(mut self, status: u16) -> Self {
        self.http_status = Some(status);
        self
    }

    /// Attaches a bounded provider error code.
    #[must_use]
    pub fn with_code(mut self, code: &str) -> Self {
        self.provider_code = Some(BoundedString::truncating(code));
        self
    }

    /// The port-facing recovery class.
    #[must_use]
    pub const fn class(&self) -> ProviderFailureClass {
        self.kind.class()
    }

    /// The redacted diagnostic body.
    #[must_use]
    pub fn as_str(&self) -> &str {
        self.message.as_str()
    }
}

impl core::fmt::Display for RedactedDetail {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(formatter, "{:?}", self.kind)?;
        if let Some(status) = self.http_status {
            write!(formatter, " http={status}")?;
        }
        if let Some(code) = &self.provider_code {
            write!(formatter, " code={code}")?;
        }
        write!(formatter, ": {}", self.message)
    }
}
