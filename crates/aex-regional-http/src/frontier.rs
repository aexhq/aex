//! How far behind the regional projection is, stated as a number.
//!
//! Every pause, revocation, key change and limit change in this system takes
//! effect by being projected: finance commits in Aurora, the control worker
//! drains a queue, and the regional edge reads a replica. "Takes effect" is
//! therefore a duration, and until now nothing measured it — `FeedFrontier`
//! carried `covered_through` and had no production reader at all.
//!
//! # Why this is measured on a schedule and not per request
//!
//! A per-request frontier read would double the reads on the hot admission path
//! to bound a fact that changes on operator-paced events, and would hand every
//! request an answer no request can act on. The bound belongs to the deployment,
//! so it is measured on the deployment's own cadence and published as one
//! series.
//!
//! # Why breaching the ceiling pages instead of failing closed
//!
//! Refusing admission while the frontier lags would trade a bounded, priced
//! exposure — a paused account consuming capacity for at most the ceiling — for
//! an unbounded one: a total regional outage caused by a slow queue. That is not
//! what deprioritising availability means. The failure stays loud rather than
//! becoming silent, which is the whole point of measuring it.

use aex_wire::types::Timestamp;

/// The lag this plane intends to hold, at the 99th percentile.
///
/// Not a guarantee and not enforced anywhere — it is the number a change to the
/// control queue's batching or visibility settings has to be checked against.
pub const TARGET_P99_MS: i64 = 30_000;

/// The lag that pages somebody.
///
/// A cost decision: at the launch rate card, a paused account consuming capacity
/// for two minutes is a bounded and small bill, and it is the cheaper side of
/// the trade against refusing an entire region's traffic because a queue is
/// slow. Restate it if the control queue ever gains a delay or a dead-letter
/// retry longer than this.
pub const CEILING_MS: i64 = 120_000;

/// What one frontier measurement says about the plane.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrontierHealth {
    /// Inside the intended bound.
    WithinTarget,
    /// Past the intended bound, inside the ceiling. Visible, not paged.
    OverTarget,
    /// Past the ceiling. This pages; it does not refuse traffic.
    OverCeiling,
}

/// One measurement of how far the projection lags its authority.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FrontierLag {
    /// Milliseconds between now and the covered-through instant.
    ///
    /// Never negative. A `covered_through` in the future is a clock disagreement
    /// between two planes, not negative lag, and clamping it to zero keeps a
    /// skewed clock from reading as impossibly healthy in the other direction.
    pub millis: i64,
    /// Which side of the two bounds the measurement fell on.
    pub health: FrontierHealth,
}

impl FrontierLag {
    /// Measures the lag of `covered_through` as of `now`.
    #[must_use]
    pub fn measure(now: Timestamp, covered_through: Timestamp) -> Self {
        let millis = now
            .unix_millis()
            .saturating_sub(covered_through.unix_millis())
            .max(0);
        let health = if millis > CEILING_MS {
            FrontierHealth::OverCeiling
        } else if millis > TARGET_P99_MS {
            FrontierHealth::OverTarget
        } else {
            FrontierHealth::WithinTarget
        };
        Self { millis, health }
    }

    /// Whether this measurement should page.
    #[must_use]
    pub const fn pages(self) -> bool {
        matches!(self.health, FrontierHealth::OverCeiling)
    }
}

#[cfg(test)]
mod tests {
    use super::{CEILING_MS, FrontierHealth, FrontierLag, TARGET_P99_MS};
    use aex_wire::types::Timestamp;

    fn at(millis: i64) -> Timestamp {
        Timestamp::from_unix_millis(millis).expect("a representable instant")
    }

    /// The two bounds are ordered and both are stated in one place.
    ///
    /// They are a cost decision, not an arbitrary pair: the ceiling is how long
    /// a paused account may keep consuming paid capacity, and the target is what
    /// a change to the control queue's settings must be re-checked against.
    #[test]
    fn the_stated_bound_is_thirty_seconds_with_a_two_minute_ceiling() {
        assert_eq!(TARGET_P99_MS, 30_000);
        assert_eq!(CEILING_MS, 120_000);
        assert!(TARGET_P99_MS < CEILING_MS);
    }

    #[test]
    fn each_side_of_each_bound_classifies_the_way_the_alarm_reads_it() {
        let now = at(1_000_000_000);
        for (lag, expected) in [
            (0, FrontierHealth::WithinTarget),
            (TARGET_P99_MS, FrontierHealth::WithinTarget),
            (TARGET_P99_MS + 1, FrontierHealth::OverTarget),
            (CEILING_MS, FrontierHealth::OverTarget),
            (CEILING_MS + 1, FrontierHealth::OverCeiling),
        ] {
            let measured = FrontierLag::measure(now, at(now.unix_millis() - lag));
            assert_eq!(measured.millis, lag);
            assert_eq!(measured.health, expected, "lag {lag}");
        }
    }

    /// Only the ceiling pages, and paging is the whole of what it does.
    ///
    /// A breach must not fail admission closed: that converts a bounded billing
    /// exposure into an unbounded regional outage, which is the opposite of the
    /// trade this bound exists to make.
    #[test]
    fn a_breach_is_loud_and_is_not_a_refusal() {
        let now = at(1_000_000_000);
        assert!(FrontierLag::measure(now, at(now.unix_millis() - CEILING_MS - 1)).pages());
        assert!(!FrontierLag::measure(now, at(now.unix_millis() - CEILING_MS)).pages());
        assert!(!FrontierLag::measure(now, now).pages());
    }

    /// A frontier ahead of the local clock is skew, not negative lag.
    #[test]
    fn a_frontier_ahead_of_the_clock_reads_as_zero_rather_than_as_impossibly_healthy() {
        let now = at(1_000_000_000);
        let measured = FrontierLag::measure(now, at(now.unix_millis() + 5_000));
        assert_eq!(measured.millis, 0);
        assert_eq!(measured.health, FrontierHealth::WithinTarget);
    }
}
