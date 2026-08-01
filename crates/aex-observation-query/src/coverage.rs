//! The pinned snapshot, the settle window and coverage honesty.
//!
//! `gsi_scope_time`, `gsi_ws_time` and `gsi_ws_accepted` are eventually
//! consistent. That staleness is made **exact** rather than hidden:
//! the snapshot is `min(frontier.acceptedAt, now - AEX_OBS_INDEX_SETTLE_MS)`.
//!
//! Admission asserts `|now - acceptedAt| < AEX_OBS_CLOCK_SKEW_MAX_MS` at commit
//! and fails closed otherwise, so the settle window provably dominates both
//! propagation and skew, and everything at or below the snapshot is complete in
//! every index by construction.
//!
//! `caughtUp: false` therefore means "inside a two-second window", not "an
//! unbounded materializer backlog". An empty successful page is **never**
//! returned in place of an error.

use aex_observation_domain::limits;
use aex_wire::types::{DecimalU128, Timestamp};

/// The accepted-time position a query is pinned to.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct Snapshot {
    accepted_at: Timestamp,
}

impl Snapshot {
    /// Pins a snapshot from the frontier and the current clock.
    ///
    /// Returns `None` when `now` minus the settle window is not representable,
    /// which can only happen at the very beginning of the wire timestamp range.
    #[must_use]
    pub fn pin(frontier_accepted_at: Timestamp, now: Timestamp) -> Option<Self> {
        let settled = Timestamp::from_unix_millis(
            now.unix_millis().checked_sub(limits::OBS_INDEX_SETTLE_MS)?,
        )
        .ok()?;
        Some(Self {
            accepted_at: frontier_accepted_at.min(settled),
        })
    }

    /// The pinned position.
    #[must_use]
    pub const fn accepted_at(self) -> Timestamp {
        self.accepted_at
    }

    /// The wire rendering: an accepted-time position in epoch milliseconds.
    ///
    /// The frozen wire carries one scalar per watermark, and a
    /// per-`(scope, signal)` sequence is not a scalar. Accepted time is
    /// monotone, comparable across signals and directly usable for interval
    /// reporting (decision O-04).
    #[must_use]
    pub fn to_wire(self) -> DecimalU128 {
        DecimalU128::new(u128::try_from(self.accepted_at.unix_millis().max(0)).unwrap_or(0))
    }
}

/// What an answer is actually complete over.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Coverage {
    /// The pinned accepted-time position.
    pub snapshot: DecimalU128,
    /// The frontier across the queried signals.
    pub accepted: DecimalU128,
    /// The indexed position, equal to the snapshot in the normal path.
    pub indexed: DecimalU128,
    /// The earliest retained accepted position for the scope.
    ///
    /// Removing Kinesis removed the only mechanism that could expire a replay,
    /// so this is the beginning of retained data rather than a 168-hour horizon.
    pub earliest_replay: DecimalU128,
    /// Whether indexing has reached admission.
    pub caught_up: bool,
    /// Whether the window has no known hole.
    pub complete: bool,
    /// Known holes with a provable extent.
    pub missing_intervals: Vec<(String, Timestamp, Timestamp)>,
    /// Open gaps whose extent is unknown.
    pub unbounded_gaps: u32,
}

impl Coverage {
    /// Builds coverage from the pinned snapshot and the frontier.
    #[must_use]
    pub fn new(snapshot: Snapshot, accepted: Timestamp, earliest: Timestamp) -> Self {
        let snapshot_wire = snapshot.to_wire();
        let accepted_wire =
            DecimalU128::new(u128::try_from(accepted.unix_millis().max(0)).unwrap_or(0));
        Self {
            snapshot: snapshot_wire,
            accepted: accepted_wire,
            indexed: snapshot_wire,
            earliest_replay: DecimalU128::new(
                u128::try_from(earliest.unix_millis().max(0)).unwrap_or(0),
            ),
            caught_up: accepted_wire == snapshot_wire,
            complete: true,
            missing_intervals: Vec::new(),
            unbounded_gaps: 0,
        }
    }

    /// Records the gaps that intersect the query.
    #[must_use]
    pub fn with_gaps(
        mut self,
        missing: Vec<(String, Timestamp, Timestamp)>,
        unbounded: u32,
    ) -> Self {
        self.complete = missing.is_empty() && unbounded == 0;
        self.missing_intervals = missing;
        self.unbounded_gaps = unbounded;
        self
    }
}

/// How much of the accepted series a read must observe.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Consistency {
    /// Return the pinned snapshot immediately.
    Available,
    /// Poll the frontier and the settle clock up to `wait_ms`, then report
    /// whatever is true. Never a fabricated `caughtUp: true`.
    CaughtUp {
        /// How long the read may wait.
        wait_ms: u32,
    },
}

impl Consistency {
    /// Whether the read has waited long enough to stop.
    ///
    /// Stops as soon as the frontier is caught up, and otherwise at the
    /// deadline, reporting the truth rather than the request.
    #[must_use]
    pub const fn should_stop(self, elapsed_ms: u32, caught_up: bool) -> bool {
        match self {
            Self::Available => true,
            Self::CaughtUp { wait_ms } => caught_up || elapsed_ms >= wait_ms,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Consistency, Coverage, Snapshot};
    use aex_observation_domain::limits;
    use aex_wire::types::Timestamp;

    fn instant(millis: i64) -> Timestamp {
        Timestamp::from_unix_millis(millis).expect("representable")
    }

    #[test]
    fn the_snapshot_subtracts_the_settle_window_from_now() {
        let now = instant(1_000_000);
        let frontier = instant(999_999_999);
        let snapshot = Snapshot::pin(frontier, now).expect("pins");
        assert_eq!(
            snapshot.accepted_at(),
            instant(1_000_000 - limits::OBS_INDEX_SETTLE_MS)
        );
    }

    #[test]
    fn a_frontier_behind_the_settle_window_pins_the_frontier() {
        let now = instant(1_000_000);
        let frontier = instant(500_000);
        let snapshot = Snapshot::pin(frontier, now).expect("pins");
        assert_eq!(snapshot.accepted_at(), frontier);
    }

    #[test]
    fn caught_up_is_reported_rather_than_fabricated() {
        let now = instant(1_000_000);
        let lagging = Snapshot::pin(instant(999_999_999), now).expect("pins");
        let coverage = Coverage::new(lagging, instant(1_000_000), instant(0));
        assert!(
            !coverage.caught_up,
            "the frontier is ahead of the settle window"
        );
        assert!(coverage.complete);

        let settled = Snapshot::pin(instant(10), now).expect("pins");
        let caught = Coverage::new(settled, instant(10), instant(0));
        assert!(caught.caught_up);
    }

    #[test]
    fn any_intersecting_gap_makes_the_window_incomplete() {
        let snapshot = Snapshot::pin(instant(10), instant(1_000_000)).expect("pins");
        let coverage = Coverage::new(snapshot, instant(10), instant(0))
            .with_gaps(vec![("gap_x".to_owned(), instant(1), instant(2))], 0);
        assert!(!coverage.complete);
        assert_eq!(coverage.missing_intervals.len(), 1);

        let unbounded = Coverage::new(snapshot, instant(10), instant(0)).with_gaps(Vec::new(), 1);
        assert!(
            !unbounded.complete,
            "an unbounded gap is incomplete even with no reportable interval"
        );
        assert!(unbounded.missing_intervals.is_empty());
    }

    #[test]
    fn a_caught_up_read_stops_at_the_truth_or_the_deadline() {
        let mode = Consistency::CaughtUp { wait_ms: 500 };
        assert!(!mode.should_stop(0, false));
        assert!(mode.should_stop(0, true), "stops as soon as it is true");
        assert!(mode.should_stop(500, false), "stops at the deadline");
        assert!(Consistency::Available.should_stop(0, false));
    }

    #[test]
    fn the_watermarks_render_as_epoch_millisecond_positions() {
        let snapshot = Snapshot::pin(instant(1_234), instant(1_000_000)).expect("pins");
        let coverage = Coverage::new(snapshot, instant(9_999), instant(7));
        assert_eq!(coverage.snapshot.get(), 1_234);
        assert_eq!(coverage.accepted.get(), 9_999);
        assert_eq!(coverage.earliest_replay.get(), 7);
        assert_eq!(coverage.indexed, coverage.snapshot);
    }
}
