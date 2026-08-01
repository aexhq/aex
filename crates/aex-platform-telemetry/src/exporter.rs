//! The export port and the in-memory adapter used by local tests.
//!
//! The host binary chooses the adapter: a long-lived process installs a bounded
//! batch exporter, a Lambda installs a per-invocation exporter with a flush
//! budget, a local test installs [`InMemoryExporter`], and an unset endpoint
//! installs nothing at all. No adapter may extend a customer deadline or change
//! a run or settlement result.

use parking_lot::Mutex;

use crate::record::Record;

/// Why an export attempt failed.
///
/// Every variant is a diagnostic loss, never a product failure. The facade
/// counts the loss and carries on.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ExportError {
    /// The destination refused or could not be reached.
    #[error("telemetry export was refused: {reason}")]
    Refused {
        /// Why the destination refused the batch.
        reason: String,
    },
    /// The export did not complete inside the host's budget.
    #[error("telemetry export exceeded its budget")]
    Budget,
}

/// The port a host adapter implements to receive batched diagnostics.
pub trait Exporter: Send + Sync {
    /// Delivers one batch.
    ///
    /// Implementations must return rather than block indefinitely: the facade
    /// calls this on the host's flush path, which is bounded by a deadline.
    ///
    /// # Errors
    ///
    /// Returns [`ExportError`] when the batch could not be delivered. The facade
    /// treats that as a diagnostic loss and increments its drop counter; it
    /// never retries and never propagates the failure to product code.
    fn export(&self, batch: &[Record]) -> Result<(), ExportError>;
}

/// An exporter that keeps every delivered batch in memory.
///
/// This is the sink local tests install. It is bounded only by the queue that
/// feeds it, so it is not a production adapter.
#[derive(Debug, Default)]
pub struct InMemoryExporter {
    delivered: Mutex<Vec<Record>>,
    calls: Mutex<usize>,
}

impl InMemoryExporter {
    /// Creates an empty sink.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Every record delivered so far, in delivery order.
    #[must_use]
    pub fn delivered(&self) -> Vec<Record> {
        self.delivered.lock().clone()
    }

    /// How many times [`Exporter::export`] has been called.
    #[must_use]
    pub fn calls(&self) -> usize {
        *self.calls.lock()
    }
}

impl Exporter for InMemoryExporter {
    fn export(&self, batch: &[Record]) -> Result<(), ExportError> {
        *self.calls.lock() += 1;
        self.delivered.lock().extend_from_slice(batch);
        Ok(())
    }
}

/// An exporter that always fails.
///
/// Hosts do not install this; it exists so the facade's "an exporter failure is
/// a counted diagnostic loss, never a product failure" contract has direct
/// evidence.
#[derive(Debug, Default)]
pub struct FailingExporter {
    calls: Mutex<usize>,
}

impl FailingExporter {
    /// Creates an exporter that refuses every batch.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// How many times [`Exporter::export`] has been called.
    #[must_use]
    pub fn calls(&self) -> usize {
        *self.calls.lock()
    }
}

impl Exporter for FailingExporter {
    fn export(&self, batch: &[Record]) -> Result<(), ExportError> {
        *self.calls.lock() += 1;
        Err(ExportError::Refused {
            reason: format!("rejected {} record(s)", batch.len()),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::{ExportError, Exporter, FailingExporter, InMemoryExporter};
    use crate::record::Record;

    #[test]
    fn in_memory_exporter_records_every_batch() {
        let exporter = InMemoryExporter::new();
        exporter
            .export(&[Record::event("aex.process.started")])
            .expect("in-memory export succeeds");
        exporter
            .export(&[Record::event("aex.process.started")])
            .expect("in-memory export succeeds");
        assert_eq!(exporter.calls(), 2);
        assert_eq!(exporter.delivered().len(), 2);
    }

    #[test]
    fn failing_exporter_reports_the_batch_size() {
        let exporter = FailingExporter::new();
        let error = exporter
            .export(&[Record::event("aex.process.started")])
            .expect_err("the failing exporter always fails");
        assert_eq!(
            error,
            ExportError::Refused {
                reason: "rejected 1 record(s)".to_owned()
            }
        );
        assert_eq!(exporter.calls(), 1);
    }
}
