//! Bounds the facade enforces.
//!
//! Every field is a bound, not a target. There is no "unbounded" setting,
//! because an unbounded diagnostic queue turns a slow collector into a process
//! outage, and an unbounded flush turns a diagnostic into a customer deadline.

use std::time::Duration;

/// The bounds one [`crate::facade::Handle`] enforces.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Settings {
    /// Maximum records held for export. Emitting past this drops, never blocks.
    pub queue_capacity: usize,
    /// Maximum records handed to the exporter in one call.
    pub batch_size: usize,
    /// Default deadline a host flush is allowed to take.
    pub flush_deadline: Duration,
}

impl Settings {
    /// Bounds suitable for a request-scaled Lambda: a small queue and a flush
    /// budget well inside a single invocation.
    #[must_use]
    pub const fn lambda() -> Self {
        Self {
            queue_capacity: 512,
            batch_size: 128,
            flush_deadline: Duration::from_millis(250),
        }
    }

    /// Bounds suitable for a long-lived process: a larger queue and a flush
    /// budget used only on drain.
    #[must_use]
    pub const fn long_lived() -> Self {
        Self {
            queue_capacity: 8192,
            batch_size: 512,
            flush_deadline: Duration::from_secs(2),
        }
    }

    /// Returns a copy with `queue_capacity` replaced.
    #[must_use]
    pub const fn with_queue_capacity(mut self, queue_capacity: usize) -> Self {
        self.queue_capacity = queue_capacity;
        self
    }

    /// Returns a copy with `batch_size` replaced.
    #[must_use]
    pub const fn with_batch_size(mut self, batch_size: usize) -> Self {
        self.batch_size = batch_size;
        self
    }

    /// Returns a copy with `flush_deadline` replaced.
    #[must_use]
    pub const fn with_flush_deadline(mut self, flush_deadline: Duration) -> Self {
        self.flush_deadline = flush_deadline;
        self
    }

    /// The effective batch size: at least one record, never more than the queue.
    #[must_use]
    pub const fn effective_batch_size(&self) -> usize {
        let batch = if self.batch_size == 0 {
            1
        } else {
            self.batch_size
        };
        if batch > self.queue_capacity && self.queue_capacity > 0 {
            self.queue_capacity
        } else {
            batch
        }
    }
}

impl Default for Settings {
    fn default() -> Self {
        Self::lambda()
    }
}

#[cfg(test)]
mod tests {
    use super::Settings;
    use std::time::Duration;

    #[test]
    fn default_is_the_lambda_profile() {
        assert_eq!(Settings::default(), Settings::lambda());
    }

    #[test]
    fn long_lived_holds_more_than_lambda() {
        assert!(Settings::long_lived().queue_capacity > Settings::lambda().queue_capacity);
        assert!(Settings::long_lived().flush_deadline > Settings::lambda().flush_deadline);
    }

    #[test]
    fn effective_batch_size_is_at_least_one() {
        assert_eq!(
            Settings::lambda().with_batch_size(0).effective_batch_size(),
            1
        );
    }

    #[test]
    fn effective_batch_size_never_exceeds_the_queue() {
        let settings = Settings::lambda()
            .with_queue_capacity(4)
            .with_batch_size(64);
        assert_eq!(settings.effective_batch_size(), 4);
    }

    #[test]
    fn builders_replace_only_the_named_field() {
        let settings = Settings::lambda().with_flush_deadline(Duration::from_millis(7));
        assert_eq!(settings.flush_deadline, Duration::from_millis(7));
        assert_eq!(settings.queue_capacity, Settings::lambda().queue_capacity);
        assert_eq!(settings.batch_size, Settings::lambda().batch_size);
    }
}
