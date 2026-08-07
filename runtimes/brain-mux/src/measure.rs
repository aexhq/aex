//! A11-MUX resource measurement.
//!
//! Everything here composes `aex_usage_app::probe`. Plan 07 §9.8 sketched its own
//! `MemoryReservation`, `ReservationClass` and `ActivationMeter`; those are the usage
//! stream's types and are imported, never redeclared, because two definitions of a billing
//! unit is how a bill and a receipt come to disagree.
//!
//! Four properties this module exists to hold.
//!
//! - **Time pending on provider HTTP, a tool or a durable wait creates no compute fact.**
//!   The meter measures only the inside of a `poll`, and a pending future is by definition
//!   not inside one. Nothing is needed to get that behaviour, which is exactly why it is
//!   asserted: it would be easy to "improve" the meter into wall-clock and never notice.
//! - **Per-poll attribution is capped.** A poll above
//!   [`MAX_ATTRIBUTED_POLL_US`](aex_usage_app::probe::MAX_ATTRIBUTED_POLL_US) is a
//!   defect — long work belongs on the compute lane — so it increments a violation counter
//!   rather than becoming a larger charge.
//! - **Charged CPU never exceeds physical CPU.** Every ten seconds the reconciler reads the
//!   cgroup delta and scales every share down if the sum overshot; the remainder is an
//!   unbilled platform bucket.
//! - **A memory reservation is a `#[must_use]` RAII token.** Binding one to `_` releases it
//!   immediately, which the type system says out loud rather than leaving to review.

use aex_usage_app::probe::{
    ActivationKey, ActivationMeter, BoundedFactSink, CpuIntervalReport, CpuReconciler,
    MemoryBudget, MemoryReservation, OverflowLedger, PhysicalCpuSource, ProbeContext, ProbeError,
    RustixThreadCpuClock, SystemWallClock, ThreadCpuClock, WallClock,
};
use aex_usage_domain::fact::Attribution;
use aex_usage_domain::measurement::ReservationClass;
use std::sync::Arc;

/// How often the CPU reconciler closes an interval.
pub const RECONCILE_INTERVAL: core::time::Duration = core::time::Duration::from_secs(10);

/// The queue depth the fact sink holds before it sheds into the overflow ledger.
pub const SINK_CAPACITY: usize = 4_096;

/// Everything the mux measures with.
///
/// One value rather than five, because the five have to agree about the clock: a meter on
/// one clock and a reconciler on another would produce a ratio nobody can explain.
#[derive(Debug, Clone)]
pub struct Measurement {
    sink: Arc<BoundedFactSink>,
    overflow: Arc<OverflowLedger>,
    memory: Arc<MemoryBudget>,
    reconciler: Arc<CpuReconciler>,
    thread_clock: Arc<dyn ThreadCpuClock>,
    wall_clock: Arc<dyn WallClock>,
}

impl Measurement {
    /// Builds the measurement stack over a physical CPU source.
    ///
    /// # Errors
    ///
    /// [`ProbeError`] when the sink capacity is unusable or the memory envelope leaves no
    /// grantable bytes after its headroom. Both fail startup: a task that cannot measure
    /// what it consumes must not admit work it will have to charge for.
    pub fn new(
        envelope_bytes: u64,
        headroom_bytes: u64,
        physical: Arc<dyn PhysicalCpuSource>,
    ) -> Result<Self, ProbeError> {
        let overflow = Arc::new(OverflowLedger::default());
        let sink = Arc::new(BoundedFactSink::new(SINK_CAPACITY, Arc::clone(&overflow))?);
        let wall_clock: Arc<dyn WallClock> = Arc::new(SystemWallClock);
        let memory = MemoryBudget::new(
            envelope_bytes,
            headroom_bytes,
            Arc::clone(&sink),
            Arc::clone(&wall_clock),
        )?;
        let reconciler = CpuReconciler::new(physical, Arc::clone(&wall_clock), RECONCILE_INTERVAL);
        Ok(Self {
            sink,
            overflow,
            memory,
            reconciler,
            thread_clock: Arc::new(RustixThreadCpuClock),
            wall_clock,
        })
    }

    /// Builds the stack with an explicit clock, for a deterministic test.
    #[must_use]
    pub fn with_clocks(
        mut self,
        thread_clock: Arc<dyn ThreadCpuClock>,
        wall_clock: Arc<dyn WallClock>,
    ) -> Self {
        self.thread_clock = thread_clock;
        self.wall_clock = wall_clock;
        self
    }

    /// Opens a meter for one activation and registers it for reconciliation.
    ///
    /// The registry holds weak references, so a finished activation drops out on its own and
    /// there is no deregister to forget.
    #[must_use]
    pub fn activation(
        &self,
        key: ActivationKey,
        context: ProbeContext,
        attribution: Attribution,
    ) -> Arc<ActivationMeter> {
        let meter = ActivationMeter::new(key, context, attribution);
        self.reconciler.register(&meter);
        meter
    }

    /// Reserves `bytes` of `class`.
    ///
    /// # Errors
    ///
    /// [`ProbeError::BudgetExhausted`] when the envelope cannot grant it. The caller defers
    /// or sheds; it never proceeds without the reservation, because an unreserved buffer is
    /// memory nobody accounted for and nobody can release.
    pub fn reserve(
        &self,
        context: &ProbeContext,
        attribution: &Attribution,
        class: ReservationClass,
        bytes: u64,
    ) -> Result<MemoryReservation, ProbeError> {
        self.memory.try_reserve(context, attribution, class, bytes)
    }

    /// Closes one reconciliation interval.
    ///
    /// # Errors
    ///
    /// [`ProbeError::PhysicalSource`] when the cgroup reading is unavailable. Never reported
    /// as zero: a zero physical reading would make the `charged <= physical` cap vacuous and
    /// every attributed microsecond would pass it.
    pub fn close_interval(&self) -> Result<CpuIntervalReport, ProbeError> {
        self.reconciler.close_interval(self.sink.as_ref())
    }

    /// The thread CPU clock every meter and job shares.
    #[must_use]
    pub fn thread_clock(&self) -> &Arc<dyn ThreadCpuClock> {
        &self.thread_clock
    }

    /// The wall clock every interval shares.
    #[must_use]
    pub fn wall_clock(&self) -> &Arc<dyn WallClock> {
        &self.wall_clock
    }

    /// The bounded sink every fact is offered to.
    #[must_use]
    pub fn sink(&self) -> &Arc<BoundedFactSink> {
        &self.sink
    }

    /// The ledger that records what the sink shed.
    #[must_use]
    pub fn overflow(&self) -> &Arc<OverflowLedger> {
        &self.overflow
    }

    /// Bytes currently reserved.
    #[must_use]
    pub fn live_bytes(&self) -> u64 {
        self.memory.live_bytes()
    }

    /// Bytes still grantable.
    #[must_use]
    pub fn grantable_bytes(&self) -> u64 {
        self.memory.grantable_bytes()
    }

    /// How many times an interval had to scale attributed CPU down to physical.
    #[must_use]
    pub fn overattribution_total(&self) -> u64 {
        self.reconciler.overattribution_total()
    }

    /// How many activation meters are still live.
    #[must_use]
    pub fn live_meters(&self) -> usize {
        self.reconciler.live_meters()
    }
}

/// The unit identity `compute.millicpu_ms.v1` carries.
///
/// One CPU-microsecond at 1 000 millicpu is one millicpu-millisecond, so the quantity is
/// CPU-microseconds exactly and no rounding convention enters the billing path.
#[must_use]
pub const fn millicpu_ms_from_cpu_us(cpu_us: u64) -> u64 {
    cpu_us
}

/// The `memory.byte_ms.v1` quantity for a closed reservation interval.
#[must_use]
pub const fn byte_ms(bytes: u64, duration_ms: u64) -> u64 {
    bytes.saturating_mul(duration_ms)
}

#[cfg(test)]
#[path = "measure_tests.rs"]
mod tests;
