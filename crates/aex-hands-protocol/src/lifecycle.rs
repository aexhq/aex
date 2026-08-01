//! The provider control plane. Never the guest.
//!
//! Everything here describes what AEX asked a `MicroVM` provider to do and what
//! the provider answered. The guest has no voice in this module at all, which is
//! the whole point: a customer with root inside the guest can falsify any
//! in-guest counter, so the only billable evidence is a provider response.

use aex_wire::ids::GenerationId;
use aex_wire::types::{ComputeSize, DecimalU128, Timestamp};
use serde::{Deserialize, Serialize};

/// A provider-assigned receipt identity.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ProviderReceiptId(pub String);

/// A provider-assigned request identity, used to reconcile an unknown outcome.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ProviderRequestId(pub String);

/// Why the provider refused.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ProviderFailure {
    /// A stable, non-localized reason.
    pub reason: String,
    /// Whether an identical retry can succeed.
    pub retryable: bool,
}

/// A keepalive lease that holds a generation warm.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct KeepaliveLease {
    /// The lease identity.
    pub lease_id: String,
    /// When it stops holding.
    pub expires_at: Timestamp,
}

/// What AEX asked the provider control plane to do.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "intent",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub enum LifecycleIntent {
    /// Launch a new generation.
    Launch {
        /// Which generation.
        generation: GenerationId,
        /// Which shape.
        shape: ComputeSize,
    },
    /// Resume a suspended generation.
    Resume {
        /// Which generation.
        generation: GenerationId,
    },
    /// Suspend a running generation.
    Suspend {
        /// Which generation.
        generation: GenerationId,
    },
    /// Terminate a generation.
    Terminate {
        /// Which generation.
        generation: GenerationId,
    },
    /// Snapshot a generation.
    Snapshot {
        /// Which generation.
        generation: GenerationId,
    },
}

/// What the provider answered.
///
/// [`LifecycleOutcome::Unknown`] is first class. It is reconciled by exact
/// `MicroVM` identity, never by assuming success and never by retrying blindly:
/// a blind retry on an unknown launch is how one session gets two guests.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "outcome",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub enum LifecycleOutcome {
    /// The provider applied the intent.
    Applied {
        /// The provider receipt.
        receipt: ProviderReceiptId,
        /// When the provider says it happened.
        observed_at: Timestamp,
    },
    /// The provider refused.
    Rejected {
        /// Why.
        failure: ProviderFailure,
    },
    /// The outcome is indeterminate and must be reconciled.
    Unknown {
        /// The request to reconcile by.
        request: ProviderRequestId,
    },
}

/// Why a runtime receipt did not add up.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum ReceiptError {
    /// The interval ran backwards.
    #[error("a receipt interval must move forward in time")]
    NotForwardInTime,
    /// The accounted milliseconds did not exhaust the interval.
    #[error("running_ms + suspended_ms must exhaust the receipt interval exactly")]
    UnexplainedRemainder,
}

/// The only billable Hands evidence.
///
/// There is deliberately no constructor from guest input. `transmit_bytes` is
/// `None` when the provider exposes no per-generation transmit receipt, and the
/// launch rate book then forbids charging transfer at all — an absent measure is
/// recorded as absent rather than estimated.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct RuntimeReceipt {
    /// Which generation.
    pub generation: GenerationId,
    /// Which shape it ran at.
    pub shape: ComputeSize,
    /// Milliseconds spent running.
    pub running_ms: u64,
    /// Milliseconds spent suspended.
    pub suspended_ms: u64,
    /// The start of the accounted interval.
    pub from: Timestamp,
    /// The end of the accounted interval.
    pub to: Timestamp,
    /// Retained snapshot bytes, when the provider reported any.
    pub snapshot_bytes: Option<DecimalU128>,
    /// Transmitted bytes; `None` means the provider exposes no such receipt.
    pub transmit_bytes: Option<DecimalU128>,
}

impl RuntimeReceipt {
    /// Checks that the accounted milliseconds exhaust the interval.
    ///
    /// An unexplained remainder over-charges if billed as running and
    /// under-charges if dropped, so it is a hard error either way rather than a
    /// choice made silently at the call site.
    ///
    /// # Errors
    ///
    /// Returns [`ReceiptError`] when the interval runs backwards or the
    /// milliseconds do not sum to it exactly.
    pub fn validate(&self) -> Result<(), ReceiptError> {
        let span = self.to.unix_millis() - self.from.unix_millis();
        if span < 0 {
            return Err(ReceiptError::NotForwardInTime);
        }
        let accounted = i64::try_from(self.running_ms.saturating_add(self.suspended_ms))
            .map_err(|_| ReceiptError::UnexplainedRemainder)?;
        if accounted != span {
            return Err(ReceiptError::UnexplainedRemainder);
        }
        Ok(())
    }
}

/// Brain-authoritative evidence that a generation is genuinely idle.
///
/// Guest silence proves nothing: a guest can be quiet because it is finished or
/// because it is wedged, and only Brain knows whether anything is still admitted,
/// queued or open.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct TrueIdleEvidence {
    /// Which generation.
    pub generation: GenerationId,
    /// The activity revision this evidence was taken at.
    pub activity_revision: u64,
    /// How many operations Brain has admitted and not yet settled.
    pub admitted: u32,
    /// How many are queued.
    pub queued: u32,
    /// How many connections are open.
    pub open: u32,
    /// A keepalive lease that holds the generation warm regardless.
    pub keepalive_lease: Option<KeepaliveLease>,
    /// When the evidence was taken.
    pub observed_at: Timestamp,
}

impl TrueIdleEvidence {
    /// Whether the generation is genuinely idle.
    #[must_use]
    pub const fn is_true_idle(&self) -> bool {
        self.admitted == 0 && self.queued == 0 && self.open == 0 && self.keepalive_lease.is_none()
    }
}
