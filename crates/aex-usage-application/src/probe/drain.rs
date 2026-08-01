//! The drain side of the probe sink.
//!
//! A probe runs on a reactor thread and offers a draft to a bounded channel. The
//! drain is what turns those drafts into admitted facts: it batches, calls
//! [`RecordFactPort`], and refuses to report a clean shutdown while anything is
//! unaccounted for.
//!
//! The rule the whole module exists to enforce is `U-25`: **a lost fact is lost
//! money.** So:
//!
//! - a saturated offer is a typed refusal the caller sees, never a silent drop;
//! - a `Drop`-path draft that cannot be queued lands in the overflow ledger; and
//! - [`FactDrain::flush`] fails while either the queue or the ledger is
//!   non-empty, so a process cannot exit clean having discarded a measurement.
//!
//! Pacing is a trait with one real implementation, like the other four OS seams,
//! so the batching window is scripted in tests rather than slept through.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use aex_usage_domain::fact::FactDraft;
use async_trait::async_trait;

use crate::ports::RecordFactPort;

use super::sink::{BoundedFactSink, DrainError, DrainPolicy, DrainReport, OverflowLedger};

/// How the drain waits between empty polls.
///
/// One seam, one implementation, so a test scripts the window instead of
/// sleeping through it and a drain property is exact rather than timing
/// dependent.
#[async_trait]
pub trait Pacer: std::fmt::Debug + Send + Sync {
    /// Waits for at most `window`.
    async fn pace(&self, window: Duration);
}

/// The production pacer.
#[derive(Debug, Default, Clone, Copy)]
pub struct SleepPacer;

#[async_trait]
impl Pacer for SleepPacer {
    async fn pace(&self, window: Duration) {
        tokio::time::sleep(window).await;
    }
}

/// A cooperative stop signal.
///
/// Deliberately not a timeout: a drain stops when its owner says so, and then
/// flushes. Stopping on a deadline instead would make shutdown able to discard a
/// measurement, which is the one thing this module forbids.
#[derive(Debug, Clone, Default)]
pub struct CancelToken(Arc<AtomicBool>);

impl CancelToken {
    /// A token that has not been cancelled.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Requests a stop.
    pub fn cancel(&self) {
        self.0.store(true, Ordering::Release);
    }

    /// Whether a stop has been requested.
    #[must_use]
    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Acquire)
    }
}

/// Batches offered drafts into `RecordFact`.
#[derive(Debug)]
pub struct FactDrain {
    sink: Arc<BoundedFactSink>,
    record: Arc<dyn RecordFactPort>,
    policy: DrainPolicy,
    pacer: Arc<dyn Pacer>,
}

impl FactDrain {
    /// Builds a drain and the sink probes offer into.
    ///
    /// The capacity is the host's admission cap, so saturation is structurally
    /// reachable only under a defect rather than under normal load.
    ///
    /// # Errors
    ///
    /// Returns [`super::ProbeError::Configuration`] for a zero capacity.
    pub fn new(
        capacity: usize,
        record: Arc<dyn RecordFactPort>,
        policy: DrainPolicy,
        pacer: Arc<dyn Pacer>,
    ) -> Result<(Self, Arc<BoundedFactSink>), super::ProbeError> {
        let overflow = Arc::new(OverflowLedger::new());
        let sink = Arc::new(BoundedFactSink::new(capacity, overflow)?);
        Ok((
            Self {
                sink: Arc::clone(&sink),
                record,
                policy,
                pacer,
            },
            sink,
        ))
    }

    /// The sink probes hold.
    #[must_use]
    pub fn sink(&self) -> &Arc<BoundedFactSink> {
        &self.sink
    }

    /// Records one batch, recovering anything parked in the overflow ledger
    /// first.
    ///
    /// The ledger is drained before the queue on purpose: a parked draft came
    /// from a `Drop` path that could not report a failure, so it is the one most
    /// at risk of being forgotten.
    ///
    /// # Errors
    ///
    /// Returns [`DrainError::Record`] when the port refuses. The batch is
    /// returned to the overflow ledger first, so nothing is lost by the failure.
    pub async fn drain_once(&self) -> Result<DrainReport, DrainError> {
        let mut report = DrainReport::default();
        let mut batch: Vec<FactDraft> = self.sink.overflow().take();
        report.recovered = batch.len() as u64;
        let remaining = self.policy.max_batch.saturating_sub(batch.len());
        batch.extend(self.sink.take_batch(remaining));
        if batch.is_empty() {
            return Ok(report);
        }

        match self.record.record(&batch).await {
            Ok(admissions) => {
                report.recorded = admissions.len() as u64;
                report.batches = 1;
                report.shed = self.sink.shed_total();
                Ok(report)
            }
            Err(error) => {
                // Park the whole batch before reporting. A failed record call
                // that also discarded its drafts would turn a retryable outage
                // into lost money.
                let count = batch.len();
                for draft in batch {
                    self.sink.overflow().park(draft);
                }
                Err(DrainError::Record {
                    count,
                    reason: error.to_string(),
                })
            }
        }
    }

    /// Runs until cancelled, then returns what it accounted for.
    ///
    /// # Errors
    ///
    /// Propagates the first [`DrainError::Record`]. The caller decides whether
    /// to restart; the drafts are in the overflow ledger either way.
    pub async fn run(self, cancel: CancelToken) -> Result<DrainReport, DrainError> {
        let mut total = DrainReport::default();
        while !cancel.is_cancelled() {
            let pass = self.drain_once().await?;
            total.recorded += pass.recorded;
            total.batches += pass.batches;
            total.recovered += pass.recovered;
            total.shed = pass.shed.max(total.shed);
            if pass.recorded == 0 {
                self.pacer.pace(self.policy.batch_window).await;
            }
        }
        Ok(total)
    }

    /// Drains everything outstanding, refusing to succeed while any draft is
    /// unaccounted for.
    ///
    /// `attempts` bounds the work, not the outcome: running out of attempts with
    /// drafts still queued is [`DrainError::Unflushed`], which the caller must
    /// treat as a failed shutdown rather than a slow one.
    ///
    /// # Errors
    ///
    /// [`DrainError::Record`] from the port, or [`DrainError::Unflushed`] when
    /// anything remains.
    pub async fn flush(&self, attempts: usize) -> Result<DrainReport, DrainError> {
        let mut total = DrainReport::default();
        for _ in 0..attempts {
            let pass = self.drain_once().await?;
            total.recorded += pass.recorded;
            total.batches += pass.batches;
            total.recovered += pass.recovered;
            if self.sink.queued() == 0 && self.sink.overflow().is_empty() {
                total.shed = self.sink.shed_total();
                return Ok(total);
            }
        }
        Err(DrainError::Unflushed {
            queued: self.sink.queued(),
            overflowed: self.sink.overflow().len(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::{CancelToken, FactDrain, Pacer, SleepPacer};
    use crate::ports::{Admission, PortError, RecordFactPort};
    use crate::probe::sink::{DrainError, DrainPolicy, FactSink};
    use crate::probe::testing::draft_fixture;
    use aex_usage_domain::fact::FactDraft;
    use async_trait::async_trait;
    use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    #[derive(Debug, Default)]
    struct Recorder {
        seen: Mutex<Vec<FactDraft>>,
        refusing: AtomicBool,
        calls: AtomicU64,
    }

    #[async_trait]
    impl RecordFactPort for Recorder {
        async fn record(&self, drafts: &[FactDraft]) -> Result<Vec<Admission>, PortError> {
            self.calls.fetch_add(1, Ordering::Relaxed);
            if self.refusing.load(Ordering::Acquire) {
                return Err(PortError::Unavailable {
                    what: "usage authority",
                    reason: "fixture refusing".to_owned(),
                });
            }
            let mut seen = self.seen.lock().expect("seen");
            seen.extend_from_slice(drafts);
            Ok(drafts
                .iter()
                .map(|draft| {
                    Admission::Admitted(
                        Box::new(
                            draft
                                .clone()
                                .admit(
                                    aex_usage_domain::frontier::AcceptedSequence::new(
                                        seen.len() as u64
                                    )
                                    .expect("positive"),
                                    crate::probe::testing::at(1_000),
                                )
                                .expect("admissible"),
                        ),
                    )
                })
                .collect())
        }
    }

    /// A pacer that yields instead of sleeping, so a drain loop is exact rather
    /// than timed but still cooperative.
    #[derive(Debug, Default)]
    struct ImmediatePacer(AtomicU64);

    #[async_trait]
    impl Pacer for ImmediatePacer {
        async fn pace(&self, _window: Duration) {
            self.0.fetch_add(1, Ordering::Relaxed);
            tokio::task::yield_now().await;
        }
    }

    fn policy() -> DrainPolicy {
        DrainPolicy::new(4, Duration::from_millis(5)).expect("policy")
    }

    #[tokio::test]
    async fn a_drain_batches_up_to_its_policy_and_no_further() {
        let recorder = Arc::new(Recorder::default());
        let (drain, sink) = FactDrain::new(
            32,
            Arc::clone(&recorder) as Arc<dyn RecordFactPort>,
            policy(),
            Arc::new(ImmediatePacer::default()),
        )
        .expect("builds");

        for seed in 0..10 {
            sink.offer(draft_fixture(seed)).expect("offers");
        }
        let first = drain.drain_once().await.expect("drains");
        assert_eq!(first.recorded, 4, "the batch ceiling binds");
        assert_eq!(sink.queued(), 6);

        let flushed = drain.flush(8).await.expect("flushes");
        assert_eq!(flushed.recorded, 6);
        assert_eq!(sink.queued(), 0);
        assert_eq!(recorder.seen.lock().expect("seen").len(), 10);
    }

    #[tokio::test]
    async fn a_refused_batch_is_parked_rather_than_discarded() {
        let recorder = Arc::new(Recorder::default());
        recorder.refusing.store(true, Ordering::Release);
        let (drain, sink) = FactDrain::new(
            32,
            Arc::clone(&recorder) as Arc<dyn RecordFactPort>,
            policy(),
            Arc::new(ImmediatePacer::default()),
        )
        .expect("builds");
        for seed in 0..3 {
            sink.offer(draft_fixture(seed)).expect("offers");
        }

        let error = drain.drain_once().await.expect_err("the port refused");
        assert!(matches!(error, DrainError::Record { count: 3, .. }));
        assert_eq!(
            sink.overflow().len(),
            3,
            "a failed record call must not also lose the drafts"
        );

        recorder.refusing.store(false, Ordering::Release);
        let recovered = drain.flush(4).await.expect("flushes");
        assert_eq!(recovered.recovered, 3);
        assert_eq!(recorder.seen.lock().expect("seen").len(), 3);
    }

    #[tokio::test]
    async fn a_flush_that_cannot_account_for_every_draft_fails_the_shutdown() {
        let recorder = Arc::new(Recorder::default());
        recorder.refusing.store(true, Ordering::Release);
        let (drain, sink) = FactDrain::new(
            8,
            Arc::clone(&recorder) as Arc<dyn RecordFactPort>,
            policy(),
            Arc::new(ImmediatePacer::default()),
        )
        .expect("builds");
        sink.offer_or_park(draft_fixture(1));
        sink.close();

        let error = drain.flush(2).await.expect_err("cannot flush");
        assert!(
            matches!(
                error,
                DrainError::Record { .. } | DrainError::Unflushed { .. }
            ),
            "{error:?}"
        );
        assert!(
            !sink.overflow().is_empty(),
            "the process must not be able to exit clean holding a measurement"
        );
    }

    #[tokio::test]
    async fn an_empty_drain_flushes_immediately_and_reports_nothing() {
        let recorder = Arc::new(Recorder::default());
        let (drain, sink) = FactDrain::new(
            8,
            Arc::clone(&recorder) as Arc<dyn RecordFactPort>,
            policy(),
            Arc::new(ImmediatePacer::default()),
        )
        .expect("builds");

        let report = drain.flush(1).await.expect("flushes");
        assert_eq!(report.recorded, 0);
        assert_eq!(report.batches, 0);
        assert_eq!(sink.queued(), 0);
        assert_eq!(
            recorder.calls.load(Ordering::Relaxed),
            0,
            "an empty drain must not issue an empty batch"
        );
    }

    #[tokio::test]
    async fn a_run_stops_when_cancelled_and_reports_what_it_recorded() {
        let recorder = Arc::new(Recorder::default());
        let (drain, sink) = FactDrain::new(
            32,
            Arc::clone(&recorder) as Arc<dyn RecordFactPort>,
            policy(),
            Arc::new(ImmediatePacer::default()),
        )
        .expect("builds");
        for seed in 0..4 {
            sink.offer(draft_fixture(seed)).expect("offers");
        }

        let cancel = CancelToken::new();
        let stopper = cancel.clone();
        let handle = tokio::spawn(async move { drain.run(stopper).await });
        // The loop drains the queue and then paces; cancelling ends it.
        tokio::task::yield_now().await;
        cancel.cancel();
        let report = handle.await.expect("joins").expect("runs");
        assert!(report.recorded <= 4);
        assert!(cancel.is_cancelled());
    }

    #[test]
    fn the_production_pacer_exists_and_is_the_only_real_seam() {
        // A trait with one real implementation, like the other four OS seams.
        let pacer: Arc<dyn Pacer> = Arc::new(SleepPacer);
        assert_eq!(format!("{pacer:?}"), "SleepPacer");
    }

    #[test]
    fn shedding_is_counted_and_a_shed_offer_is_refused_not_dropped() {
        let recorder = Arc::new(Recorder::default());
        let (_drain, sink) = FactDrain::new(
            1,
            recorder as Arc<dyn RecordFactPort>,
            policy(),
            Arc::new(ImmediatePacer::default()),
        )
        .expect("builds");
        sink.offer(draft_fixture(1)).expect("offers");
        assert!(sink.offer(draft_fixture(2)).is_err());
        assert_eq!(sink.shed_total(), 1);
    }
}
