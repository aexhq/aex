//! The bounded, non-blocking path from a probe to `RecordFact`.
//!
//! A probe runs on a reactor thread and must never await `DynamoDB`. It offers a
//! draft to a bounded channel and returns immediately; the drain batches and
//! records. Two rules make the failure modes honest:
//!
//! - saturation is a typed refusal the caller sees, not a silent drop; and
//! - anything the `Drop` path could not offer lands in an overflow ledger, and a
//!   non-empty ledger fails the graceful drain so the process cannot exit clean.
//!
//! A lost fact is lost money, so "we dropped some" is not an acceptable quiet
//! outcome anywhere in this module.

use std::fmt;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

use aex_usage_domain::fact::FactDraft;

use super::ProbeError;

/// How full the sink is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum SinkPressure {
    /// Below a quarter full.
    Idle,
    /// Between a quarter and full.
    Filling,
    /// At capacity; every further offer is refused.
    Saturated,
}

/// Why an offer was refused.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SinkError {
    /// The bounded channel was full.
    #[error("fact sink saturated at {queued}/{capacity} queued drafts")]
    Saturated {
        /// Drafts already queued.
        queued: usize,
        /// The channel capacity.
        capacity: usize,
    },
    /// The drain has shut down and will accept nothing further.
    #[error("fact sink is closed; the drain has already stopped")]
    Closed,
}

/// The non-blocking side a probe holds.
pub trait FactSink: std::fmt::Debug + Send + Sync + 'static {
    /// Offers one draft. Never blocks and never awaits.
    ///
    /// # Errors
    ///
    /// Returns [`SinkError::Saturated`] at capacity and [`SinkError::Closed`]
    /// once the drain has stopped.
    fn offer(&self, draft: FactDraft) -> Result<(), SinkError>;

    /// How full the sink currently is.
    fn pressure(&self) -> SinkPressure;
}

/// Drafts a `Drop` path could not offer.
///
/// A `Drop` cannot return an error and cannot await, so anything it fails to
/// offer is parked here. The graceful drain refuses to report success while this
/// is non-empty.
#[derive(Debug, Default)]
pub struct OverflowLedger {
    entries: std::sync::Mutex<Vec<FactDraft>>,
    recorded: AtomicU64,
}

impl OverflowLedger {
    /// An empty ledger.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Parks one draft that could not be offered.
    pub fn park(&self, draft: FactDraft) {
        self.recorded.fetch_add(1, Ordering::Relaxed);
        if let Ok(mut entries) = self.entries.lock() {
            entries.push(draft);
        }
    }

    /// How many drafts have ever been parked.
    #[must_use]
    pub fn parked_total(&self) -> u64 {
        self.recorded.load(Ordering::Relaxed)
    }

    /// How many drafts are still parked.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.lock().map_or(0, |entries| entries.len())
    }

    /// Whether the ledger is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Takes every parked draft, leaving the ledger empty.
    #[must_use]
    pub fn take(&self) -> Vec<FactDraft> {
        self.entries
            .lock()
            .map(|mut entries| std::mem::take(&mut *entries))
            .unwrap_or_default()
    }
}

/// How the drain batches.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DrainPolicy {
    /// The largest batch handed to `RecordFact` at once.
    pub max_batch: usize,
    /// How long the drain waits to fill a batch before flushing a partial one.
    pub batch_window: std::time::Duration,
}

impl DrainPolicy {
    /// Builds a policy, refusing a zero batch size.
    ///
    /// # Errors
    ///
    /// Returns [`ProbeError::Configuration`] when `max_batch` is zero, which
    /// would stall the drain forever.
    pub fn new(max_batch: usize, batch_window: std::time::Duration) -> Result<Self, ProbeError> {
        if max_batch == 0 {
            return Err(ProbeError::Configuration {
                what: "drain max_batch",
                reason: "a zero batch size would never flush".to_owned(),
            });
        }
        Ok(Self {
            max_batch,
            batch_window,
        })
    }
}

/// Why a drain could not complete.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum DrainError {
    /// Drafts remained unrecorded when the drain was asked to finish.
    ///
    /// This is deliberately fatal: a lost fact is lost money, so the process is
    /// not allowed to exit clean while any draft is unaccounted for.
    #[error("graceful drain left {queued} queued and {overflowed} overflowed drafts unrecorded")]
    Unflushed {
        /// Drafts still in the channel.
        queued: usize,
        /// Drafts parked in the overflow ledger.
        overflowed: usize,
    },
    /// The recording port refused a batch.
    #[error("recording {count} drafts failed: {reason}")]
    Record {
        /// How many drafts were in the refused batch.
        count: usize,
        /// What the port reported.
        reason: String,
    },
}

/// What one drain run accounted for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct DrainReport {
    /// Drafts handed to `RecordFact`.
    pub recorded: u64,
    /// Batches issued.
    pub batches: u64,
    /// Offers refused because the channel was full.
    pub shed: u64,
    /// Drafts recovered from the overflow ledger.
    pub recovered: u64,
}

/// The bounded in-memory sink.
///
/// Capacity is derived from the host's admission caps, so saturation is
/// structurally reachable only under a defect rather than under normal load.
#[derive(Debug)]
pub struct BoundedFactSink {
    queue: std::sync::Mutex<std::collections::VecDeque<FactDraft>>,
    capacity: usize,
    queued: AtomicUsize,
    shed: AtomicU64,
    closed: std::sync::atomic::AtomicBool,
    overflow: Arc<OverflowLedger>,
}

impl BoundedFactSink {
    /// Builds a sink with a bounded capacity.
    ///
    /// # Errors
    ///
    /// Returns [`ProbeError::Configuration`] for a zero capacity: an unbounded
    /// or zero-length sink is not a sink.
    pub fn new(capacity: usize, overflow: Arc<OverflowLedger>) -> Result<Self, ProbeError> {
        if capacity == 0 {
            return Err(ProbeError::Configuration {
                what: "fact sink capacity",
                reason: "a zero-capacity sink refuses every fact".to_owned(),
            });
        }
        Ok(Self {
            queue: std::sync::Mutex::new(std::collections::VecDeque::with_capacity(capacity)),
            capacity,
            queued: AtomicUsize::new(0),
            shed: AtomicU64::new(0),
            closed: std::sync::atomic::AtomicBool::new(false),
            overflow,
        })
    }

    /// How many drafts are queued.
    #[must_use]
    pub fn queued(&self) -> usize {
        self.queued.load(Ordering::Acquire)
    }

    /// How many offers have been refused for saturation.
    #[must_use]
    pub fn shed_total(&self) -> u64 {
        self.shed.load(Ordering::Relaxed)
    }

    /// The overflow ledger this sink parks `Drop`-path drafts in.
    #[must_use]
    pub fn overflow(&self) -> &Arc<OverflowLedger> {
        &self.overflow
    }

    /// Stops accepting offers.
    pub fn close(&self) {
        self.closed.store(true, Ordering::Release);
    }

    /// Takes up to `max` queued drafts.
    #[must_use]
    pub fn take_batch(&self, max: usize) -> Vec<FactDraft> {
        let Ok(mut queue) = self.queue.lock() else {
            return Vec::new();
        };
        let count = max.min(queue.len());
        let batch: Vec<FactDraft> = queue.drain(..count).collect();
        self.queued.fetch_sub(batch.len(), Ordering::AcqRel);
        batch
    }

    /// Offers a draft, parking it in the overflow ledger when it cannot be
    /// queued. Used by `Drop` paths, which cannot propagate an error.
    pub fn offer_or_park(&self, draft: FactDraft) {
        if let Err(SinkError::Saturated { .. } | SinkError::Closed) = self.offer(draft.clone()) {
            self.overflow.park(draft);
        }
    }
}

impl FactSink for BoundedFactSink {
    fn offer(&self, draft: FactDraft) -> Result<(), SinkError> {
        if self.closed.load(Ordering::Acquire) {
            return Err(SinkError::Closed);
        }
        let Ok(mut queue) = self.queue.lock() else {
            return Err(SinkError::Closed);
        };
        if queue.len() >= self.capacity {
            self.shed.fetch_add(1, Ordering::Relaxed);
            return Err(SinkError::Saturated {
                queued: queue.len(),
                capacity: self.capacity,
            });
        }
        queue.push_back(draft);
        self.queued.store(queue.len(), Ordering::Release);
        Ok(())
    }

    fn pressure(&self) -> SinkPressure {
        let queued = self.queued();
        if queued >= self.capacity {
            SinkPressure::Saturated
        } else if queued * 4 >= self.capacity {
            SinkPressure::Filling
        } else {
            SinkPressure::Idle
        }
    }
}

impl fmt::Display for SinkPressure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Idle => "idle",
            Self::Filling => "filling",
            Self::Saturated => "saturated",
        })
    }
}

#[cfg(test)]
mod tests {
    use super::{BoundedFactSink, DrainPolicy, FactSink, OverflowLedger, SinkError, SinkPressure};
    use crate::probe::testing::draft_fixture;
    use std::sync::Arc;

    fn sink(capacity: usize) -> BoundedFactSink {
        BoundedFactSink::new(capacity, Arc::new(OverflowLedger::new())).expect("capacity")
    }

    #[test]
    fn a_zero_capacity_sink_is_refused_at_construction() {
        assert!(BoundedFactSink::new(0, Arc::new(OverflowLedger::new())).is_err());
        assert!(DrainPolicy::new(0, std::time::Duration::from_millis(5)).is_err());
    }

    #[test]
    fn saturation_is_a_typed_refusal_not_a_silent_drop() {
        let sink = sink(2);
        sink.offer(draft_fixture(1)).expect("first");
        sink.offer(draft_fixture(2)).expect("second");

        let refused = sink.offer(draft_fixture(3)).expect_err("full");
        assert!(matches!(
            refused,
            SinkError::Saturated {
                queued: 2,
                capacity: 2
            }
        ));
        assert_eq!(sink.shed_total(), 1, "shedding is counted, never silent");
        assert_eq!(sink.pressure(), SinkPressure::Saturated);
    }

    #[test]
    fn pressure_tracks_the_queue_depth() {
        let sink = sink(8);
        assert_eq!(sink.pressure(), SinkPressure::Idle);
        for seed in 0..2 {
            sink.offer(draft_fixture(seed)).expect("offers");
        }
        assert_eq!(sink.pressure(), SinkPressure::Filling);
        for seed in 2..8 {
            sink.offer(draft_fixture(seed)).expect("offers");
        }
        assert_eq!(sink.pressure(), SinkPressure::Saturated);
    }

    #[test]
    fn a_drop_path_draft_that_cannot_queue_is_parked_never_lost() {
        let sink = sink(1);
        sink.offer(draft_fixture(1)).expect("first");

        sink.offer_or_park(draft_fixture(2));
        assert_eq!(sink.overflow().len(), 1, "a lost fact would be lost money");
        assert_eq!(sink.overflow().parked_total(), 1);

        let recovered = sink.overflow().take();
        assert_eq!(recovered.len(), 1);
        assert!(sink.overflow().is_empty());
        // The running total survives the take, so the alarm is not erased by
        // recovering the drafts.
        assert_eq!(sink.overflow().parked_total(), 1);
    }

    #[test]
    fn a_closed_sink_refuses_and_parks_rather_than_accepting() {
        let sink = sink(4);
        sink.close();
        assert!(matches!(
            sink.offer(draft_fixture(1)),
            Err(SinkError::Closed)
        ));
        sink.offer_or_park(draft_fixture(2));
        assert_eq!(sink.overflow().len(), 1);
    }

    #[test]
    fn batches_are_taken_in_offer_order_and_bounded() {
        let sink = sink(16);
        for seed in 0..5 {
            sink.offer(draft_fixture(seed)).expect("offers");
        }
        let batch = sink.take_batch(3);
        assert_eq!(batch.len(), 3);
        assert_eq!(sink.queued(), 2);

        let rest = sink.take_batch(100);
        assert_eq!(rest.len(), 2);
        assert_eq!(sink.queued(), 0);
        assert!(sink.take_batch(10).is_empty());
    }
}
