//! Provider effect state and honest unknown-outcome recovery.

use time::{Duration, OffsetDateTime};

/// Provider effect kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EffectKind {
    /// Customer creation.
    CustomerCreate,
    /// Hosted checkout creation.
    CheckoutSessionCreate,
    /// Off-session payment intent.
    OffSessionCharge,
    /// Refund creation.
    RefundCreate,
    /// Tax calculation.
    TaxCalculationCreate,
    /// Tax transaction.
    TaxTransactionCreate,
}

/// Durable effect state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EffectState {
    /// Committed before provider dispatch.
    Prepared,
    /// Dispatch began.
    Dispatched,
    /// Provider success, terminal.
    Succeeded,
    /// Deterministic provider rejection, terminal.
    Failed,
    /// Provider may have accepted the effect.
    OutcomeUnknown,
    /// Automatic recovery is unsafe.
    ManualReview,
}

/// Provider object returned by success.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderObject {
    /// Stable provider object id.
    pub id: String,
}

/// Deterministic provider decline.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeclineCode(pub String);

/// Why dispatch outcome cannot be known.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IndeterminateReason {
    /// Request timed out.
    Timeout,
    /// Provider returned a 5xx response.
    Provider5xx,
    /// Connection reset around dispatch.
    ConnectionReset,
}

/// One observed provider outcome.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EffectOutcome {
    /// Confirmed success.
    Succeeded(ProviderObject),
    /// Confirmed deterministic decline.
    Failed(DeclineCode),
    /// May have succeeded.
    Indeterminate(IndeterminateReason),
}

/// Durable effect aggregate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderEffect {
    kind: EffectKind,
    state: EffectState,
    prepared_at: OffsetDateTime,
    first_dispatch_at: Option<OffsetDateTime>,
    provider_object_id: Option<String>,
}

impl ProviderEffect {
    /// Creates a prepared effect before a network call is allowed.
    #[must_use]
    pub const fn prepare(kind: EffectKind, now: OffsetDateTime) -> Self {
        Self {
            kind,
            state: EffectState::Prepared,
            prepared_at: now,
            first_dispatch_at: None,
            provider_object_id: None,
        }
    }

    /// Marks dispatch before calling the provider.
    ///
    /// # Errors
    /// Only a prepared effect may begin dispatch.
    pub fn dispatch(&self, now: OffsetDateTime) -> Result<Self, EffectTransitionError> {
        if self.state != EffectState::Prepared {
            return Err(EffectTransitionError::InvalidTransition);
        }
        let mut next = self.clone();
        next.state = EffectState::Dispatched;
        next.first_dispatch_at = Some(now);
        Ok(next)
    }

    /// State.
    #[must_use]
    pub const fn state(&self) -> EffectState {
        self.state
    }
}

/// Applies an outcome without conflating indeterminate failure with decline.
///
/// # Errors
/// Only a dispatched effect accepts an outcome.
pub fn transition(
    effect: &ProviderEffect,
    outcome: EffectOutcome,
    _now: OffsetDateTime,
) -> Result<ProviderEffect, EffectTransitionError> {
    if effect.state != EffectState::Dispatched {
        return Err(EffectTransitionError::InvalidTransition);
    }
    let mut next = effect.clone();
    match outcome {
        EffectOutcome::Succeeded(object) => {
            next.state = EffectState::Succeeded;
            next.provider_object_id = Some(object.id);
        }
        EffectOutcome::Failed(_) => next.state = EffectState::Failed,
        EffectOutcome::Indeterminate(_) => next.state = EffectState::OutcomeUnknown,
    }
    Ok(next)
}

/// Recovery operation permitted by durable facts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RecoveryAction {
    /// Repeat the exact request and idempotency key within the safe window.
    RetryExactKey,
    /// Retrieve the known provider object.
    LookupByObject(String),
    /// Search provider metadata for the effect id.
    SearchByEffectId,
    /// Fence and require a human decision.
    EscalateManualReview,
}

/// Chooses recovery at the strict 12-hour boundary.
#[must_use]
pub fn recovery_action(effect: &ProviderEffect, now: OffsetDateTime) -> RecoveryAction {
    if effect.state != EffectState::OutcomeUnknown {
        return RecoveryAction::EscalateManualReview;
    }
    let dispatched = effect.first_dispatch_at.unwrap_or(effect.prepared_at);
    if now - dispatched < Duration::hours(12) {
        RecoveryAction::RetryExactKey
    } else if let Some(id) = &effect.provider_object_id {
        RecoveryAction::LookupByObject(id.clone())
    } else {
        RecoveryAction::EscalateManualReview
    }
}

/// Invalid provider-effect transition.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum EffectTransitionError {
    /// State and event do not form a legal edge.
    #[error("provider effect transition is invalid")]
    InvalidTransition,
}
