//! Evidence for the pump: it drains on its own while the host does nothing,
//! stopping joins the thread and flushes once more, stopping twice is safe, an
//! exporter failure is counted without stopping the thread, and a dropped pump
//! leaves nothing running.
//!
//! Timing assertions wait for a condition with a generous ceiling rather than
//! sleeping for a fixed span, so a slow machine costs time and only a genuinely
//! stalled pump fails.

use std::sync::Arc;
use std::time::{Duration, Instant};

use aex_platform_telemetry::{
    FailingExporter, FlushOutcome, Handle, InMemoryExporter, Record, Settings, TelemetryPump,
    TelemetryStats,
};
use aex_telemetry_schema::generated::{AEX_PLANE, EVENT_AEX_PROCESS_STARTED};
use parking_lot::Mutex;

/// The longest any of these tests waits for the pump to do its work.
const PATIENCE: Duration = Duration::from_secs(10);

/// Short enough that a test observes several ticks, long enough that a loaded
/// machine is not the thing under test.
const TICK: Duration = Duration::from_millis(10);

/// Longer than any test runs, so only an explicit stop can cause a flush.
const NEVER: Duration = Duration::from_hours(1);

fn started() -> Record {
    Record::event(EVENT_AEX_PROCESS_STARTED).with(AEX_PLANE, "dev")
}

fn wait_until(mut satisfied: impl FnMut() -> bool) {
    let deadline = Instant::now() + PATIENCE;
    while Instant::now() < deadline {
        if satisfied() {
            return;
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    panic!("the pump did not reach the expected state within {PATIENCE:?}");
}

#[test]
fn the_pump_drains_the_queue_without_the_host_flushing() {
    let exporter = Arc::new(InMemoryExporter::new());
    let handle = Handle::install(&Settings::long_lived(), Some(exporter.clone()));
    let mut pump = TelemetryPump::start(handle.clone(), TICK, Duration::from_secs(1))
        .expect("the pump thread starts");

    for _ in 0..8 {
        handle.emit(started());
    }
    wait_until(|| exporter.delivered().len() == 8);
    assert_eq!(handle.pending(), 0);

    for _ in 0..8 {
        handle.emit(started());
    }
    wait_until(|| exporter.delivered().len() == 16);
    assert_eq!(
        pump.stop_and_join(),
        FlushOutcome::Drained { exported: 0 },
        "the drain flush finds an already-drained queue"
    );
}

#[test]
fn stopping_joins_the_thread_and_performs_a_final_flush() {
    let exporter = Arc::new(InMemoryExporter::new());
    let handle = Handle::install(&Settings::long_lived(), Some(exporter.clone()));
    let mut pump = TelemetryPump::start(handle.clone(), NEVER, Duration::from_secs(1))
        .expect("the pump starts");

    for _ in 0..5 {
        handle.emit(started());
    }
    assert_eq!(
        exporter.delivered().len(),
        0,
        "no interval elapses during this test, so only the drain flush can export"
    );

    assert_eq!(pump.stop_and_join(), FlushOutcome::Drained { exported: 5 });
    assert_eq!(exporter.delivered().len(), 5);
    assert_eq!(handle.pending(), 0);
}

#[test]
fn stopping_twice_waits_for_nothing_and_reports_the_same_outcome() {
    let exporter = Arc::new(InMemoryExporter::new());
    let handle = Handle::install(&Settings::long_lived(), Some(exporter.clone()));
    let mut pump = TelemetryPump::start(handle.clone(), NEVER, Duration::from_secs(1))
        .expect("the pump starts");
    handle.emit(started());

    let first = pump.stop_and_join();
    let started_at = Instant::now();
    let second = pump.stop_and_join();
    assert_eq!(first, FlushOutcome::Drained { exported: 1 });
    assert_eq!(second, first);
    assert!(
        started_at.elapsed() < Duration::from_secs(1),
        "a second stop has nothing to wait for"
    );
    assert_eq!(exporter.delivered().len(), 1, "the final flush ran once");
}

#[test]
fn an_exporter_failure_is_counted_and_the_pump_keeps_running() {
    let exporter = Arc::new(FailingExporter::new());
    let handle = Handle::install(&Settings::long_lived(), Some(exporter.clone()));
    let mut pump = TelemetryPump::start(handle.clone(), TICK, Duration::from_secs(1))
        .expect("the pump thread starts");

    handle.emit(started());
    wait_until(|| handle.dropped() == 1);
    handle.emit(started());
    wait_until(|| handle.dropped() == 2);
    assert!(
        exporter.calls() >= 2,
        "the thread survived the first failure"
    );
    let _ = pump.stop_and_join();
}

#[test]
fn dropping_the_pump_stops_the_thread_and_drains() {
    let exporter = Arc::new(InMemoryExporter::new());
    let handle = Handle::install(&Settings::long_lived(), Some(exporter.clone()));
    {
        let _pump = TelemetryPump::start(handle.clone(), NEVER, Duration::from_secs(1))
            .expect("the pump starts");
        handle.emit(started());
    }
    assert_eq!(
        exporter.delivered().len(),
        1,
        "dropping the pump joins its thread after a final flush"
    );
}

#[test]
fn the_shutdown_budget_is_two_flush_budgets() {
    let handle = Handle::install(&Settings::long_lived(), None);
    let mut pump =
        TelemetryPump::start(handle, NEVER, Duration::from_millis(250)).expect("the pump starts");
    assert_eq!(pump.shutdown_budget(), Duration::from_millis(500));
    let _ = pump.stop_and_join();
}

/// A stats destination that keeps every snapshot the pump publishes.
#[derive(Default)]
struct RecordingStats {
    published: Mutex<Vec<TelemetryStats>>,
}

impl RecordingStats {
    fn published(&self) -> Vec<TelemetryStats> {
        self.published.lock().clone()
    }
}

impl aex_platform_telemetry::StatsSink for RecordingStats {
    fn publish(&self, stats: &TelemetryStats) {
        self.published.lock().push(*stats);
    }
}

#[test]
fn published_stats_are_monotonic_and_never_enqueue_a_record() {
    let exporter = Arc::new(InMemoryExporter::new());
    let handle = Handle::install(
        &Settings::long_lived().with_batch_size(1),
        Some(exporter.clone()),
    );
    let stats = Arc::new(RecordingStats::default());
    let mut pump = TelemetryPump::start_reporting(
        handle.clone(),
        TICK,
        Duration::from_secs(1),
        Arc::clone(&stats) as Arc<dyn aex_platform_telemetry::StatsSink>,
        TICK,
    )
    .expect("the pump thread starts");

    for _ in 0..20 {
        handle.emit(started());
    }
    wait_until(|| stats.published().len() >= 3);
    let _ = pump.stop_and_join();

    let published = stats.published();
    assert!(
        published.len() >= 3,
        "the pump reported its own state on its own cadence"
    );
    let mut previous = published[0];
    for snapshot in &published[1..] {
        assert!(snapshot.accepted >= previous.accepted);
        assert!(snapshot.exported >= previous.exported);
        assert!(snapshot.dropped >= previous.dropped);
        assert!(snapshot.redacted_attributes >= previous.redacted_attributes);
        assert!(snapshot.flush_deadline_exceeded >= previous.flush_deadline_exceeded);
        assert_eq!(snapshot.queue_capacity, previous.queue_capacity);
        previous = *snapshot;
    }
    let last = published[published.len() - 1];
    assert_eq!(last.accepted, 20, "no stats snapshot became a record");
    assert_eq!(last.emitted(), 20);
    assert_eq!(last.exported, 20);
    assert_eq!(last.pending, 0);
}
