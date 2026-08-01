//! The authority's own clock.
//!
//! Producer clocks stamp service time; this one stamps admission. Ordering and
//! idempotency never depend on a producer clock, which is why the authority
//! reads its own wall clock rather than trusting an instant that arrived over
//! the wire.

use std::time::{SystemTime, UNIX_EPOCH};

use aex_usage_application::ports::Clock;
use aex_usage_domain::wire_pending::Timestamp;

/// The process wall clock.
#[derive(Debug, Clone, Copy, Default)]
pub struct SystemClock;

impl SystemClock {
    /// A clock reading this process's wall clock.
    #[must_use]
    pub const fn new() -> Self {
        Self
    }

    /// The current instant, or why it could not be read.
    ///
    /// # Errors
    ///
    /// Returns the reason when the host clock is before the Unix epoch or past
    /// the representable range. A caller that needs an instant for money
    /// evidence gets the failure rather than a substituted one.
    pub fn read(self) -> Result<Timestamp, String> {
        let since = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|error| error.to_string())?;
        let millis =
            i64::try_from(since.as_millis()).map_err(|_| "the host clock is unrepresentable")?;
        Timestamp::from_unix_millis(millis).map_err(|error| error.to_string())
    }
}

impl Clock for SystemClock {
    /// The current instant.
    ///
    /// # Panics
    ///
    /// Panics when the host wall clock cannot be read. There is no honest
    /// instant to substitute — a stamped-wrong admission is money evidence that
    /// reads as ordinary — so the invocation fails loudly and the record is
    /// redelivered instead.
    fn now(&self) -> Timestamp {
        self.read()
            .expect("the host wall clock must be readable to stamp admission")
    }
}

#[cfg(test)]
mod tests {
    use super::SystemClock;
    use aex_usage_application::ports::Clock;

    #[test]
    fn the_authority_clock_advances_and_is_representable() {
        let clock = SystemClock::new();
        let read = clock.read().expect("a host clock is readable");
        assert!(
            read.unix_millis() > 1_700_000_000_000,
            "the authority clock must be a real instant, not the epoch"
        );
        assert!(
            Clock::now(&clock).unix_millis() >= read.unix_millis(),
            "the authority clock never moves backwards inside one process"
        );
    }
}
