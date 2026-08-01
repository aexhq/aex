//! The bounded, non-blocking facade every deployable installs.
//!
//! Three properties are load-bearing and are each covered by a test:
//!
//! 1. [`Handle::emit`] never blocks and never panics. A full queue drops the
//!    newest record and increments a counter.
//! 2. An absent exporter and a failing exporter are both no-ops with counters.
//!    Neither can fail a product operation.
//! 3. [`Handle::flush`] stops at its deadline and reports what it left behind.
//!    No flush may extend a customer deadline.

use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use parking_lot::Mutex;

use crate::exporter::Exporter;
use crate::record::{Attribute, Record};
use crate::settings::Settings;

/// How a [`Handle::flush`] ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FlushOutcome {
    /// No exporter is installed, so there was nothing to flush.
    NoExporter,
    /// The queue was emptied inside the deadline.
    Drained {
        /// Records the exporter accepted during this flush.
        exported: usize,
    },
    /// The deadline arrived first.
    DeadlineExceeded {
        /// Records still queued when the flush stopped.
        pending: usize,
    },
}

#[derive(Debug, Default)]
struct Counters {
    dropped: AtomicU64,
    exported: AtomicU64,
    redacted: AtomicU64,
    deadline_exceeded: AtomicU64,
}

struct Inner {
    settings: Settings,
    exporter: Option<Arc<dyn Exporter>>,
    queue: Mutex<VecDeque<Record>>,
    counters: Counters,
}

/// The handle a deployable installs once and shares for the life of the process.
///
/// Cloning is cheap and every clone shares one queue and one counter set.
#[derive(Clone)]
pub struct Handle {
    inner: Arc<Inner>,
}

impl std::fmt::Debug for Handle {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Handle")
            .field("settings", &self.inner.settings)
            .field("exporter_installed", &self.inner.exporter.is_some())
            .field("pending", &self.pending())
            .field("dropped", &self.dropped())
            .field("exported", &self.exported())
            .field("redacted_attributes", &self.redacted_attributes())
            .finish()
    }
}

impl Handle {
    /// Installs the facade with `settings` and an optional host exporter.
    ///
    /// Passing `None` installs a no-op sink. That is the correct configuration
    /// for an unset endpoint or a disabled plane: diagnostics are discarded and
    /// counted, and nothing about product behaviour changes.
    #[must_use]
    pub fn install(settings: &Settings, exporter: Option<Arc<dyn Exporter>>) -> Self {
        Self {
            inner: Arc::new(Inner {
                settings: *settings,
                exporter,
                queue: Mutex::new(VecDeque::with_capacity(settings.queue_capacity.min(1024))),
                counters: Counters::default(),
            }),
        }
    }

    /// Records one diagnostic.
    ///
    /// Never blocks, never allocates past the configured queue capacity, and
    /// never panics. Attributes whose registry visibility class is not public
    /// are removed before the record is queued, so a non-public value cannot
    /// reach an exporter even if a caller supplies one.
    pub fn emit(&self, record: Record) {
        if self.inner.exporter.is_none() {
            self.inner.counters.dropped.fetch_add(1, Ordering::Relaxed);
            return;
        }
        let record = self.redact(record);
        let mut queue = self.inner.queue.lock();
        if queue.len() >= self.inner.settings.queue_capacity {
            drop(queue);
            self.inner.counters.dropped.fetch_add(1, Ordering::Relaxed);
            return;
        }
        queue.push_back(record);
    }

    /// Delivers queued diagnostics, stopping at `deadline`.
    ///
    /// An exporter failure is counted as a drop and the flush continues; the
    /// failure is never propagated, because a diagnostic loss must not become a
    /// product failure.
    #[must_use]
    pub fn flush(&self, deadline: Duration) -> FlushOutcome {
        let Some(exporter) = self.inner.exporter.as_ref() else {
            return FlushOutcome::NoExporter;
        };
        let started = Instant::now();
        let batch_size = self.inner.settings.effective_batch_size();
        let mut accepted = 0_usize;
        loop {
            let batch = self.take(batch_size);
            if batch.is_empty() {
                return FlushOutcome::Drained { exported: accepted };
            }
            if started.elapsed() >= deadline {
                let mut queue = self.inner.queue.lock();
                for record in batch.into_iter().rev() {
                    queue.push_front(record);
                }
                let pending = queue.len();
                drop(queue);
                self.inner
                    .counters
                    .deadline_exceeded
                    .fetch_add(1, Ordering::Relaxed);
                return FlushOutcome::DeadlineExceeded { pending };
            }
            let delivered = batch.len();
            match exporter.export(&batch) {
                Ok(()) => {
                    accepted += delivered;
                    self.inner
                        .counters
                        .exported
                        .fetch_add(as_u64(delivered), Ordering::Relaxed);
                }
                Err(_) => {
                    self.inner
                        .counters
                        .dropped
                        .fetch_add(as_u64(delivered), Ordering::Relaxed);
                }
            }
        }
    }

    /// Records currently queued for export.
    #[must_use]
    pub fn pending(&self) -> usize {
        self.inner.queue.lock().len()
    }

    /// Records discarded because the queue was full, the exporter failed, or no
    /// exporter was installed.
    #[must_use]
    pub fn dropped(&self) -> u64 {
        self.inner.counters.dropped.load(Ordering::Relaxed)
    }

    /// Records the exporter accepted.
    #[must_use]
    pub fn exported(&self) -> u64 {
        self.inner.counters.exported.load(Ordering::Relaxed)
    }

    /// Attributes removed because their registry visibility class is not public.
    #[must_use]
    pub fn redacted_attributes(&self) -> u64 {
        self.inner.counters.redacted.load(Ordering::Relaxed)
    }

    /// Flushes that stopped at their deadline with records still pending.
    #[must_use]
    pub fn deadline_exceeded(&self) -> u64 {
        self.inner
            .counters
            .deadline_exceeded
            .load(Ordering::Relaxed)
    }

    /// Whether a host exporter is installed.
    #[must_use]
    pub fn has_exporter(&self) -> bool {
        self.inner.exporter.is_some()
    }

    fn redact(&self, mut record: Record) -> Record {
        let before = record.attributes.len();
        record
            .attributes
            .retain(|attribute: &Attribute| aex_telemetry_schema::is_exportable(attribute.key));
        let removed = before - record.attributes.len();
        if removed > 0 {
            self.inner
                .counters
                .redacted
                .fetch_add(as_u64(removed), Ordering::Relaxed);
        }
        record
    }

    fn take(&self, count: usize) -> Vec<Record> {
        let mut queue = self.inner.queue.lock();
        let taken = count.min(queue.len());
        queue.drain(..taken).collect()
    }
}

/// Saturating conversion so a counter update can never panic.
fn as_u64(value: usize) -> u64 {
    u64::try_from(value).unwrap_or(u64::MAX)
}
