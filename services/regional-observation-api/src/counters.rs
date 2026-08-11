//! Process-local read accounting for long-lived observation sockets.
//!
//! A follow socket is cheap to open and expensive to leave open: every idle
//! cycle costs authority reads whether or not anything was produced. Without a
//! measurement, "reads per socket-minute" is an estimate derived from the code,
//! and an estimate cannot detect a regression.
//!
//! Two properties keep this honest and affordable:
//!
//! - **Counting is not diagnosing.** Every provider call increments one relaxed
//!   atomic. Nothing is emitted per call, so a thousand sockets cost a thousand
//!   increments rather than a thousand records.
//! - **Publication is an aggregate delta.** [`publish`] wakes on a fixed
//!   interval and emits at most one record per counter that moved, so the record
//!   rate is bounded by the counter set rather than by traffic.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;

use aex_platform_telemetry::{Handle, Record};
use aex_telemetry_schema::generated::{
    AEX_DEPLOYABLE, AEX_PLANE, AEX_REGION, AEX_STREAM_COUNTER, METRIC_AEX_STREAM_BYTES,
    METRIC_AEX_STREAM_COUNT,
};

/// How often aggregate deltas are published.
///
/// One minute is the reporting unit the stream capacity work states its results
/// in: provider operations per socket-minute.
pub const PUBLISH_INTERVAL: Duration = Duration::from_mins(1);

/// How often the publisher notices that the process began draining.
///
/// A drain deadline is measured in seconds, so the publisher must not be able to
/// sit inside a whole [`PUBLISH_INTERVAL`] while the task is being stopped.
const DRAIN_CHECK: Duration = Duration::from_millis(250);

/// One measured quantity of the long-lived stream read path.
///
/// The set is closed on purpose: it is the metric dimension, so a variant added
/// here is a registry decision rather than a call-site decision.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum ReadCounter {
    /// Batched frontier reads: one provider request per idle cycle.
    FrontierBatch,
    /// Session-head point reads made outside the frontier batch.
    SessionHead,
    /// Provider queries against the gap ledger.
    GapHistory,
    /// Strong queries against accepted/event-time segment directories.
    SegmentDirectory,
    /// Gap-change hint rows that could not be read, each of which cost the
    /// ledger read the hint exists to avoid.
    GapHintUnreadable,
    /// Provider queries against an observation index segment.
    ObservationPage,
    /// Mutable-authorization renewals for an open socket.
    AuthorizationRenewal,
    /// Idle cycles entered because the bounded fallback delay expired.
    FallbackPoll,
    /// Idle cycles entered because a wake hint arrived first.
    WakePoll,
    /// Observations handed to a socket.
    RecordsReturned,
    /// Authority bytes read on behalf of open sockets.
    BytesReturned,
    /// Producers started.
    SocketOpened,
    /// Producers finished, on every exit path.
    SocketClosed,
    /// Frames abandoned because the transport did not accept them in time.
    WriteStall,
    /// Wake-reader provider calls that failed — discovery, iterator
    /// acquisition or a record read. Wakes are latency hints, so a degraded
    /// reader silently demotes every consumer to fallback polling; this
    /// counter is what makes that demotion observable.
    WakeReaderDegraded,
}

impl ReadCounter {
    /// Every counter, in declaration order.
    pub const ALL: &'static [Self] = &[
        Self::FrontierBatch,
        Self::SessionHead,
        Self::GapHistory,
        Self::SegmentDirectory,
        Self::GapHintUnreadable,
        Self::ObservationPage,
        Self::AuthorizationRenewal,
        Self::FallbackPoll,
        Self::WakePoll,
        Self::RecordsReturned,
        Self::BytesReturned,
        Self::SocketOpened,
        Self::SocketClosed,
        Self::WriteStall,
        Self::WakeReaderDegraded,
    ];

    /// How many counters exist.
    pub const COUNT: usize = Self::ALL.len();

    /// The stable dimension value of this counter.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::FrontierBatch => "frontier_batch",
            Self::SessionHead => "session_head",
            Self::GapHistory => "gap_history",
            Self::SegmentDirectory => "segment_directory",
            Self::GapHintUnreadable => "gap_hint_unreadable",
            Self::ObservationPage => "observation_page",
            Self::AuthorizationRenewal => "authorization_renewal",
            Self::FallbackPoll => "fallback_poll",
            Self::WakePoll => "wake_poll",
            Self::RecordsReturned => "records_returned",
            Self::BytesReturned => "bytes_returned",
            Self::SocketOpened => "socket_opened",
            Self::SocketClosed => "socket_closed",
            Self::WriteStall => "write_stall",
            Self::WakeReaderDegraded => "wake_reader_degraded",
        }
    }

    /// The slot this counter occupies, which is its declaration order.
    const fn slot(self) -> usize {
        self as usize
    }

    /// The record one aggregate delta of this counter publishes.
    ///
    /// Bytes carry their own unit, so they are a separate instrument rather than
    /// another dimension value on a dimensionless count.
    fn record(self, delta: u64) -> Record {
        let delta = i64::try_from(delta).unwrap_or(i64::MAX);
        match self {
            Self::BytesReturned => Record::metric(METRIC_AEX_STREAM_BYTES, delta),
            counted => Record::metric(METRIC_AEX_STREAM_COUNT, delta)
                .with(AEX_STREAM_COUNTER, counted.as_str()),
        }
    }
}

/// The fixed resource identity every published delta carries.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CounterResource {
    /// Deployment plane the emitting process belongs to.
    pub plane: String,
    /// Region the emitting process is bound to.
    pub region: String,
    /// Name of the emitting deployable.
    pub deployable: &'static str,
}

/// Every socket's read accounting for one process.
///
/// Shared by clone of the `Arc`, never by lock: an increment on the read path
/// must not be able to contend with an increment on another socket's read path.
#[derive(Debug, Default)]
pub struct ReadCounters {
    totals: [AtomicU64; ReadCounter::COUNT],
    published: [AtomicU64; ReadCounter::COUNT],
}

impl ReadCounters {
    /// Counts one occurrence.
    pub fn record(&self, counter: ReadCounter) {
        self.add(counter, 1);
    }

    /// Counts `amount` occurrences.
    pub fn add(&self, counter: ReadCounter, amount: u64) {
        self.totals[counter.slot()].fetch_add(amount, Ordering::Relaxed);
    }

    /// The lifetime total of one counter.
    #[must_use]
    pub fn total(&self, counter: ReadCounter) -> u64 {
        self.totals[counter.slot()].load(Ordering::Relaxed)
    }

    /// Takes every counter's movement since the previous drain.
    ///
    /// Only counters that moved are returned, so an idle process publishes
    /// nothing rather than a page of zeroes.
    pub fn drain_deltas(&self) -> Vec<(ReadCounter, u64)> {
        ReadCounter::ALL
            .iter()
            .filter_map(|counter| {
                let total = self.totals[counter.slot()].load(Ordering::Relaxed);
                let previous = self.published[counter.slot()].swap(total, Ordering::Relaxed);
                let delta = total.saturating_sub(previous);
                (delta > 0).then_some((*counter, delta))
            })
            .collect()
    }

    /// One aggregate record per counter that moved since the previous drain.
    #[must_use]
    pub fn drain_records(&self, resource: &CounterResource) -> Vec<Record> {
        self.drain_deltas()
            .into_iter()
            .map(|(counter, delta)| {
                counter
                    .record(delta)
                    .with(AEX_PLANE, resource.plane.clone())
                    .with(AEX_REGION, resource.region.clone())
                    .with(AEX_DEPLOYABLE, resource.deployable)
            })
            .collect()
    }
}

/// Publishes aggregate deltas every `interval` until the process drains.
///
/// The drain publication is not optional: a socket's last minute is exactly the
/// minute a capacity investigation cares about, and dropping it would make a
/// rotate look free.
pub async fn publish(
    counters: Arc<ReadCounters>,
    telemetry: Handle,
    resource: CounterResource,
    interval: Duration,
    draining: Arc<AtomicBool>,
) {
    let emit = |counters: &ReadCounters| {
        for record in counters.drain_records(&resource) {
            telemetry.emit(record);
        }
    };
    let mut due = tokio::time::Instant::now() + interval;
    loop {
        tokio::time::sleep(DRAIN_CHECK.min(interval)).await;
        if draining.load(Ordering::Acquire) {
            emit(&counters);
            return;
        }
        if tokio::time::Instant::now() >= due {
            emit(&counters);
            due = tokio::time::Instant::now() + interval;
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::Duration;

    use aex_platform_telemetry::{AttributeValue, InMemoryExporter, Record, RecordKind, Settings};
    use aex_telemetry_schema::generated::{
        AEX_STREAM_COUNTER, METRIC_AEX_STREAM_BYTES, METRIC_AEX_STREAM_COUNT,
    };

    use super::{CounterResource, ReadCounter, ReadCounters, publish};

    fn resource() -> CounterResource {
        CounterResource {
            plane: "dev".to_owned(),
            region: "eu-west-1".to_owned(),
            deployable: "regional-stream",
        }
    }

    fn value_of(records: &[Record], counter: ReadCounter) -> Option<i64> {
        records
            .iter()
            .find(|record| match counter {
                ReadCounter::BytesReturned => record.name == METRIC_AEX_STREAM_BYTES,
                named => {
                    record.name == METRIC_AEX_STREAM_COUNT
                        && record.attribute(AEX_STREAM_COUNTER)
                            == Some(&AttributeValue::Text(named.as_str().to_owned()))
                }
            })
            .and_then(|record| record.value)
    }

    #[test]
    fn every_counter_occupies_its_own_declaration_ordered_slot() {
        for (index, counter) in ReadCounter::ALL.iter().enumerate() {
            assert_eq!(counter.slot(), index, "{counter:?}");
        }
        assert_eq!(ReadCounter::COUNT, ReadCounter::ALL.len());
    }

    #[test]
    fn counter_dimension_values_are_unique_and_within_the_declared_length() {
        let mut seen = std::collections::BTreeSet::new();
        for counter in ReadCounter::ALL {
            assert!(seen.insert(counter.as_str()), "{counter:?} is not unique");
            assert!(counter.as_str().len() <= 24, "{counter:?} exceeds max_len");
        }
    }

    #[test]
    fn one_representative_idle_cycle_reports_exactly_what_it_did() {
        // A session-scoped four-signal follow that produced nothing: one batched
        // frontier read, one renewal, one page query per open segment, and one
        // fallback wake-up. No gap-history query: the hint rides the frontier
        // batch, and a steady-state cycle reads the ledger only when it moves.
        let counters = ReadCounters::default();
        counters.record(ReadCounter::AuthorizationRenewal);
        counters.record(ReadCounter::SessionHead);
        counters.record(ReadCounter::FrontierBatch);
        counters.add(ReadCounter::ObservationPage, 10);
        counters.record(ReadCounter::FallbackPoll);

        let records = counters.drain_records(&resource());

        assert_eq!(value_of(&records, ReadCounter::FrontierBatch), Some(1));
        assert_eq!(value_of(&records, ReadCounter::SessionHead), Some(1));
        assert_eq!(value_of(&records, ReadCounter::ObservationPage), Some(10));
        assert_eq!(
            value_of(&records, ReadCounter::AuthorizationRenewal),
            Some(1)
        );
        assert_eq!(value_of(&records, ReadCounter::FallbackPoll), Some(1));
        assert_eq!(
            value_of(&records, ReadCounter::GapHistory),
            None,
            "an unchanged hint is a cycle that queried no gap history at all"
        );
        assert_eq!(
            value_of(&records, ReadCounter::RecordsReturned),
            None,
            "an idle cycle returned nothing, so nothing is published"
        );
        assert_eq!(records.len(), 5, "only the counters that moved publish");
        assert!(
            records
                .iter()
                .all(|record| record.kind == RecordKind::Metric)
        );
    }

    #[test]
    fn a_publication_reports_the_delta_rather_than_the_running_total() {
        let counters = ReadCounters::default();
        counters.add(ReadCounter::FrontierBatch, 4);
        assert_eq!(
            value_of(
                &counters.drain_records(&resource()),
                ReadCounter::FrontierBatch
            ),
            Some(4)
        );
        assert!(
            counters.drain_records(&resource()).is_empty(),
            "an unchanged counter republishes nothing"
        );
        counters.add(ReadCounter::FrontierBatch, 3);
        assert_eq!(
            value_of(
                &counters.drain_records(&resource()),
                ReadCounter::FrontierBatch
            ),
            Some(3)
        );
        assert_eq!(counters.total(ReadCounter::FrontierBatch), 7);
    }

    #[test]
    fn bytes_carry_their_own_instrument_rather_than_a_dimensionless_count() {
        let counters = ReadCounters::default();
        counters.add(ReadCounter::BytesReturned, 4_096);
        let records = counters.drain_records(&resource());
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].name, METRIC_AEX_STREAM_BYTES);
        assert_eq!(records[0].value, Some(4_096));
        assert_eq!(records[0].attribute(AEX_STREAM_COUNTER), None);
    }

    #[tokio::test]
    async fn a_draining_process_publishes_its_last_delta_before_the_task_ends() {
        let exporter = Arc::new(InMemoryExporter::default());
        let telemetry = aex_platform_telemetry::Handle::install(
            &Settings::default(),
            Some(Arc::clone(&exporter) as Arc<dyn aex_platform_telemetry::Exporter>),
        );
        let counters = Arc::new(ReadCounters::default());
        let draining = Arc::new(AtomicBool::new(false));
        // An interval far longer than the test: what is proven here is that the
        // drain publishes without waiting for it.
        let task = tokio::spawn(publish(
            Arc::clone(&counters),
            telemetry.clone(),
            resource(),
            Duration::from_hours(1),
            Arc::clone(&draining),
        ));

        counters.record(ReadCounter::SocketOpened);
        counters.record(ReadCounter::SocketClosed);
        draining.store(true, Ordering::Release);
        task.await
            .expect("the publisher stops when the process drains");

        assert_eq!(
            telemetry.pending(),
            2,
            "the drain published every counter that moved"
        );
    }
}
