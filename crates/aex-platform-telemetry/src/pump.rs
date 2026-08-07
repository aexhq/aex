//! The dedicated flush thread a long-lived host owns.
//!
//! A [`crate::facade::Handle`] only moves records when someone calls
//! [`crate::facade::Handle::flush`]. A Lambda has an obvious moment for that —
//! the end of an invocation — but a process that runs for days has none, so its
//! queue fills and every further record is dropped. The pump is that moment,
//! on a schedule.
//!
//! It owns an OS thread rather than a Tokio task on purpose: telemetry must
//! keep draining while the runtime is saturated, which is exactly when a task
//! competing for the same worker threads would stop being scheduled. It is also
//! what keeps this crate free of a runtime dependency, so a host that has no
//! Tokio runtime can still install it.
//!
//! The thread is joined, never detached: [`TelemetryPump::stop_and_join`] and
//! `Drop` both stop it and wait, so a drained host cannot leave a thread
//! writing to a stream the process is about to close.

use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use parking_lot::{Condvar, Mutex};

use crate::facade::{FlushOutcome, Handle, TelemetryStats};

/// Where the pump publishes its own queue state.
///
/// A port separate from [`crate::exporter::Exporter`] because the pump must
/// never enqueue a record about the queue: such a record would be one more
/// entry in the queue it reports on, and a full queue would then drop the
/// evidence that it is full. An implementation writes directly.
pub trait StatsSink: Send + Sync {
    /// Publishes one snapshot directly, without enqueueing anything.
    ///
    /// A failure here is a lost diagnostic and must not be propagated; the pump
    /// has no way to report it that is more reliable than the one that just
    /// failed.
    fn publish(&self, stats: &TelemetryStats);
}

/// Why a pump could not be started.
///
/// Reported rather than absorbed: a host that silently ran without a pump would
/// look healthy while its queue filled and every diagnostic was dropped.
#[derive(Debug, thiserror::Error)]
#[error("the telemetry pump thread could not be spawned: {0}")]
pub struct PumpStartError(#[from] std::io::Error);

/// The name the pump's thread carries in a stack dump.
const PUMP_THREAD_NAME: &str = "aex-telemetry-pump";

#[derive(Debug)]
struct Shared {
    stopping: Mutex<bool>,
    wake: Condvar,
    last_flush: Mutex<FlushOutcome>,
}

/// A thread that drains one [`Handle`] on an interval and once more on drain.
#[derive(Debug)]
pub struct TelemetryPump {
    shared: Arc<Shared>,
    thread: Option<JoinHandle<()>>,
    flush_budget: Duration,
}

impl TelemetryPump {
    /// Starts a pump that flushes `handle` every `interval`, giving each flush
    /// `flush_budget`.
    ///
    /// # Errors
    ///
    /// Returns [`PumpStartError`] when the operating system refuses the thread.
    pub fn start(
        handle: Handle,
        interval: Duration,
        flush_budget: Duration,
    ) -> Result<Self, PumpStartError> {
        Self::spawn(handle, interval, flush_budget, None)
    }

    /// Starts a pump that also publishes its own queue state to `stats` every
    /// `report_every`, and once more when it drains.
    ///
    /// # Errors
    ///
    /// Returns [`PumpStartError`] when the operating system refuses the thread.
    pub fn start_reporting(
        handle: Handle,
        interval: Duration,
        flush_budget: Duration,
        stats: Arc<dyn StatsSink>,
        report_every: Duration,
    ) -> Result<Self, PumpStartError> {
        Self::spawn(
            handle,
            interval,
            flush_budget,
            Some(Reporting {
                sink: stats,
                every: report_every,
            }),
        )
    }

    /// Stops the thread, waits for it, and reports its final flush.
    ///
    /// Idempotent: a second call reports the same outcome and waits for
    /// nothing, so a host may call it explicitly and still let `Drop` run.
    pub fn stop_and_join(&mut self) -> FlushOutcome {
        {
            let mut stopping = self.shared.stopping.lock();
            *stopping = true;
            self.shared.wake.notify_all();
        }
        if let Some(thread) = self.thread.take() {
            // A join error means the worker panicked, which it cannot: the loop
            // calls only counted, non-panicking facade operations. Absorbing it
            // here keeps shutdown from turning a diagnostic into a host failure.
            let _ = thread.join();
        }
        *self.shared.last_flush.lock()
    }

    /// The longest [`Self::stop_and_join`] can wait.
    ///
    /// Two flush budgets: a flush already running when the stop lands runs to
    /// its own deadline, and the drain flush that follows is given the same
    /// budget. Nothing else can extend it — there is no network client behind
    /// either flush, and the facade checks its deadline before every batch.
    #[must_use]
    pub const fn shutdown_budget(&self) -> Duration {
        self.flush_budget.saturating_mul(2)
    }

    fn spawn(
        handle: Handle,
        interval: Duration,
        flush_budget: Duration,
        reporting: Option<Reporting>,
    ) -> Result<Self, PumpStartError> {
        let shared = Arc::new(Shared {
            stopping: Mutex::new(false),
            wake: Condvar::new(),
            last_flush: Mutex::new(FlushOutcome::Drained { exported: 0 }),
        });
        let worker = Arc::clone(&shared);
        let thread = std::thread::Builder::new()
            .name(PUMP_THREAD_NAME.to_owned())
            .spawn(move || {
                pump(&handle, interval, flush_budget, reporting.as_ref(), &worker);
            })?;
        Ok(Self {
            shared,
            thread: Some(thread),
            flush_budget,
        })
    }
}

impl Drop for TelemetryPump {
    fn drop(&mut self) {
        let _ = self.stop_and_join();
    }
}

/// The stats destination and cadence, when a host asked for one.
struct Reporting {
    sink: Arc<dyn StatsSink>,
    every: Duration,
}

/// Flushes on the interval until stopped, then flushes once more.
///
/// The wait is a condition variable rather than a sleep so a stop is observed
/// immediately instead of after up to a full interval; shutdown latency is
/// otherwise dominated by an interval that exists only to keep the process
/// quiet.
fn pump(
    handle: &Handle,
    interval: Duration,
    flush_budget: Duration,
    reporting: Option<&Reporting>,
    shared: &Shared,
) {
    let mut last_report = Instant::now();
    loop {
        {
            let mut stopping = shared.stopping.lock();
            if !*stopping {
                let _ = shared.wake.wait_for(&mut stopping, interval);
            }
            if *stopping {
                break;
            }
        }
        *shared.last_flush.lock() = handle.flush(flush_budget);
        if let Some(reporting) = reporting
            && last_report.elapsed() >= reporting.every
        {
            reporting.sink.publish(&handle.stats());
            last_report = Instant::now();
        }
    }
    *shared.last_flush.lock() = handle.flush(flush_budget);
    if let Some(reporting) = reporting {
        reporting.sink.publish(&handle.stats());
    }
}
