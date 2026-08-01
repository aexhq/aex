//! The three runtimes, and the compute lane between them.
//!
//! One process, three schedulers, for one measured reason each.
//!
//! 1. The **main runtime** is a multi-thread Tokio runtime carrying admission, activations,
//!    provider streams and store I/O.
//! 2. The **control runtime** is a `new_current_thread` runtime on its own OS thread serving
//!    `/internal/healthz`, `/internal/readyz` and the metric publisher. It exists because
//!    the activation-pool spike measured synchronous CPU on the reactor starving health
//!    checks, and ECS replaced tasks that were working perfectly.
//! 3. The **compute lane** is `spawn_blocking` gated by a weighted semaphore. Tokenization,
//!    canonicalization, compression and hashing above the inline budget run there, leaving
//!    the reactor bounded parsing, scheduling and async I/O.

use aex_brain_application::kernel::{PermitKind, PermitSet, Reservation};
use std::sync::Arc;

/// The CPU cost above which work belongs on the compute lane rather than inline.
///
/// Measured: the v4 to v5 fix. Below this a job costs less than the scheduling it would
/// take to move it; above it, the reactor's latency is what pays.
pub const INLINE_CPU_BUDGET: core::time::Duration = core::time::Duration::from_micros(200);

/// The blocking-thread ceiling of the main runtime.
pub const MAX_BLOCKING_THREADS: usize = 8;

/// How the process's runtimes are shaped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RuntimeShape {
    /// Worker threads on the main runtime.
    pub worker_threads: usize,
    /// Blocking threads the main runtime may create.
    pub max_blocking_threads: usize,
    /// Concurrent compute-lane jobs.
    pub compute_permits: u64,
}

impl RuntimeShape {
    /// The shape for a task with `parallelism` available cores.
    ///
    /// The compute lane is sized to the worker count rather than to the blocking pool: it
    /// bounds *CPU*, and there are only that many cores to saturate.
    #[must_use]
    pub const fn for_parallelism(parallelism: usize) -> Self {
        let workers = if parallelism == 0 { 1 } else { parallelism };
        Self {
            worker_threads: workers,
            max_blocking_threads: MAX_BLOCKING_THREADS,
            compute_permits: workers as u64,
        }
    }

    /// The shape this host would take.
    #[must_use]
    pub fn detected() -> Self {
        Self::for_parallelism(
            std::thread::available_parallelism().map_or(1, std::num::NonZeroUsize::get),
        )
    }
}

/// The gate every compute-lane job passes through.
///
/// A permit rather than an unbounded `spawn_blocking`: the blocking pool would happily
/// start eight tokenizations on a one-core task and starve the reactor doing it.
#[derive(Debug)]
pub struct ComputeLane {
    permits: Arc<PermitSet>,
    queued: std::sync::atomic::AtomicUsize,
}

/// Permission to run one bounded compute job.
#[derive(Debug)]
#[must_use = "dropping the slot releases the lane immediately"]
pub struct ComputeSlot {
    _permit: Reservation,
}

/// The lane is full.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("the compute lane is saturated")]
pub struct LaneFull;

impl ComputeLane {
    /// Builds a lane over the process's permit set.
    #[must_use]
    pub const fn new(permits: Arc<PermitSet>) -> Self {
        Self {
            permits,
            queued: std::sync::atomic::AtomicUsize::new(0),
        }
    }

    /// Takes one slot.
    ///
    /// # Errors
    ///
    /// [`LaneFull`] when every permit is held. The caller queues with an age bound; it never
    /// runs the job inline, because that is exactly the reactor stall the lane exists to
    /// prevent.
    pub fn try_enter(&self) -> Result<ComputeSlot, LaneFull> {
        self.permits
            .acquire(PermitKind::ComputeLane, 1)
            .map(|permit| ComputeSlot { _permit: permit })
            .map_err(|_| LaneFull)
    }

    /// How many jobs are waiting for a slot.
    #[must_use]
    pub fn queue_depth(&self) -> usize {
        self.queued.load(std::sync::atomic::Ordering::Relaxed)
    }

    /// Records that a job is waiting.
    pub fn enqueued(&self) {
        self.queued
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    }

    /// Records that a waiting job started or gave up.
    pub fn dequeued(&self) {
        let _ = self.queued.fetch_update(
            std::sync::atomic::Ordering::Relaxed,
            std::sync::atomic::Ordering::Relaxed,
            |current| Some(current.saturating_sub(1)),
        );
    }

    /// The fraction of the lane in use, in hundredths.
    #[must_use]
    pub fn utilization_percent(&self) -> u32 {
        let limit = self.permits.limit(PermitKind::ComputeLane);
        if limit == 0 {
            return 100;
        }
        let held = self.permits.held(PermitKind::ComputeLane);
        u32::try_from(held.saturating_mul(100) / limit).unwrap_or(100)
    }

    /// Whether `cost` belongs on the lane rather than inline.
    #[must_use]
    pub const fn belongs_on_lane(cost: core::time::Duration) -> bool {
        cost.as_micros() > INLINE_CPU_BUDGET.as_micros()
    }
}

#[cfg(test)]
mod tests {
    use super::{ComputeLane, INLINE_CPU_BUDGET, MAX_BLOCKING_THREADS, RuntimeShape};
    use aex_brain_application::kernel::{PermitKind, PermitSet};
    use std::collections::BTreeMap;
    use std::sync::Arc;

    fn lane(permits: u64) -> ComputeLane {
        ComputeLane::new(Arc::new(PermitSet::new(BTreeMap::from([(
            PermitKind::ComputeLane,
            permits,
        )]))))
    }

    /// The lane bounds CPU, so it is sized to the worker count rather than to the blocking
    /// pool: there are only that many cores to saturate.
    #[test]
    fn the_compute_lane_is_sized_to_the_worker_count() {
        let shape = RuntimeShape::for_parallelism(4);
        assert_eq!(shape.worker_threads, 4);
        assert_eq!(shape.compute_permits, 4);
        assert_eq!(shape.max_blocking_threads, MAX_BLOCKING_THREADS);
    }

    /// A reported parallelism of zero must not produce a runtime with no workers.
    #[test]
    fn a_degenerate_parallelism_still_produces_a_usable_runtime() {
        let shape = RuntimeShape::for_parallelism(0);
        assert_eq!(shape.worker_threads, 1);
        assert_eq!(shape.compute_permits, 1);
    }

    #[test]
    fn the_detected_shape_is_usable() {
        let shape = RuntimeShape::detected();
        assert!(shape.worker_threads >= 1);
        assert_eq!(shape.compute_permits, shape.worker_threads as u64);
    }

    /// A saturated lane refuses rather than admitting: running the job inline instead is
    /// precisely the reactor stall the lane exists to prevent.
    #[test]
    fn a_saturated_lane_refuses_rather_than_running_the_job_inline() {
        let lane = lane(1);
        let held = lane.try_enter().expect("the first slot");
        assert!(lane.try_enter().is_err());
        drop(held);
        assert!(lane.try_enter().is_ok(), "a released slot is reusable");
    }

    #[test]
    fn utilization_reports_the_fraction_in_use() {
        let lane = lane(4);
        assert_eq!(lane.utilization_percent(), 0);
        let _one = lane.try_enter().expect("slot");
        let _two = lane.try_enter().expect("slot");
        assert_eq!(lane.utilization_percent(), 50);
    }

    /// An unconfigured lane is unavailable, never unbounded: reporting zero utilization for
    /// a lane nobody sized would hide the misconfiguration until the reactor stalled.
    #[test]
    fn an_unconfigured_lane_reads_as_full_rather_than_as_idle() {
        let lane = lane(0);
        assert_eq!(lane.utilization_percent(), 100);
        assert!(lane.try_enter().is_err());
    }

    #[test]
    fn the_queue_depth_never_goes_negative() {
        let lane = lane(1);
        lane.dequeued();
        assert_eq!(lane.queue_depth(), 0);
        lane.enqueued();
        lane.enqueued();
        assert_eq!(lane.queue_depth(), 2);
        lane.dequeued();
        assert_eq!(lane.queue_depth(), 1);
    }

    #[test]
    fn the_inline_budget_decides_where_a_job_runs() {
        assert!(!ComputeLane::belongs_on_lane(
            core::time::Duration::from_micros(50)
        ));
        assert!(!ComputeLane::belongs_on_lane(INLINE_CPU_BUDGET));
        assert!(ComputeLane::belongs_on_lane(
            core::time::Duration::from_micros(201)
        ));
    }
}
