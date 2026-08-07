//! Activation-scoped CPU attribution.
//!
//! Two shapes, and only two:
//!
//! 1. A **bounded compute-lane job** measured end to end with [`CpuJob`].
//! 2. An **async activation future** wrapped in [`ActivationScope`], which reads
//!    the thread CPU clock around each inner poll and adds the capped delta.
//!
//! What both refuse to measure is the point. Time pending on provider HTTP, a
//! tool call or a durable wait is not inside a poll — the future is parked and
//! the thread is running something else — so it contributes **no** attribution
//! and therefore no compute fact. Billing a customer for an idle await would be
//! billing them for the reactor's patience.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::task::{Context, Poll};

use aex_usage_domain::fact::Attribution;
use pin_project_lite::pin_project;

use super::ProbeContext;
use super::clock::{CpuInstant, CpuMicros, ThreadCpuClock};

/// The longest single poll that may be attributed in full.
///
/// A poll longer than this is a defect — a blocking call on the reactor — so the
/// excess is discarded rather than billed and the violation is counted. Matches
/// Brain `BC-26`.
pub const MAX_ATTRIBUTED_POLL_US: u64 = 50_000;

/// Which activation a measurement belongs to.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ActivationKey {
    /// The session.
    pub session: aex_usage_domain::wire_pending::SessionId,
    /// The agent.
    pub agent: aex_usage_domain::wire_pending::AgentId,
    /// The activation.
    pub activation: aex_usage_domain::wire_pending::ActivationId,
    /// The Brain activation fence this measurement was taken under.
    pub fence: u64,
}

/// The accumulator one activation attributes into.
///
/// Shared by every scope and job for that activation, drained only by
/// [`super::reconciler::CpuReconciler::close_interval`].
#[derive(Debug)]
pub struct ActivationMeter {
    key: ActivationKey,
    context: ProbeContext,
    attribution: Attribution,
    attributed_us: AtomicU64,
    poll_count: AtomicU64,
    long_poll_violations: AtomicU64,
    dropped_jobs: AtomicU64,
}

impl ActivationMeter {
    /// Opens an accumulator for one activation.
    #[must_use]
    pub fn new(key: ActivationKey, context: ProbeContext, attribution: Attribution) -> Arc<Self> {
        Arc::new(Self {
            key,
            context,
            attribution,
            attributed_us: AtomicU64::new(0),
            poll_count: AtomicU64::new(0),
            long_poll_violations: AtomicU64::new(0),
            dropped_jobs: AtomicU64::new(0),
        })
    }

    /// Which activation this accumulates for.
    #[must_use]
    pub const fn key(&self) -> &ActivationKey {
        &self.key
    }

    /// The probe context every fact from this meter carries.
    #[must_use]
    pub const fn context(&self) -> &ProbeContext {
        &self.context
    }

    /// The customer work every fact from this meter is attributed to.
    #[must_use]
    pub const fn attribution(&self) -> &Attribution {
        &self.attribution
    }

    /// Microseconds attributed since the last drain.
    #[must_use]
    pub fn attributed_us(&self) -> u64 {
        self.attributed_us.load(Ordering::Acquire)
    }

    /// How many polls have been measured.
    #[must_use]
    pub fn poll_count(&self) -> u64 {
        self.poll_count.load(Ordering::Relaxed)
    }

    /// How many polls exceeded [`MAX_ATTRIBUTED_POLL_US`].
    #[must_use]
    pub fn long_poll_violations(&self) -> u64 {
        self.long_poll_violations.load(Ordering::Relaxed)
    }

    /// How many [`CpuJob`] values were dropped instead of finished.
    #[must_use]
    pub fn dropped_jobs(&self) -> u64 {
        self.dropped_jobs.load(Ordering::Relaxed)
    }

    /// Adds one measured poll, capping it at [`MAX_ATTRIBUTED_POLL_US`].
    fn attribute_poll(&self, elapsed_us: u64) {
        self.poll_count.fetch_add(1, Ordering::Relaxed);
        let charged = if elapsed_us > MAX_ATTRIBUTED_POLL_US {
            self.long_poll_violations.fetch_add(1, Ordering::Relaxed);
            MAX_ATTRIBUTED_POLL_US
        } else {
            elapsed_us
        };
        self.attributed_us.fetch_add(charged, Ordering::AcqRel);
    }

    /// Adds one measured bounded job. A job is not a poll, so it is not capped:
    /// the compute lane's own admission bounds it.
    fn attribute_job(&self, elapsed_us: u64) {
        self.attributed_us.fetch_add(elapsed_us, Ordering::AcqRel);
    }

    /// Takes and zeroes the accumulator.
    ///
    /// Only [`super::reconciler::CpuReconciler`] calls this, at an interval
    /// close. Draining anywhere else would attribute the same microseconds
    /// twice.
    pub(super) fn drain_us(&self) -> u64 {
        self.attributed_us.swap(0, Ordering::AcqRel)
    }
}

pin_project! {
    /// An activation future that attributes its own poll cost.
    ///
    /// Only the time spent *inside* `inner.poll` is measured. Time the future
    /// spends pending — a provider round trip, a tool call, a durable wait —
    /// happens between polls and is never attributed.
    pub struct ActivationScope<F> {
        #[pin]
        inner: F,
        meter: Arc<ActivationMeter>,
        clock: Arc<dyn ThreadCpuClock>,
    }
}

impl<F: Future> Future for ActivationScope<F> {
    type Output = F::Output;

    fn poll(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.project();
        let started = this.clock.thread_cpu_now();
        let outcome = this.inner.poll(context);
        let elapsed = this.clock.thread_cpu_now().saturating_since(started);
        this.meter.attribute_poll(elapsed);
        outcome
    }
}

/// Wraps any future so its poll cost is attributed to one activation.
pub trait ActivationScoped: Future + Sized {
    /// Meters this future against `meter`.
    fn metered(
        self,
        meter: Arc<ActivationMeter>,
        clock: Arc<dyn ThreadCpuClock>,
    ) -> ActivationScope<Self> {
        ActivationScope {
            inner: self,
            meter,
            clock,
        }
    }
}

impl<F: Future> ActivationScoped for F {}

/// One bounded compute-lane job, measured end to end.
///
/// Dropping a job instead of finishing it still attributes the elapsed CPU — the
/// work happened either way — but counts the drop, because the intended shape is
/// an explicit `finish`.
#[must_use = "a CpuJob must be finished; dropping it still attributes but is counted as a defect"]
#[derive(Debug)]
pub struct CpuJob {
    meter: Arc<ActivationMeter>,
    clock: Arc<dyn ThreadCpuClock>,
    started: CpuInstant,
    finished: bool,
}

impl CpuJob {
    /// Starts measuring one bounded job.
    pub fn begin(meter: &Arc<ActivationMeter>, clock: &Arc<dyn ThreadCpuClock>) -> Self {
        Self {
            meter: Arc::clone(meter),
            clock: Arc::clone(clock),
            started: clock.thread_cpu_now(),
            finished: false,
        }
    }

    /// Closes the job and returns what it attributed.
    #[must_use]
    pub fn finish(mut self) -> CpuMicros {
        let elapsed = self.clock.thread_cpu_now().saturating_since(self.started);
        self.meter.attribute_job(elapsed);
        self.finished = true;
        CpuMicros::new(elapsed)
    }
}

impl Drop for CpuJob {
    fn drop(&mut self) {
        if self.finished {
            return;
        }
        let elapsed = self.clock.thread_cpu_now().saturating_since(self.started);
        self.meter.attribute_job(elapsed);
        self.meter.dropped_jobs.fetch_add(1, Ordering::Relaxed);
    }
}

#[cfg(test)]
mod tests {
    use super::{ActivationMeter, ActivationScoped, CpuJob, MAX_ATTRIBUTED_POLL_US};
    use crate::probe::clock::{CpuInstant, ThreadCpuClock};
    use crate::probe::testing::{activation_key, probe_context};
    use aex_usage_domain::fact::Attribution;
    use std::future::Future;
    use std::pin::Pin;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::task::{Context, Poll, Waker};

    /// A clock that advances by a scripted amount on each read, so every
    /// assertion below is exact rather than statistical.
    #[derive(Debug)]
    struct ScriptedClock {
        readings: std::sync::Mutex<std::collections::VecDeque<u64>>,
        last: AtomicU64,
    }

    impl ScriptedClock {
        fn new(readings: impl IntoIterator<Item = u64>) -> Arc<Self> {
            Arc::new(Self {
                readings: std::sync::Mutex::new(readings.into_iter().collect()),
                last: AtomicU64::new(0),
            })
        }
    }

    impl ThreadCpuClock for ScriptedClock {
        fn thread_cpu_now(&self) -> CpuInstant {
            let next = self
                .readings
                .lock()
                .expect("clock lock")
                .pop_front()
                .unwrap_or_else(|| self.last.load(Ordering::Relaxed));
            self.last.store(next, Ordering::Relaxed);
            CpuInstant::from_micros(next)
        }
    }

    fn meter() -> Arc<ActivationMeter> {
        ActivationMeter::new(activation_key(), probe_context(), Attribution::default())
    }

    /// A future that is pending for `pending_polls` polls before completing.
    struct PendingFuture {
        remaining: u32,
    }

    impl Future for PendingFuture {
        type Output = ();

        fn poll(mut self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<()> {
            if self.remaining == 0 {
                return Poll::Ready(());
            }
            self.remaining -= 1;
            context.waker().wake_by_ref();
            Poll::Pending
        }
    }

    fn drive<F: Future>(future: F) -> F::Output {
        let waker = Waker::noop();
        let mut context = Context::from_waker(waker);
        let mut future = Box::pin(future);
        loop {
            if let Poll::Ready(output) = future.as_mut().poll(&mut context) {
                return output;
            }
        }
    }

    #[test]
    fn only_time_inside_a_poll_is_attributed() {
        // Three polls. Each poll reads the clock twice; between polls the clock
        // jumps by a large amount, standing in for a provider round trip, a tool
        // call and a durable wait. None of those jumps may be attributed.
        let clock = ScriptedClock::new([
            0, 10, // poll 1: 10 us of real work
            5_000_000, 5_000_020, // 5 s awaited elsewhere, then 20 us of work
            9_000_000, 9_000_030, // 4 s awaited elsewhere, then 30 us of work
        ]);
        let meter = meter();

        drive(PendingFuture { remaining: 2 }.metered(Arc::clone(&meter), clock));

        assert_eq!(
            meter.attributed_us(),
            60,
            "only the 10 + 20 + 30 us actually spent polling is attributable"
        );
        assert_eq!(meter.poll_count(), 3);
        assert_eq!(meter.long_poll_violations(), 0);
    }

    #[test]
    fn a_future_that_only_awaits_attributes_nothing() {
        // Both clock reads in each poll return the same value: the future did no
        // work at all, it just registered a waker and returned pending.
        let clock = ScriptedClock::new([0, 0, 1_000_000, 1_000_000, 2_000_000, 2_000_000]);
        let meter = meter();

        drive(PendingFuture { remaining: 2 }.metered(Arc::clone(&meter), clock));

        assert_eq!(
            meter.attributed_us(),
            0,
            "awaiting a sleep, an HTTP call or a durable wait creates no compute fact"
        );
        assert_eq!(meter.poll_count(), 3);
    }

    #[test]
    fn a_poll_over_the_cap_is_truncated_and_counted_as_a_violation() {
        let over = MAX_ATTRIBUTED_POLL_US + 25_000;
        let clock = ScriptedClock::new([0, over]);
        let meter = meter();

        drive(PendingFuture { remaining: 0 }.metered(Arc::clone(&meter), clock));

        assert_eq!(
            meter.attributed_us(),
            MAX_ATTRIBUTED_POLL_US,
            "a blocking call on the reactor is a defect, not a billable quantity"
        );
        assert_eq!(meter.long_poll_violations(), 1);
    }

    #[test]
    fn a_finished_job_attributes_exactly_its_elapsed_cpu() {
        let clock = ScriptedClock::new([1_000, 4_500]);
        let meter = meter();
        let job = CpuJob::begin(&meter, &(clock as Arc<dyn ThreadCpuClock>));

        assert_eq!(job.finish().get(), 3_500);
        assert_eq!(meter.attributed_us(), 3_500);
        assert_eq!(meter.dropped_jobs(), 0);
    }

    #[test]
    fn a_dropped_job_still_attributes_but_is_counted() {
        let clock = ScriptedClock::new([100, 900]);
        let meter = meter();
        {
            let _job = CpuJob::begin(&meter, &(clock as Arc<dyn ThreadCpuClock>));
        }
        assert_eq!(
            meter.attributed_us(),
            800,
            "the work happened, so it is still attributed"
        );
        assert_eq!(meter.dropped_jobs(), 1, "but the drop is a counted defect");
    }

    #[test]
    fn draining_zeroes_the_accumulator_so_nothing_is_attributed_twice() {
        let clock = ScriptedClock::new([0, 700]);
        let meter = meter();
        let _ = CpuJob::begin(&meter, &(clock as Arc<dyn ThreadCpuClock>)).finish();
        assert_eq!(meter.attributed_us(), 700);

        assert_eq!(meter.drain_us(), 700);
        assert_eq!(meter.attributed_us(), 0);
        assert_eq!(meter.drain_us(), 0);
    }
}
