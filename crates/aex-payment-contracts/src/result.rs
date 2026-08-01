//! Effect outcomes.
//!
//! The state machine has five states because four is not enough. A provider
//! 5xx, a timeout and a transport loss are all *outcome-indeterminate*: the
//! charge may or may not have happened. Folding them into `Failed` is what turns
//! a retry into a double charge, so [`EffectState::OutcomeUnknown`] fences
//! another automatic charge for the account until reconciliation resolves it.

use aex_wire::types::{Cents, HttpsUrl, StableCode, Timestamp};
use serde::{Deserialize, Serialize};

use crate::command::EffectId;
use crate::{ProviderObjectRef, RedactedEmail};

/// Where an effect is in its lifecycle.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EffectState {
    /// Admitted and committed by finance; not yet executed.
    Prepared,
    /// The provider confirmed success.
    Succeeded,
    /// The provider deterministically refused.
    Failed,
    /// The outcome is indeterminate; automatic charging is fenced.
    OutcomeUnknown,
    /// Reconciliation could not resolve it; a human must look.
    ManualReview,
}

impl EffectState {
    /// Every state.
    pub const ALL: [Self; 5] = [
        Self::Prepared,
        Self::Succeeded,
        Self::Failed,
        Self::OutcomeUnknown,
        Self::ManualReview,
    ];

    /// Whether this state permits another automatic charge for the account.
    #[must_use]
    pub const fn permits_automatic_charge(self) -> bool {
        matches!(self, Self::Succeeded | Self::Failed)
    }

    /// Whether `next` is a legal transition from this state.
    #[must_use]
    pub const fn can_transition_to(self, next: Self) -> bool {
        match self {
            Self::Prepared => matches!(next, Self::Succeeded | Self::Failed | Self::OutcomeUnknown),
            // Reconciliation may resolve an unknown either way, or give up.
            Self::OutcomeUnknown => {
                matches!(next, Self::Succeeded | Self::Failed | Self::ManualReview)
            }
            // Terminal.
            Self::Succeeded | Self::Failed | Self::ManualReview => false,
        }
    }
}

/// How tax is handled for a hosted session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaxMode {
    /// The provider calculates and collects tax.
    ProviderAutomatic,
    /// No tax is calculated.
    None,
}

/// What the provider reported about tax on a settled effect.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct TaxEvidence {
    /// How much tax the provider collected.
    pub collected: Cents,
    /// The provider transaction reference for the tax leg.
    pub transaction: ProviderObjectRef,
}

/// Why a deterministic failure happened.
///
/// Only `CardDeclined`, `AuthenticationRequired` and `InvalidRequest` are
/// deterministic. Everything else becomes [`PaymentResult::Unknown`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PaymentFailureClass {
    /// The issuer declined.
    CardDeclined,
    /// The issuer requires an authentication step.
    AuthenticationRequired,
    /// AEX sent something the provider rejected outright.
    InvalidRequest,
    /// The provider rate-limited the call.
    RateLimited,
    /// The provider was unavailable.
    ProviderUnavailable,
    /// The provider refused permanently for a reason that will not change.
    Permanent,
}

impl PaymentFailureClass {
    /// Whether this class is a determinate outcome.
    ///
    /// A class that is not determinate must never be recorded as `Failed`, which
    /// is what `a_provider_5xx_never_classifies_as_failed` asserts.
    #[must_use]
    pub const fn is_determinate(self) -> bool {
        matches!(
            self,
            Self::CardDeclined | Self::AuthenticationRequired | Self::InvalidRequest
        )
    }
}

/// A deterministic provider failure.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct PaymentFailure {
    /// Which class of failure.
    pub class: PaymentFailureClass,
    /// The stable provider code, carried but never interpreted.
    pub provider_code: Option<StableCode>,
    /// The stable decline code, carried but never interpreted.
    pub decline_code: Option<StableCode>,
    /// Whether an identical retry can succeed.
    pub retryable: bool,
}

/// A short-lived hosted provider page.
///
/// The URL never leaves the finance path: it is returned to the caller that
/// asked for it and is not written to any log, journal or event.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct HostedSession {
    /// The hosted page.
    pub url: HttpsUrl,
    /// When the page stops working.
    pub expires_at: Timestamp,
}

impl std::fmt::Debug for HostedSession {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "HostedSession(<redacted-url>, expires_at: {})",
            self.expires_at
        )
    }
}

/// What was actually observed when an effect ended indeterminate.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "evidence",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub enum UnknownEvidence {
    /// The call timed out before any response.
    Timeout {
        /// How long the edge waited.
        waited_ms: u64,
    },
    /// The transport failed mid-call.
    TransportLost,
    /// The provider returned a server error.
    ServerError {
        /// The HTTP status observed.
        status: u16,
    },
    /// The provider answered, but not in a way that settles the outcome.
    AmbiguousResponse {
        /// The stable provider code, when one was present.
        provider_code: Option<StableCode>,
    },
}

/// The outcome of one executed effect.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "outcome",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub enum PaymentResult {
    /// The provider confirmed success.
    Succeeded {
        /// Which effect.
        effect: EffectId,
        /// The object the provider created.
        provider_ref: ProviderObjectRef,
        /// When the provider says it happened.
        provider_created_at: Timestamp,
        /// The hosted page, for the two commands that create one.
        hosted: Option<HostedSession>,
        /// How much was actually charged.
        charged: Cents,
        /// What the provider reported about tax.
        tax: Option<TaxEvidence>,
    },
    /// The provider deterministically refused.
    Failed {
        /// Which effect.
        effect: EffectId,
        /// Why.
        failure: PaymentFailure,
    },
    /// The outcome is indeterminate. Never retried blindly.
    Unknown {
        /// Which effect.
        effect: EffectId,
        /// What was observed.
        evidence: UnknownEvidence,
    },
}

impl PaymentResult {
    /// Which effect this result is about.
    #[must_use]
    pub const fn effect(&self) -> EffectId {
        match self {
            Self::Succeeded { effect, .. }
            | Self::Failed { effect, .. }
            | Self::Unknown { effect, .. } => *effect,
        }
    }

    /// The state this result moves the effect to.
    #[must_use]
    pub const fn state(&self) -> EffectState {
        match self {
            Self::Succeeded { .. } => EffectState::Succeeded,
            Self::Failed { .. } => EffectState::Failed,
            Self::Unknown { .. } => EffectState::OutcomeUnknown,
        }
    }

    /// Classifies an edge observation into a result.
    ///
    /// This is the only sanctioned way to build a `Failed`, which is what stops
    /// an indeterminate observation being recorded as a determinate refusal.
    #[must_use]
    pub fn from_failure(effect: EffectId, failure: PaymentFailure) -> Self {
        if failure.class.is_determinate() {
            Self::Failed { effect, failure }
        } else {
            Self::Unknown {
                effect,
                evidence: UnknownEvidence::AmbiguousResponse {
                    provider_code: failure.provider_code,
                },
            }
        }
    }
}

/// Kept so an edge cannot smuggle an address through a result.
///
/// Referencing the type here rather than in a result field is deliberate: it
/// documents that a customer address is an input to `EnsureCustomer` and never
/// an output of anything.
pub type CustomerAddressIsInputOnly = RedactedEmail;
