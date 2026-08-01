//! Typed provider failures and the single translation site onto the Brain
//! port's four classes (plan 08 §3.6, D-27).
//!
//! [`ProviderFailureKind`] lives in `aex-model-catalog` because a catalog
//! entry's `error_map` is data that names these kinds; it is re-exported here
//! so the plan's `error::ProviderFailureKind` path resolves.

use core::time::Duration;

use aex_model_catalog::primitives::BoundedString;
use aex_wire::types::Timestamp;
use serde::{Deserialize, Serialize};

pub use aex_model_catalog::{ProviderFailureClass, ProviderFailureKind};

/// Bounded, redacted failure detail. Never carries a credential.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RedactedDetail {
    /// The operational kind.
    pub kind: ProviderFailureKind,
    /// The HTTP status, where one was received.
    pub http_status: Option<u16>,
    /// The provider's own `type` or `code` string.
    pub provider_code: Option<BoundedString<64>>,
    /// The bounded, redacted message.
    pub message: BoundedString<512>,
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

impl RedactedDetail {
    /// Builds a detail whose message is already bounded and redacted.
    #[must_use]
    pub fn new(kind: ProviderFailureKind, message: BoundedString<512>) -> Self {
        Self {
            kind,
            http_status: None,
            provider_code: None,
            message,
        }
    }

    /// Builds a detail from a message this crate produced itself, which by
    /// construction contains no provider bytes.
    #[must_use]
    pub fn internal(kind: ProviderFailureKind, message: &str) -> Self {
        Self::new(kind, BoundedString::truncating(message))
    }

    /// Attaches the HTTP status.
    #[must_use]
    pub fn with_status(mut self, status: u16) -> Self {
        self.http_status = Some(status);
        self
    }

    /// Attaches the provider's own code.
    #[must_use]
    pub fn with_code(mut self, code: &str) -> Self {
        self.provider_code = Some(BoundedString::truncating(code));
        self
    }

    /// The port-facing class.
    #[must_use]
    pub const fn class(&self) -> ProviderFailureClass {
        self.kind.class()
    }
}

/// A classified provider failure plus whatever backpressure the provider
/// published alongside it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderFailure {
    /// The classified detail.
    pub detail: RedactedDetail,
    /// Any rate-limit feedback carried on the same response.
    pub rate_limit: RateLimitFeedback,
}

impl ProviderFailure {
    /// Builds a failure with no rate-limit feedback.
    #[must_use]
    pub fn new(detail: RedactedDetail) -> Self {
        Self {
            detail,
            rate_limit: RateLimitFeedback::none(),
        }
    }

    /// The operational kind.
    #[must_use]
    pub const fn kind(&self) -> ProviderFailureKind {
        self.detail.kind
    }

    /// The port-facing class.
    #[must_use]
    pub const fn class(&self) -> ProviderFailureClass {
        self.detail.kind.class()
    }
}

/// What the provider published about its own limits.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RateLimitFeedback {
    /// How long the provider asked the caller to wait.
    pub retry_after: Option<Duration>,
    /// Requests left in the window.
    pub requests_remaining: Option<u32>,
    /// Tokens left in the window.
    pub tokens_remaining: Option<u64>,
    /// When the window resets.
    pub reset_at: Option<Timestamp>,
    /// Where the answer came from.
    pub source: RateLimitSource,
}

impl RateLimitFeedback {
    /// The positive record that a provider published nothing.
    ///
    /// `DeepSeek` documents no rate-limit headers at all and Z.AI's table is
    /// login-gated, so "absent" is a fact this type states rather than a value
    /// it omits.
    #[must_use]
    pub fn none() -> Self {
        Self::default()
    }

    /// A `Retry-After` header answer.
    #[must_use]
    pub fn retry_after(delay: Duration) -> Self {
        Self {
            retry_after: Some(delay),
            source: RateLimitSource::RetryAfterHeader,
            ..Self::default()
        }
    }

    /// Whether the provider said anything at all.
    #[must_use]
    pub fn is_absent(&self) -> bool {
        self.source == RateLimitSource::NotProvided
    }
}

/// Where rate-limit feedback came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RateLimitSource {
    /// The provider publishes none.
    #[default]
    NotProvided,
    /// A standard `Retry-After` header.
    RetryAfterHeader,
    /// Vendor `x-ratelimit-*` or `anthropic-ratelimit-*` headers.
    VendorHeaders,
    /// A code inside the error body, which is all Z.AI offers.
    ErrorBody,
}

#[cfg(test)]
mod tests {
    use super::{ProviderFailureClass, ProviderFailureKind, RateLimitFeedback, RedactedDetail};

    #[test]
    fn every_kind_maps_onto_exactly_one_port_class() {
        // The table in plan 08 §3.6, asserted exhaustively so widening
        // `ProviderFailureKind` cannot silently reclassify an existing member.
        for kind in ProviderFailureKind::ALL {
            let class = kind.class();
            let expected = match kind {
                ProviderFailureKind::Transport
                | ProviderFailureKind::Timeout
                | ProviderFailureKind::ServerError
                | ProviderFailureKind::InsufficientProviderResource => {
                    ProviderFailureClass::Transient
                }
                ProviderFailureKind::RateLimited | ProviderFailureKind::Overloaded => {
                    ProviderFailureClass::Overloaded
                }
                ProviderFailureKind::Cancelled => ProviderFailureClass::Cancelled,
                _ => ProviderFailureClass::Permanent,
            };
            assert_eq!(class, expected, "{kind:?} maps to the wrong class");
        }
    }

    #[test]
    fn quota_and_billing_are_permanent_so_they_are_never_retried() {
        assert_eq!(
            ProviderFailureKind::Quota.class(),
            ProviderFailureClass::Permanent
        );
        assert_eq!(
            ProviderFailureKind::Billing.class(),
            ProviderFailureClass::Permanent
        );
        assert!(!ProviderFailureKind::Quota.is_in_call_retryable());
        assert!(!ProviderFailureKind::Billing.is_in_call_retryable());
    }

    #[test]
    fn only_rate_limited_and_overloaded_are_in_call_retryable() {
        let retryable: Vec<_> = ProviderFailureKind::ALL
            .into_iter()
            .filter(|kind| kind.is_in_call_retryable())
            .collect();
        assert_eq!(
            retryable,
            vec![
                ProviderFailureKind::RateLimited,
                ProviderFailureKind::Overloaded
            ]
        );
    }

    #[test]
    fn absent_rate_limit_feedback_is_a_positive_record() {
        let feedback = RateLimitFeedback::none();
        assert!(feedback.is_absent());
        assert_eq!(feedback.retry_after, None);
    }

    #[test]
    fn detail_renders_status_and_code() {
        let detail = RedactedDetail::internal(ProviderFailureKind::Quota, "balance exhausted")
            .with_status(402)
            .with_code("insufficient_balance");
        let rendered = detail.to_string();
        assert!(rendered.contains("http=402"), "{rendered}");
        assert!(rendered.contains("code=insufficient_balance"), "{rendered}");
    }
}
