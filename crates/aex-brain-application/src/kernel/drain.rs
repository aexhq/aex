//! `DrainGate` — one-way admission stop (Loom target L6).
//!
//! Once any thread observes drain, no thread admits new work. In-flight guards still
//! complete, because abandoning a dispatched non-replayable effect to shut down faster
//! would turn a graceful stop into an interrupted run.

use super::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

/// A one-way admission gate.
#[derive(Debug, Default)]
pub struct DrainGate {
    draining: AtomicBool,
    in_flight: AtomicUsize,
}

/// Permission to run one admitted unit of work. Decrements the in-flight count on drop.
#[derive(Debug)]
#[must_use = "dropping the permit immediately marks the work complete"]
pub struct DrainPermit<'gate> {
    gate: &'gate DrainGate,
}

impl DrainGate {
    /// A gate that is not draining.
    #[must_use]
    pub fn new() -> Self {
        Self {
            draining: AtomicBool::new(false),
            in_flight: AtomicUsize::new(0),
        }
    }

    /// Starts draining. Idempotent.
    pub fn start_drain(&self) {
        self.draining.store(true, Ordering::SeqCst);
    }

    /// Whether drain has started.
    #[must_use]
    pub fn is_draining(&self) -> bool {
        self.draining.load(Ordering::SeqCst)
    }

    /// Admits one unit of work, unless drain has started.
    ///
    /// The order matters: the in-flight count is incremented first and the flag is
    /// re-checked afterwards, so a permit issued concurrently with `start_drain` is either
    /// visible to the drainer or never issued. Checking first and incrementing second would
    /// leave a window where drain sees zero in flight while a permit is being handed out.
    #[must_use]
    pub fn try_admit(&self) -> Option<DrainPermit<'_>> {
        if self.is_draining() {
            return None;
        }
        self.in_flight.fetch_add(1, Ordering::SeqCst);
        if self.is_draining() {
            self.in_flight.fetch_sub(1, Ordering::SeqCst);
            return None;
        }
        Some(DrainPermit { gate: self })
    }

    /// How many admitted units are still running.
    #[must_use]
    pub fn in_flight(&self) -> usize {
        self.in_flight.load(Ordering::SeqCst)
    }

    /// Whether the process may exit: draining and nothing left in flight.
    #[must_use]
    pub fn is_quiesced(&self) -> bool {
        self.is_draining() && self.in_flight() == 0
    }
}

impl Drop for DrainPermit<'_> {
    fn drop(&mut self) {
        self.gate.in_flight.fetch_sub(1, Ordering::SeqCst);
    }
}
