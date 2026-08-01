//! The deterministic clock every fixture in this crate reads.
//!
//! Fixtures never call `OffsetDateTime::now_utc`. A test that depends on the
//! real clock cannot be replayed, and a failure it produces cannot be
//! distinguished from a scheduling accident.

use std::time::Duration;

use time::OffsetDateTime;

/// Milliseconds from the Unix epoch to `2026-01-01T00:00:00Z`, the instant every
/// deterministic fixture starts from.
pub const EPOCH_MILLIS: u64 = 1_767_225_600_000;

/// The default gap between two consecutive [`TestClock::tick`] calls.
pub const DEFAULT_STEP: Duration = Duration::from_millis(250);

/// An injected clock that only moves when a test moves it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TestClock {
    current: OffsetDateTime,
    step: Duration,
}

impl TestClock {
    /// A clock at [`EPOCH_MILLIS`] stepping by [`DEFAULT_STEP`].
    #[must_use]
    pub fn new() -> Self {
        Self::with_step(DEFAULT_STEP)
    }

    /// A clock at [`EPOCH_MILLIS`] stepping by `step`.
    #[must_use]
    pub fn with_step(step: Duration) -> Self {
        Self {
            current: OffsetDateTime::UNIX_EPOCH + Duration::from_millis(EPOCH_MILLIS),
            step,
        }
    }

    /// The current instant. Reading it never advances the clock.
    #[must_use]
    pub const fn now(&self) -> OffsetDateTime {
        self.current
    }

    /// Advances by the configured step and returns the new instant.
    pub fn tick(&mut self) -> OffsetDateTime {
        self.advance(self.step);
        self.current
    }

    /// Advances by an explicit duration.
    pub fn advance(&mut self, by: Duration) {
        self.current += by;
    }

    /// The current instant as whole milliseconds since the Unix epoch.
    #[must_use]
    pub fn unix_millis(&self) -> u64 {
        let millis = self.current.unix_timestamp_nanos() / 1_000_000;
        u64::try_from(millis).unwrap_or(EPOCH_MILLIS)
    }
}

impl Default for TestClock {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::{DEFAULT_STEP, EPOCH_MILLIS, TestClock};
    use std::time::Duration;

    #[test]
    fn a_new_clock_starts_at_the_fixed_epoch() {
        assert_eq!(TestClock::new().unix_millis(), EPOCH_MILLIS);
    }

    #[test]
    fn reading_the_clock_does_not_move_it() {
        let clock = TestClock::new();
        assert_eq!(clock.now(), clock.now());
        assert_eq!(clock.unix_millis(), EPOCH_MILLIS);
    }

    #[test]
    fn ticking_advances_by_exactly_one_step() {
        let mut clock = TestClock::new();
        clock.tick();
        let step = u64::try_from(DEFAULT_STEP.as_millis()).expect("the step fits in a u64");
        assert_eq!(clock.unix_millis(), EPOCH_MILLIS + step);
    }

    #[test]
    fn two_clocks_built_the_same_way_agree_forever() {
        let mut left = TestClock::new();
        let mut right = TestClock::new();
        for _ in 0..64 {
            assert_eq!(left.tick(), right.tick());
        }
        assert_eq!(left, right);
    }

    #[test]
    fn advancing_uses_the_supplied_duration() {
        let mut clock = TestClock::new();
        clock.advance(Duration::from_secs(3));
        assert_eq!(clock.unix_millis(), EPOCH_MILLIS + 3_000);
    }
}
