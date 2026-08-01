//! The `METER-02` runtime measurement probe library.
//!
//! Behind the non-default `probe` feature. The pure arithmetic — minute accrual,
//! CPU proportional scaling, the byte-millisecond integral, quantity bounds —
//! lives in `aex_usage_domain::interval`, where it is testable with no
//! dependencies at all. This module is the *library*: the scopes, tokens,
//! counters, clocks and the sink that turn a running process into
//! [`FactDraft`](aex_usage_domain::fact::FactDraft) values.
//!
//! Two things this module deliberately cannot express:
//!
//! - **A guest-reported number.** Every [`Evidence`](aex_usage_domain::measurement::Evidence)
//!   variant names a receipt, a counter, a cgroup reading or a reservation the
//!   platform itself holds. There is no constructor that turns a log line, a
//!   trace, an `OTel` metric or a customer-supplied counter into a fact, because
//!   customer root can falsify all four.
//! - **An estimate.** Where evidence is absent the probe returns a typed
//!   refusal — `NoTransmitEvidence`, `UnexhaustedLifetime`, `PhysicalSource` —
//!   rather than a plausible number. An unbilled byte is a cost; an invented one
//!   is a false invoice.

pub mod body;
pub mod clock;
pub mod cpu;
pub mod drain;
pub mod egress;
pub mod memory;
pub mod reconciler;
pub mod shape;
pub mod sink;
pub mod storage;

#[cfg(test)]
pub(crate) mod testing;

use aex_usage_domain::fact::ResourceGeneration;
use aex_usage_domain::identity::IdentityError;
use aex_usage_domain::interval::IntervalError;
use aex_usage_domain::measurement::MeasurementError;
use aex_usage_domain::wire_pending::{
    IdError, OrganizationId, PricingVersion, RegionId, ReservationId, ServiceId, Timestamp,
    TimestampError, WorkspaceId,
};
use time::OffsetDateTime;

pub use body::CountingBody;
pub use clock::{
    CGROUP_V2_CPU_STAT, CGROUP_V2_MEMORY_CURRENT, CgroupV2Cpu, CgroupV2Memory, CpuInstant,
    CpuMicros, PhysicalCpuSource, PhysicalMemorySource, RustixThreadCpuClock, SystemWallClock,
    ThreadCpuClock, WallClock,
};
pub use cpu::{
    ActivationKey, ActivationMeter, ActivationScope, ActivationScoped, CpuJob,
    MAX_ATTRIBUTED_POLL_US,
};
pub use drain::{CancelToken, FactDrain, Pacer, SleepPacer};
pub use egress::{BoundaryReceipt, EgressCounter, egress_from_receipt};
pub use memory::{MemoryBudget, MemoryReport, MemoryReservation};
pub use reconciler::{CpuIntervalReport, CpuReconciler};
pub use shape::{LambdaReport, RuntimeReceipt, hands_baseline, hands_facts, lambda_facts};
pub use sink::{
    BoundedFactSink, DrainError, DrainPolicy, DrainReport, FactSink, OverflowLedger, SinkError,
    SinkPressure,
};
pub use storage::{FencedCursor, StorageCursorStore, StorageEvent, StorageProbe};

/// Everything every fact a probe produces carries.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProbeContext {
    /// The organization that owes the money.
    pub organization: OrganizationId,
    /// The workspace the fact partitions under.
    pub workspace: WorkspaceId,
    /// The region the measurement is taken in.
    pub region: RegionId,
    /// The deployable doing the measuring.
    pub service: ServiceId,
    /// The exact physical thing measured, pinned to an immutable generation.
    pub resource: ResourceGeneration,
    /// The rate book pinned at admission. Never the string `current`.
    pub pricing_version: PricingVersion,
    /// The escrow reservation this work draws down, when there is one.
    pub reservation: Option<ReservationId>,
}

/// Why a probe refused.
///
/// Every variant is a refusal to invent a number. None of them has a fallback
/// path that produces a plausible quantity instead.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ProbeError {
    /// A physical reading was unavailable. Never reported as zero: a zero
    /// physical reading would make the `charged <= physical` cap vacuous.
    #[error("{source_name} at `{path}` is unreadable: {reason}")]
    PhysicalSource {
        /// Which seam failed.
        source_name: &'static str,
        /// The path it was bound to.
        path: String,
        /// What went wrong.
        reason: String,
    },
    /// A composition value was invalid at startup.
    #[error("`{what}` is misconfigured: {reason}")]
    Configuration {
        /// Which value.
        what: &'static str,
        /// Why it was refused.
        reason: String,
    },
    /// A lock was poisoned by a panic elsewhere.
    #[error("`{what}` is poisoned; a panic left the probe state unusable")]
    Poisoned {
        /// Which state.
        what: &'static str,
    },
    /// The memory envelope could not grant the request.
    #[error("memory budget exhausted: {requested} requested, {live} live, {grantable} grantable")]
    BudgetExhausted {
        /// Bytes requested.
        requested: u64,
        /// Bytes already granted.
        live: u64,
        /// The grantable ceiling.
        grantable: u64,
    },
    /// The budget was torn down while a reservation was still held.
    #[error("memory budget was dropped while a reservation was still held")]
    BudgetGone,
    /// A byte counter overflowed.
    #[error("byte counter for `{boundary}` overflowed")]
    CounterOverflow {
        /// Which boundary.
        boundary: &'static str,
    },
    /// A Hands generation exposed no per-generation transmit receipt.
    ///
    /// No transfer fact exists for that generation. Estimating would bill an
    /// unmeasured quantity, so this is a refusal rather than a fallback.
    #[error("generation `{generation}` has no provider transmit receipt; no transfer fact exists")]
    NoTransmitEvidence {
        /// The generation with no receipt.
        generation: String,
    },
    /// A lifecycle receipt did not account for its own lifetime.
    #[error("generation `{generation}` lived {lifetime_ms} ms but accounts for {accounted_ms} ms")]
    UnexhaustedLifetime {
        /// The generation.
        generation: String,
        /// How long it actually lived.
        lifetime_ms: u64,
        /// How much running and suspended time explain.
        accounted_ms: u64,
    },
    /// A lifecycle receipt ended before it started.
    #[error("generation `{generation}` ended before it started")]
    InvertedLifetime {
        /// The generation.
        generation: String,
    },
    /// The durable cursor store failed.
    #[error("storage cursor store failed: {reason}")]
    CursorStore {
        /// What the store reported.
        reason: String,
    },
    /// A concurrent transition won the cursor fence.
    ///
    /// The caller must re-read and re-apply; retrying the same write blindly
    /// would either double-charge or drop a minute.
    #[error("storage cursor fence conflict; expected revision {expected:?}")]
    CursorConflict {
        /// The revision this attempt expected.
        expected: Option<u64>,
    },
    /// The sink refused the draft.
    #[error(transparent)]
    Sink(#[from] SinkError),
    /// A measurement failed its own construction rules.
    #[error(transparent)]
    Measurement(#[from] MeasurementError),
    /// An interval could not be closed.
    #[error(transparent)]
    Interval(#[from] IntervalError),
    /// A derived identifier was not valid.
    #[error(transparent)]
    Identity(#[from] IdentityError),
    /// A derived identifier body was not valid.
    #[error(transparent)]
    Id(#[from] IdError),
    /// A derived instant was not representable.
    #[error(transparent)]
    Timestamp(#[from] TimestampError),
}

/// Converts a wall-clock reading into the canonical fixed-width instant.
///
/// # Errors
///
/// Returns [`ProbeError::Timestamp`] when the reading is outside the
/// representable range.
pub fn to_timestamp(at: OffsetDateTime) -> Result<Timestamp, ProbeError> {
    let millis = i64::try_from(at.unix_timestamp_nanos() / 1_000_000)
        .map_err(|_| ProbeError::Timestamp(TimestampError::OutOfRange { millis: i64::MAX }))?;
    Ok(Timestamp::from_unix_millis(millis)?)
}
