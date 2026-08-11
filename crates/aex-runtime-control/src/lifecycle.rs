//! Lifecycle: provider state mapping, the actively-enforced eight-hour lifetime,
//! lifecycle intents and ambiguous-outcome reconciliation.
//!
//! Provider states are passed through verbatim with no normalization and no
//! unknown-state fallback: an unmodelled string is a hard error, never guessed.

use core::fmt;
use core::str::FromStr;
use core::time::Duration;

use aex_hands_protocol::lifecycle::ProviderRequestId;
use aex_hands_protocol::rpc::Fence;
use aex_wire::ids::GenerationId;
use aex_wire::types::Timestamp;
use serde::{Deserialize, Serialize};

use crate::clock::{millis_between, plus_millis};
use crate::generation::GenerationState;

/// Provider-hard maximum retained compute lifetime, across running *and*
/// suspended time.
pub const PROVIDER_LIFETIME_MS: u64 = 28_800_000;

/// At `expires_at - this`, stop admitting new operations.
pub const LIFETIME_DRAIN_MARGIN_MS: u64 = 300_000;

/// At `expires_at - this`, terminate and close receipts cleanly.
pub const LIFETIME_TERMINATE_MARGIN_MS: u64 = 60_000;

/// Attempts before an unresolved lifecycle intent is quarantined.
pub const RECONCILE_ATTEMPTS: u32 = 8;

/// How long an untracked provider `MicroVM` may live before the orphan sweep ends it.
pub const ORPHAN_GRACE_MS: u64 = 600_000;

/// The provider's identifier for one `MicroVM`.
///
/// Provider-supplied and never normalized: AEX reconciles by exact identity, and
/// a "helpfully" rewritten id reconciles against the wrong machine.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct MicrovmId(pub String);

impl fmt::Display for MicrovmId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

/// The provider quota a capacity failure named.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ProviderQuotaId(pub String);

impl fmt::Display for ProviderQuotaId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

/// One recorded lifecycle intent.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct LifecycleIntentId(pub String);

impl fmt::Display for LifecycleIntentId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

/// A verbatim provider `MicroVM` state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ProviderState {
    /// The `MicroVM` is starting.
    Pending,
    /// The `MicroVM` is running.
    Running,
    /// The `MicroVM` is snapshotted and stopped.
    Suspended,
    /// The `MicroVM` is taking a snapshot.
    Suspending,
    /// The `MicroVM` is gone.
    Terminated,
    /// The `MicroVM` is being torn down.
    Terminating,
}

/// A provider state string outside the modelled set.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error(
    "`{observed}` is not a modelled MicroVM state; an unmodelled state is fatal, never guessed"
)]
pub struct UnmodelledProviderState {
    /// The string the provider returned, echoed verbatim.
    pub observed: String,
}

impl ProviderState {
    /// Every modelled provider state.
    pub const ALL: [Self; 6] = [
        Self::Pending,
        Self::Running,
        Self::Suspended,
        Self::Suspending,
        Self::Terminated,
        Self::Terminating,
    ];

    /// The provider's own spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "PENDING",
            Self::Running => "RUNNING",
            Self::Suspended => "SUSPENDED",
            Self::Suspending => "SUSPENDING",
            Self::Terminated => "TERMINATED",
            Self::Terminating => "TERMINATING",
        }
    }

    /// The AEX generation state this provider state maps onto.
    ///
    /// The mapping is a pure function of the observed provider state and the
    /// recorded AEX state: `RUNNING` while AEX is draining stays draining, and
    /// `TERMINATED` observed out of band is `lost` rather than a clean retirement.
    #[must_use]
    pub const fn map_onto(self, recorded: GenerationState) -> GenerationState {
        match self {
            Self::Pending => GenerationState::Launching,
            Self::Running => match recorded {
                GenerationState::LifetimeDraining => GenerationState::LifetimeDraining,
                _ => GenerationState::Running,
            },
            Self::Suspended => GenerationState::Suspended,
            Self::Suspending => GenerationState::Suspending,
            Self::Terminating => GenerationState::Terminating,
            Self::Terminated => match recorded {
                GenerationState::Terminating | GenerationState::Terminated => {
                    GenerationState::Terminated
                }
                _ => GenerationState::Lost,
            },
        }
    }
}

impl FromStr for ProviderState {
    type Err = UnmodelledProviderState;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        Self::ALL
            .into_iter()
            .find(|state| state.as_str() == text)
            .ok_or_else(|| UnmodelledProviderState {
                observed: text.to_owned(),
            })
    }
}

/// A lifecycle action AEX asks the provider to perform.
///
/// Named `LifecycleAction`, not `LifecycleIntent`: the contract's
/// `aex_hands_protocol::lifecycle::LifecycleIntent` already names the transported
/// intent union, and [`IntentRecord`] is the durable record. Three same-named
/// types in one call graph is a review hazard.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LifecycleAction {
    /// `RunMicrovm`.
    Launch,
    /// `SuspendMicrovm`.
    Suspend,
    /// `ResumeMicrovm`.
    Resume,
    /// Provider-native same-generation resume caused by authenticated endpoint
    /// traffic. No `ResumeMicrovm` request exists for this action.
    NativeResume,
    /// `TerminateMicrovm`.
    Terminate,
}

impl LifecycleAction {
    /// Every modelled action.
    pub const ALL: [Self; 5] = [
        Self::Launch,
        Self::Suspend,
        Self::Resume,
        Self::NativeResume,
        Self::Terminate,
    ];

    /// The receipt-identity component this action contributes.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Launch => "run",
            Self::Suspend => "suspend",
            Self::Resume => "resume",
            Self::NativeResume => "native_resume",
            Self::Terminate => "terminate",
        }
    }
}

/// Which transient failure class was observed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TransientClass {
    /// `InternalServerException`.
    InternalServer,
    /// `ServiceUnavailableException`.
    ServiceUnavailable,
    /// A 5xx status with no modelled exception.
    ServerStatus,
    /// A connection-level failure with `$fault = server`.
    ServerFault,
}

/// Provider detail carried into diagnostics with every value elided.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct RedactedDetail(Box<str>);

impl RedactedDetail {
    /// Wraps a provider message that has already been proven free of customer and
    /// credential material.
    #[must_use]
    pub fn new(text: impl Into<Box<str>>) -> Self {
        Self(text.into())
    }

    /// The redacted text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for RedactedDetail {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

/// How a provider call ended. Never `anyhow`, never a bare string.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ProviderCall {
    /// The provider throttled the call.
    #[error("provider throttled; retry after {retry_after:?}")]
    Throttled {
        /// The provider's advertised backoff.
        retry_after: Duration,
    },
    /// A regional pool or account quota is exhausted.
    #[error("provider capacity exhausted on quota `{quota}`")]
    Capacity {
        /// The named quota.
        quota: ProviderQuotaId,
    },
    /// A retryable server-side failure.
    #[error("transient provider failure: {class:?}")]
    Transient {
        /// Which transient class was observed.
        class: TransientClass,
    },
    /// The named resource does not exist.
    #[error("provider resource not found")]
    NotFound,
    /// The request was rejected before any effect.
    #[error("provider rejected field `{field}`")]
    Invalid {
        /// The field the provider named.
        field: Box<str>,
        /// Redacted provider detail.
        detail: RedactedDetail,
    },
    /// The outcome is unknown. Never retried blindly, never assumed successful.
    #[error("provider outcome unknown")]
    Unknown {
        /// The request id, when the provider produced one before the ambiguity.
        request: Option<ProviderRequestId>,
    },
    /// A modelled, non-retryable provider failure.
    #[error("fatal provider failure `{code}`")]
    Fatal {
        /// The provider's error code.
        code: Box<str>,
    },
}

/// The retry policy for one provider outcome.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RetryPolicy {
    /// Retry inside the adapter with jitter in the given inclusive range.
    InAdapter {
        /// Lowest backoff, milliseconds.
        min_ms: u64,
        /// Highest backoff, milliseconds.
        max_ms: u64,
        /// Attempts before giving up.
        attempts: u32,
    },
    /// Do not retry here; take the long durable backoff at the worker.
    Durable {
        /// Lowest backoff, milliseconds.
        min_ms: u64,
        /// Highest backoff, milliseconds.
        max_ms: u64,
    },
    /// Never retry; reconcile by exact identity.
    Reconcile,
    /// Never retry; the call cannot succeed.
    Never,
}

impl ProviderCall {
    /// How this outcome is handled.
    ///
    /// `Capacity` deliberately does **not** retry in-adapter: a full regional
    /// memory pool must take the long durable backoff at the worker instead of
    /// burning the API rate. `Unknown` never retries; it reconciles.
    #[must_use]
    pub const fn retry_policy(&self) -> RetryPolicy {
        match self {
            Self::Throttled { .. } => RetryPolicy::InAdapter {
                min_ms: 1_000,
                max_ms: 5_000,
                attempts: 3,
            },
            Self::Transient { .. } => RetryPolicy::InAdapter {
                min_ms: 100,
                max_ms: 300,
                attempts: 3,
            },
            Self::Capacity { .. } => RetryPolicy::Durable {
                min_ms: 15_000,
                max_ms: 60_000,
            },
            Self::Unknown { .. } => RetryPolicy::Reconcile,
            Self::NotFound | Self::Invalid { .. } | Self::Fatal { .. } => RetryPolicy::Never,
        }
    }

    /// The exponential backoff for attempt `n` (one-based) under an in-adapter
    /// transient policy: `100 ms x 2^(n-1)`, capped at 300 ms.
    #[must_use]
    pub const fn transient_backoff_ms(attempt: u32) -> u64 {
        let shift = if attempt == 0 { 0 } else { attempt - 1 };
        let raw = if shift >= 16 { 300 } else { 100_u64 << shift };
        if raw > 300 { 300 } else { raw }
    }
}

/// What the lifetime margins say about a generation right now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LifetimeVerdict {
    /// More than the drain margin remains.
    Continue {
        /// Remaining lifetime in milliseconds.
        remaining_ms: u64,
    },
    /// Inside the drain margin: refuse new admissions, let open work finish.
    Drain {
        /// Remaining lifetime in milliseconds.
        remaining_ms: u64,
    },
    /// Inside the terminate margin: terminate and close receipts cleanly.
    Terminate {
        /// Remaining lifetime in milliseconds.
        remaining_ms: u64,
    },
}

/// The eight-hour lifetime, actively enforced rather than passively observed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct Lifetime {
    /// When the provider started counting.
    pub launched_at: Timestamp,
}

impl Lifetime {
    /// When the provider hard-stops this generation.
    #[must_use]
    pub fn expires_at(self) -> Timestamp {
        plus_millis(self.launched_at, PROVIDER_LIFETIME_MS)
    }

    /// Milliseconds of provider lifetime left at `now`.
    #[must_use]
    pub fn remaining_ms(self, now: Timestamp) -> u64 {
        millis_between(now, self.expires_at())
    }

    /// The drain/terminate decision at `now`.
    #[must_use]
    pub fn verdict(self, now: Timestamp) -> LifetimeVerdict {
        let remaining_ms = self.remaining_ms(now);
        if remaining_ms <= LIFETIME_TERMINATE_MARGIN_MS {
            LifetimeVerdict::Terminate { remaining_ms }
        } else if remaining_ms <= LIFETIME_DRAIN_MARGIN_MS {
            LifetimeVerdict::Drain { remaining_ms }
        } else {
            LifetimeVerdict::Continue { remaining_ms }
        }
    }

    /// Whether a resume is permitted, or refused in favour of a new generation.
    ///
    /// # Errors
    ///
    /// Returns [`ResumeRefused`] when at most [`LIFETIME_TERMINATE_MARGIN_MS`] of
    /// provider lifetime remains.
    pub fn permit_resume(self, now: Timestamp) -> Result<u64, ResumeRefused> {
        let remaining_ms = self.remaining_ms(now);
        if remaining_ms <= LIFETIME_TERMINATE_MARGIN_MS {
            Err(ResumeRefused { remaining_ms })
        } else {
            Ok(remaining_ms)
        }
    }
}

/// A resume refused because the generation is inside its terminate margin.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error(
    "resume refused with {remaining_ms} ms of provider lifetime left; allocate a new generation"
)]
pub struct ResumeRefused {
    /// Provider lifetime remaining in milliseconds.
    pub remaining_ms: u64,
}

/// Where a lifecycle intent stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IntentState {
    /// Written, call dispatched or about to be. Blocks any second effect.
    Dispatched,
    /// The provider outcome was ambiguous; reconciliation owns it.
    Unknown,
    /// Settled with a receipt.
    Settled,
    /// Exhausted reconciliation; an operator record and an alarm exist.
    Quarantined,
}

/// The durable record written **before** a lifecycle call, in the same conditional
/// write that takes the fence.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct IntentRecord {
    /// Identity of this intent.
    pub intent_id: LifecycleIntentId,
    /// The generation it acts on.
    pub generation: GenerationId,
    /// The provider `MicroVM`, absent for a launch that has not produced one yet.
    pub microvm: Option<MicrovmId>,
    /// What is being attempted.
    pub action: LifecycleAction,
    /// The fence the intent took.
    pub fence: Fence,
    /// Where the intent stands.
    pub state: IntentState,
    /// The provider request id, once one exists.
    pub provider_request_id: Option<ProviderRequestId>,
    /// Reconciliation attempts spent.
    pub attempts: u32,
    /// When the intent was recorded.
    pub dispatched_at: Timestamp,
}

/// What reconciliation should do next for an ambiguous intent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReconcileStep {
    /// Probe the provider by exact `MicroVM` identity.
    Probe {
        /// The `MicroVM` to probe.
        microvm: MicrovmId,
    },
    /// The launch never produced a `MicroVM` id, so re-issue the identical
    /// `RunMicrovm` with the same client token. That is the only idempotent path
    /// available and it also covers the pre-`vmId` window.
    ReissueLaunch {
        /// The deterministic replay identity.
        client_token: String,
    },
    /// Attempts are exhausted: quarantine with an operator record and an alarm.
    /// Never redrive hot.
    Quarantine {
        /// Attempts spent.
        attempts: u32,
    },
}

impl IntentRecord {
    /// The receipt identity for a settled intent.
    ///
    /// A missing provider request id is a hard error rather than an invented
    /// identity, because the request id is the only evidence an empty-bodied
    /// suspend or resume response carries.
    ///
    /// # Errors
    ///
    /// Returns [`MissingProviderEvidence`] when either the `MicroVM` id or the
    /// provider request id is absent.
    pub fn receipt_id(&self) -> Result<String, MissingProviderEvidence> {
        let microvm = self
            .microvm
            .as_ref()
            .ok_or(MissingProviderEvidence::Microvm)?;
        let request = self
            .provider_request_id
            .as_ref()
            .ok_or(MissingProviderEvidence::RequestId)?;
        Ok(format!(
            "lambda-microvm:{microvm}:{}:{}",
            self.action.as_str(),
            request.0
        ))
    }

    /// Whether a second lifecycle effect may be dispatched for this generation.
    ///
    /// Enforced by the intent's conditional write, not by a timer.
    #[must_use]
    pub const fn permits_new_effect(&self) -> bool {
        matches!(self.state, IntentState::Settled)
    }

    /// The next reconciliation step.
    #[must_use]
    pub fn next_reconcile_step(&self, client_token: &str) -> ReconcileStep {
        if self.attempts >= RECONCILE_ATTEMPTS {
            return ReconcileStep::Quarantine {
                attempts: self.attempts,
            };
        }
        match &self.microvm {
            Some(microvm) => ReconcileStep::Probe {
                microvm: microvm.clone(),
            },
            None => ReconcileStep::ReissueLaunch {
                client_token: client_token.to_owned(),
            },
        }
    }
}

/// Provider evidence AEX refuses to invent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum MissingProviderEvidence {
    /// No `MicroVM` id.
    #[error("the intent carries no MicroVM id; a receipt identity is never invented")]
    Microvm,
    /// No provider request id.
    #[error("the provider returned no request id; a receipt identity is never invented")]
    RequestId,
}

/// The deterministic launch replay identity.
///
/// Derived from the generation alone — not from an agent, session or epoch — so a
/// repeated `RunMicrovm` returns the same `MicroVM` and a lost response, a mux crash
/// between dispatch and commit and a cross-process race all converge on one VM.
#[must_use]
pub fn client_token(generation: GenerationId) -> String {
    format!("aexgen-{generation}")
}

/// The AEX-minted snapshot lifecycle identity.
///
/// The provider exposes no snapshot operation, identity or timestamp: the RAM+disk
/// snapshot is a side effect of `SuspendMicrovm`. This identity is therefore an AEX
/// construct and is never an AWS snapshot id.
#[must_use]
pub fn snapshot_lifecycle_id(microvm: &MicrovmId, ordinal: u32) -> String {
    format!("snapshot-lifecycle:{microvm}:{ordinal}")
}

#[cfg(test)]
mod tests {
    use super::{
        IntentRecord, IntentState, LIFETIME_DRAIN_MARGIN_MS, LIFETIME_TERMINATE_MARGIN_MS,
        LifecycleAction, LifecycleIntentId, Lifetime, LifetimeVerdict, MicrovmId,
        MissingProviderEvidence, PROVIDER_LIFETIME_MS, ProviderCall, ProviderQuotaId,
        ProviderState, RECONCILE_ATTEMPTS, ReconcileStep, RedactedDetail, ResumeRefused,
        RetryPolicy, TransientClass, client_token, snapshot_lifecycle_id,
    };
    use crate::clock::{minus_millis, plus_millis};
    use crate::generation::GenerationState;
    use aex_hands_protocol::lifecycle::ProviderRequestId;
    use aex_hands_protocol::rpc::Fence;
    use aex_wire::ids::{GenerationId, PrefixedId as _, Uuid7};
    use aex_wire::types::Timestamp;
    use core::time::Duration;

    const LAUNCHED_AT: i64 = 10_000_000;

    fn lifetime() -> Lifetime {
        Lifetime {
            launched_at: Timestamp::from_unix_millis(LAUNCHED_AT).expect("a bounded instant"),
        }
    }

    fn at(remaining_ms: u64) -> Timestamp {
        minus_millis(lifetime().expires_at(), remaining_ms)
    }

    fn generation() -> GenerationId {
        GenerationId::from_uuid7(Uuid7::compose(6, [6; 10]))
    }

    #[test]
    fn every_provider_state_round_trips_and_an_unmodelled_string_is_fatal() {
        for state in ProviderState::ALL {
            assert_eq!(state.as_str().parse::<ProviderState>(), Ok(state));
        }
        let error = "PAUSED"
            .parse::<ProviderState>()
            .expect_err("an unmodelled state is never guessed");
        assert_eq!(error.observed, "PAUSED");
        for guess in ["", "running", "Running", "STOPPED", "UNKNOWN"] {
            assert!(guess.parse::<ProviderState>().is_err(), "{guess}");
        }
    }

    #[test]
    fn the_state_mapping_table_is_exact() {
        use GenerationState as G;
        use ProviderState as P;
        let table = [
            (P::Pending, G::Launching, G::Launching),
            (P::Running, G::Launching, G::Running),
            (P::Running, G::Resuming, G::Running),
            (P::Running, G::LifetimeDraining, G::LifetimeDraining),
            (P::Suspending, G::Running, G::Suspending),
            (P::Suspended, G::Suspending, G::Suspended),
            (P::Terminating, G::Running, G::Terminating),
            (P::Terminated, G::Terminating, G::Terminated),
            (P::Terminated, G::Running, G::Lost),
            (P::Terminated, G::Suspended, G::Lost),
        ];
        for (provider, recorded, expected) in table {
            assert_eq!(
                provider.map_onto(recorded),
                expected,
                "{provider:?} observed while AEX recorded {recorded:?}"
            );
        }
    }

    #[test]
    fn capacity_never_retries_in_adapter() {
        let capacity = ProviderCall::Capacity {
            quota: ProviderQuotaId("microvm-memory-gib".to_owned()),
        };
        assert_eq!(
            capacity.retry_policy(),
            RetryPolicy::Durable {
                min_ms: 15_000,
                max_ms: 60_000
            }
        );
    }

    #[test]
    fn every_provider_outcome_has_a_stated_retry_policy() {
        let cases = [
            (
                ProviderCall::Throttled {
                    retry_after: Duration::from_secs(1),
                },
                RetryPolicy::InAdapter {
                    min_ms: 1_000,
                    max_ms: 5_000,
                    attempts: 3,
                },
            ),
            (
                ProviderCall::Transient {
                    class: TransientClass::InternalServer,
                },
                RetryPolicy::InAdapter {
                    min_ms: 100,
                    max_ms: 300,
                    attempts: 3,
                },
            ),
            (ProviderCall::NotFound, RetryPolicy::Never),
            (
                ProviderCall::Invalid {
                    field: "idlePolicy".into(),
                    detail: RedactedDetail::new("out of range"),
                },
                RetryPolicy::Never,
            ),
            (
                ProviderCall::Fatal {
                    code: "AccessDeniedException".into(),
                },
                RetryPolicy::Never,
            ),
            (
                ProviderCall::Unknown { request: None },
                RetryPolicy::Reconcile,
            ),
        ];
        for (call, expected) in cases {
            assert_eq!(call.retry_policy(), expected, "{call:?}");
        }
    }

    #[test]
    fn transient_backoff_doubles_and_caps_at_three_hundred_milliseconds() {
        assert_eq!(ProviderCall::transient_backoff_ms(1), 100);
        assert_eq!(ProviderCall::transient_backoff_ms(2), 200);
        assert_eq!(ProviderCall::transient_backoff_ms(3), 300);
        assert_eq!(ProviderCall::transient_backoff_ms(9), 300);
        assert_eq!(ProviderCall::transient_backoff_ms(u32::MAX), 300);
    }

    #[test]
    fn the_lifetime_margins_are_exact() {
        assert_eq!(
            lifetime().expires_at(),
            Timestamp::from_unix_millis(LAUNCHED_AT + 28_800_000).expect("a bounded instant")
        );
        let cases = [
            (PROVIDER_LIFETIME_MS, false, false),
            (LIFETIME_DRAIN_MARGIN_MS + 1, false, false),
            (LIFETIME_DRAIN_MARGIN_MS, true, false),
            (LIFETIME_TERMINATE_MARGIN_MS + 1, true, false),
            (LIFETIME_TERMINATE_MARGIN_MS, false, true),
            (0, false, true),
        ];
        for (remaining, draining, terminating) in cases {
            let verdict = lifetime().verdict(at(remaining));
            match verdict {
                LifetimeVerdict::Continue { remaining_ms } => {
                    assert!(!draining && !terminating, "{remaining} ms -> {verdict:?}");
                    assert_eq!(remaining_ms, remaining);
                }
                LifetimeVerdict::Drain { remaining_ms } => {
                    assert!(draining, "{remaining} ms -> {verdict:?}");
                    assert_eq!(remaining_ms, remaining);
                }
                LifetimeVerdict::Terminate { remaining_ms } => {
                    assert!(terminating, "{remaining} ms -> {verdict:?}");
                    assert_eq!(remaining_ms, remaining);
                }
            }
        }
    }

    #[test]
    fn remaining_lifetime_floors_at_zero_after_expiry() {
        let past_expiry = plus_millis(lifetime().expires_at(), 1_000);
        assert_eq!(lifetime().remaining_ms(past_expiry), 0);
        assert_eq!(
            lifetime().verdict(past_expiry),
            LifetimeVerdict::Terminate { remaining_ms: 0 }
        );
    }

    #[test]
    fn resume_is_refused_inside_the_terminate_margin() {
        assert_eq!(
            lifetime().permit_resume(at(LIFETIME_TERMINATE_MARGIN_MS + 1)),
            Ok(LIFETIME_TERMINATE_MARGIN_MS + 1)
        );
        assert_eq!(
            lifetime().permit_resume(at(LIFETIME_TERMINATE_MARGIN_MS)),
            Err(ResumeRefused {
                remaining_ms: LIFETIME_TERMINATE_MARGIN_MS
            })
        );
        assert_eq!(
            lifetime().permit_resume(at(0)),
            Err(ResumeRefused { remaining_ms: 0 })
        );
    }

    fn intent(state: IntentState, microvm: Option<&str>, attempts: u32) -> IntentRecord {
        IntentRecord {
            intent_id: LifecycleIntentId("lci_5".to_owned()),
            generation: generation(),
            microvm: microvm.map(|id| MicrovmId(id.to_owned())),
            action: LifecycleAction::Suspend,
            fence: Fence(2),
            state,
            provider_request_id: Some(ProviderRequestId("req-123".to_owned())),
            attempts,
            dispatched_at: Timestamp::from_unix_millis(1).expect("a bounded instant"),
        }
    }

    #[test]
    fn a_dispatched_or_unknown_intent_blocks_any_second_effect() {
        assert!(!intent(IntentState::Dispatched, Some("mvm-1"), 0).permits_new_effect());
        assert!(!intent(IntentState::Unknown, Some("mvm-1"), 0).permits_new_effect());
        assert!(intent(IntentState::Settled, Some("mvm-1"), 0).permits_new_effect());
        assert!(
            !intent(IntentState::Quarantined, Some("mvm-1"), 8).permits_new_effect(),
            "quarantine is an operator fence, not permission for an automatic second effect"
        );
    }

    #[test]
    fn reconciliation_probes_by_identity_reissues_before_a_vm_id_and_quarantines_after_eight() {
        let token = client_token(generation());
        assert_eq!(
            intent(IntentState::Unknown, Some("mvm-1"), 0).next_reconcile_step(&token),
            ReconcileStep::Probe {
                microvm: MicrovmId("mvm-1".to_owned())
            }
        );
        assert_eq!(
            intent(IntentState::Unknown, None, 3).next_reconcile_step(&token),
            ReconcileStep::ReissueLaunch {
                client_token: token.clone()
            }
        );
        assert_eq!(
            intent(IntentState::Unknown, Some("mvm-1"), RECONCILE_ATTEMPTS)
                .next_reconcile_step(&token),
            ReconcileStep::Quarantine {
                attempts: RECONCILE_ATTEMPTS
            }
        );
    }

    #[test]
    fn a_receipt_identity_is_never_invented() {
        let settled = intent(IntentState::Settled, Some("mvm-9"), 0);
        assert_eq!(
            settled.receipt_id(),
            Ok("lambda-microvm:mvm-9:suspend:req-123".to_owned())
        );
        assert_eq!(
            intent(IntentState::Settled, None, 0).receipt_id(),
            Err(MissingProviderEvidence::Microvm)
        );
        let mut no_request = intent(IntentState::Settled, Some("mvm-9"), 0);
        no_request.provider_request_id = None;
        assert_eq!(
            no_request.receipt_id(),
            Err(MissingProviderEvidence::RequestId)
        );
    }

    #[test]
    fn every_action_contributes_its_own_receipt_component() {
        for (action, expected) in [
            (LifecycleAction::Launch, "run"),
            (LifecycleAction::Suspend, "suspend"),
            (LifecycleAction::Resume, "resume"),
            (LifecycleAction::NativeResume, "native_resume"),
            (LifecycleAction::Terminate, "terminate"),
        ] {
            assert_eq!(action.as_str(), expected);
        }
    }

    #[test]
    fn the_replay_identity_is_derived_from_the_generation_alone() {
        let generation = generation();
        let token = client_token(generation);
        assert_eq!(token, format!("aexgen-{generation}"));
        assert!(token.starts_with("aexgen-gen_"));
        assert_eq!(
            token.len(),
            37,
            "`aexgen-` plus the 30-character generation id spelling"
        );
        assert_eq!(
            token,
            client_token(generation),
            "the token is deterministic, so a replay is safe"
        );
        // A different generation is a different token, so replay cannot collapse
        // two generations onto one MicroVM.
        let other = GenerationId::from_uuid7(Uuid7::compose(7, [7; 10]));
        assert_ne!(client_token(other), token);
    }

    #[test]
    fn the_snapshot_identity_is_an_aex_construct_and_names_no_aws_snapshot() {
        let id = snapshot_lifecycle_id(&MicrovmId("mvm-7".to_owned()), 0);
        assert_eq!(id, "snapshot-lifecycle:mvm-7:0");
        assert!(
            id.starts_with("snapshot-lifecycle:"),
            "the prefix states that AEX minted this identity"
        );
        assert!(
            !id.contains("snap-"),
            "an AWS snapshot id shape must never appear here"
        );
        assert_eq!(
            snapshot_lifecycle_id(&MicrovmId("mvm-7".to_owned()), 3),
            "snapshot-lifecycle:mvm-7:3"
        );
    }
}
