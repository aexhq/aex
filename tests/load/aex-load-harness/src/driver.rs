//! The driver that walks an arrival schedule.
//!
//! The driver's one hard promise is the accounting identity every capacity gate
//! rests on: `offered == completed + typed_rejected + failed`. A load run that
//! silently drops an offer would report a higher completion rate than the system
//! achieved, which is exactly the defect `LOAD-500-BOUNDED` exists to detect.

use std::sync::Arc;

use serde::{Deserialize, Serialize};
use tokio::sync::Semaphore;

use crate::arrival::{Arrival, ArrivalSchedule, Offer};

/// What happened to one offer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum OfferOutcome {
    /// The unit of work reached its terminal state.
    Completed,
    /// The system refused the work with a typed, retryable rejection. This is a
    /// correct behaviour under pressure, not a failure.
    TypedRejection {
        /// The rejection's type.
        reason: String,
    },
    /// The work neither completed nor was typed-rejected.
    Failed {
        /// What went wrong.
        reason: String,
    },
}

/// The system under test, as the driver sees it.
pub trait Workload: Send + Sync + 'static {
    /// Offers one unit of work.
    fn offer(&self, offer: Offer) -> impl Future<Output = OfferOutcome> + Send;
}

/// Why a campaign could not run.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum DriverError {
    /// Closed arrival with no agents in flight offers nothing.
    #[error("closed arrival declares concurrency 0; a campaign that offers nothing proves nothing")]
    ZeroConcurrency,
    /// An offer task did not finish.
    #[error("offer {seq} did not finish: {detail}")]
    OfferLost {
        /// The offer that was lost.
        seq: u64,
        /// Why the task ended.
        detail: String,
    },
    /// The accounting identity did not hold.
    #[error(
        "offered {offered} but accounted {accounted} (completed {completed}, typed_rejected {typed_rejected}, failed {failed}); a dropped offer inflates every rate in the report"
    )]
    LostWork {
        /// How many offers were made.
        offered: usize,
        /// How many were accounted for.
        accounted: usize,
        /// How many completed.
        completed: usize,
        /// How many were typed-rejected.
        typed_rejected: usize,
        /// How many failed.
        failed: usize,
    },
}

/// What one campaign did.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct RunSummary {
    /// How many offers were made.
    pub offered: usize,
    /// How many completed.
    pub completed: usize,
    /// How many were typed-rejected.
    pub typed_rejected: usize,
    /// How many failed.
    pub failed: usize,
}

impl RunSummary {
    /// Whether every offer is accounted for.
    #[must_use]
    pub const fn is_balanced(&self) -> bool {
        self.offered == self.completed + self.typed_rejected + self.failed
    }
}

/// Walks an [`ArrivalSchedule`] against a [`Workload`].
#[derive(Debug)]
pub struct Driver {
    schedule: ArrivalSchedule,
}

impl Driver {
    /// A driver for `schedule`.
    #[must_use]
    pub const fn new(schedule: ArrivalSchedule) -> Self {
        Self { schedule }
    }

    /// The schedule this driver walks.
    #[must_use]
    pub const fn schedule(&self) -> &ArrivalSchedule {
        &self.schedule
    }

    /// Runs the campaign and returns what happened.
    ///
    /// # Errors
    ///
    /// Returns [`DriverError`] when the schedule cannot be walked, when an
    /// offer task is lost, or when the accounting identity does not hold.
    pub async fn run<W: Workload>(&self, workload: Arc<W>) -> Result<RunSummary, DriverError> {
        let outcomes = match self.schedule.arrival {
            Arrival::Closed => self.run_closed(workload).await?,
            Arrival::Open => self.run_open(workload).await?,
        };
        let mut summary = RunSummary {
            offered: self.schedule.offered(),
            ..RunSummary::default()
        };
        for outcome in outcomes {
            match outcome {
                OfferOutcome::Completed => summary.completed += 1,
                OfferOutcome::TypedRejection { .. } => summary.typed_rejected += 1,
                OfferOutcome::Failed { .. } => summary.failed += 1,
            }
        }
        if summary.is_balanced() {
            Ok(summary)
        } else {
            Err(DriverError::LostWork {
                offered: summary.offered,
                accounted: summary.completed + summary.typed_rejected + summary.failed,
                completed: summary.completed,
                typed_rejected: summary.typed_rejected,
                failed: summary.failed,
            })
        }
    }

    async fn run_closed<W: Workload>(
        &self,
        workload: Arc<W>,
    ) -> Result<Vec<OfferOutcome>, DriverError> {
        if self.schedule.concurrency == 0 {
            return Err(DriverError::ZeroConcurrency);
        }
        let permits = Arc::new(Semaphore::new(self.schedule.concurrency as usize));
        let mut handles = Vec::with_capacity(self.schedule.offers.len());
        for offer in self.schedule.offers.clone() {
            let permits = Arc::clone(&permits);
            let workload = Arc::clone(&workload);
            let seq = offer.seq;
            handles.push((
                seq,
                tokio::spawn(async move {
                    let permit = permits
                        .acquire_owned()
                        .await
                        .expect("the campaign semaphore is never closed");
                    let outcome = workload.offer(offer).await;
                    drop(permit);
                    outcome
                }),
            ));
        }
        collect(handles).await
    }

    async fn run_open<W: Workload>(
        &self,
        workload: Arc<W>,
    ) -> Result<Vec<OfferOutcome>, DriverError> {
        let started = tokio::time::Instant::now();
        let mut handles = Vec::with_capacity(self.schedule.offers.len());
        for offer in self.schedule.offers.clone() {
            let due = started + std::time::Duration::from_millis(offer.due_at_ms);
            tokio::time::sleep_until(due).await;
            let workload = Arc::clone(&workload);
            let seq = offer.seq;
            handles.push((
                seq,
                tokio::spawn(async move { workload.offer(offer).await }),
            ));
        }
        collect(handles).await
    }
}

async fn collect(
    handles: Vec<(u64, tokio::task::JoinHandle<OfferOutcome>)>,
) -> Result<Vec<OfferOutcome>, DriverError> {
    let mut outcomes = Vec::with_capacity(handles.len());
    for (seq, handle) in handles {
        let outcome = handle.await.map_err(|error| DriverError::OfferLost {
            seq,
            detail: error.to_string(),
        })?;
        outcomes.push(outcome);
    }
    Ok(outcomes)
}

#[cfg(test)]
mod tests {
    use super::{Driver, DriverError, OfferOutcome, Workload};
    use crate::arrival::{ArrivalSchedule, Mix, Offer};
    use std::collections::BTreeMap;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn mix() -> Mix {
        Mix(BTreeMap::from([
            ("short_turn".to_owned(), 0.5),
            ("tool_heavy".to_owned(), 0.5),
        ]))
    }

    #[derive(Debug, Default)]
    struct Counting {
        in_flight: AtomicUsize,
        peak: AtomicUsize,
        seen: AtomicUsize,
    }

    impl Workload for Counting {
        async fn offer(&self, _offer: Offer) -> OfferOutcome {
            let now = self.in_flight.fetch_add(1, Ordering::SeqCst) + 1;
            self.peak.fetch_max(now, Ordering::SeqCst);
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
            self.in_flight.fetch_sub(1, Ordering::SeqCst);
            self.seen.fetch_add(1, Ordering::SeqCst);
            OfferOutcome::Completed
        }
    }

    #[derive(Debug)]
    struct Mixed;

    impl Workload for Mixed {
        async fn offer(&self, offer: Offer) -> OfferOutcome {
            match offer.seq % 3 {
                0 => OfferOutcome::Completed,
                1 => OfferOutcome::TypedRejection {
                    reason: "queue_full".to_owned(),
                },
                _ => OfferOutcome::Failed {
                    reason: "socket hang".to_owned(),
                },
            }
        }
    }

    #[tokio::test(flavor = "current_thread", start_paused = true)]
    async fn closed_arrival_never_exceeds_the_declared_concurrency() {
        let schedule = ArrivalSchedule::closed(4, 40, &mix(), 11);
        let workload = Arc::new(Counting::default());
        let summary = Driver::new(schedule)
            .run(Arc::clone(&workload))
            .await
            .expect("the campaign runs");
        assert_eq!(summary.offered, 40);
        assert_eq!(summary.completed, 40);
        assert!(summary.is_balanced());
        assert_eq!(workload.seen.load(Ordering::SeqCst), 40);
        assert!(
            workload.peak.load(Ordering::SeqCst) <= 4,
            "peak in flight {} exceeded the declared 4",
            workload.peak.load(Ordering::SeqCst)
        );
    }

    #[tokio::test(flavor = "current_thread", start_paused = true)]
    async fn every_offer_is_accounted_for_across_all_three_outcomes() {
        let schedule = ArrivalSchedule::closed(8, 30, &mix(), 3);
        let summary = Driver::new(schedule)
            .run(Arc::new(Mixed))
            .await
            .expect("the campaign runs");
        assert_eq!(summary.offered, 30);
        assert_eq!(summary.completed, 10);
        assert_eq!(summary.typed_rejected, 10);
        assert_eq!(summary.failed, 10);
        assert!(summary.is_balanced());
    }

    #[tokio::test(flavor = "current_thread", start_paused = true)]
    async fn open_arrival_offers_the_whole_schedule() {
        let schedule = ArrivalSchedule::open(200.0, 1_000, &mix(), 5);
        let offered = schedule.offered();
        let summary = Driver::new(schedule)
            .run(Arc::new(Mixed))
            .await
            .expect("the campaign runs");
        assert_eq!(summary.offered, offered);
        assert!(summary.is_balanced());
    }

    #[tokio::test(flavor = "current_thread", start_paused = true)]
    async fn closed_arrival_with_no_agents_is_a_typed_failure() {
        let schedule = ArrivalSchedule::closed(0, 10, &mix(), 1);
        let error = Driver::new(schedule)
            .run(Arc::new(Mixed))
            .await
            .expect_err("a campaign with no agents offers nothing");
        assert_eq!(error, DriverError::ZeroConcurrency);
    }
}
