//! The allocation gate.
//!
//! A request reserves its worst-case decoded footprint **before the first decode
//! byte**. Reservation failure is an explicit retryable `503`, which is the
//! whole point: the failure mode this replaces is an unbounded allocation that
//! the process discovers as an OOM kill.
//!
//! The budget is a plain atomic counter rather than a `tokio` semaphore so the
//! decoder stays runtime-free; the 50 ms reservation *wait* is the service's
//! job, and `regional-otlp` loops `try_reserve` against its own deadline.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use crate::error::OtlpError;

/// A process-wide decode-memory budget.
#[derive(Clone, Debug)]
pub struct MemoryBudget {
    inner: Arc<BudgetInner>,
}

#[derive(Debug)]
struct BudgetInner {
    capacity: usize,
    reserved: AtomicUsize,
}

impl MemoryBudget {
    /// A budget of `capacity` bytes.
    #[must_use]
    pub fn new(capacity: usize) -> Self {
        Self {
            inner: Arc::new(BudgetInner {
                capacity,
                reserved: AtomicUsize::new(0),
            }),
        }
    }

    /// The budget derived from a Lambda memory setting.
    ///
    /// 55 % of the configured memory, computed at startup and logged, so the
    /// total regional decode footprint is `reserved concurrency × decoded_max`
    /// by construction rather than by hope.
    #[must_use]
    pub fn from_process_memory_bytes(process_bytes: usize) -> Self {
        Self::new(process_bytes / 100 * 55)
    }

    /// The configured capacity.
    #[must_use]
    pub fn capacity(&self) -> usize {
        self.inner.capacity
    }

    /// How many bytes are currently reserved.
    #[must_use]
    pub fn reserved(&self) -> usize {
        self.inner.reserved.load(Ordering::Acquire)
    }

    /// Reserves `bytes`, or reports that the budget cannot cover it right now.
    ///
    /// # Errors
    ///
    /// Returns [`OtlpError::MemoryUnavailable`] when the reservation would take
    /// the budget past its capacity. A request larger than the whole capacity
    /// can never succeed and fails on the first attempt rather than spinning.
    pub fn try_reserve(&self, bytes: usize) -> Result<MemoryLease, OtlpError> {
        let mut current = self.inner.reserved.load(Ordering::Acquire);
        loop {
            let Some(next) = current.checked_add(bytes) else {
                return Err(OtlpError::MemoryUnavailable { requested: bytes });
            };
            if next > self.inner.capacity {
                return Err(OtlpError::MemoryUnavailable { requested: bytes });
            }
            match self.inner.reserved.compare_exchange_weak(
                current,
                next,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => {
                    return Ok(MemoryLease {
                        budget: Arc::clone(&self.inner),
                        bytes,
                    });
                }
                Err(observed) => current = observed,
            }
        }
    }
}

/// Proof that a decode's worst-case footprint is reserved.
///
/// Released on `Drop`, including on an early return from a decode error, which
/// is what makes the leak test meaningful.
#[derive(Debug)]
pub struct MemoryLease {
    budget: Arc<BudgetInner>,
    bytes: usize,
}

impl MemoryLease {
    /// How many bytes this lease covers.
    #[must_use]
    pub const fn bytes(&self) -> usize {
        self.bytes
    }

    /// Whether this lease covers `needed` bytes.
    #[must_use]
    pub const fn covers(&self, needed: usize) -> bool {
        self.bytes >= needed
    }
}

impl Drop for MemoryLease {
    fn drop(&mut self) {
        self.budget.reserved.fetch_sub(self.bytes, Ordering::AcqRel);
    }
}

/// How many bytes one request must reserve before decoding.
///
/// `min(encoded × expansion_estimate, decoded_max)`: an identity-coded body
/// cannot expand, and a compressed one is bounded by the decoded ceiling
/// whatever its ratio claims.
#[must_use]
pub fn reservation_for(encoded_len: usize, expansion_estimate: usize, decoded_max: usize) -> usize {
    encoded_len
        .saturating_mul(expansion_estimate)
        .min(decoded_max)
        .max(1)
}

#[cfg(test)]
mod tests {
    use super::{MemoryBudget, reservation_for};

    #[test]
    fn a_lease_is_released_on_drop_even_on_an_early_return() {
        let budget = MemoryBudget::new(1_024);
        {
            let lease = budget.try_reserve(1_000).expect("fits");
            assert_eq!(budget.reserved(), 1_000);
            assert!(lease.covers(999));
            assert!(!lease.covers(1_001));
            assert!(budget.try_reserve(100).is_err(), "the budget is exhausted");
        }
        assert_eq!(budget.reserved(), 0, "the lease released on drop");
        budget
            .try_reserve(1_024)
            .expect("the whole budget is free again");
    }

    #[test]
    fn a_request_larger_than_the_budget_fails_immediately() {
        let budget = MemoryBudget::new(16);
        assert!(budget.try_reserve(17).is_err());
        assert_eq!(budget.reserved(), 0);
    }

    #[test]
    fn a_leak_storm_leaves_no_residual_permits() {
        let budget = MemoryBudget::new(4_096);
        for size in 1..512 {
            let outcome = budget.try_reserve(size);
            drop(outcome);
        }
        assert_eq!(budget.reserved(), 0);
    }

    #[test]
    fn the_reservation_is_bounded_by_the_decoded_ceiling() {
        assert_eq!(reservation_for(1_000, 1, 16_000), 1_000);
        assert_eq!(reservation_for(1_000, 200, 16_000), 16_000);
        assert_eq!(reservation_for(0, 200, 16_000), 1);
        assert_eq!(reservation_for(usize::MAX, 200, 16_000), 16_000);
    }

    #[test]
    fn the_process_budget_is_fifty_five_percent() {
        let budget = MemoryBudget::from_process_memory_bytes(3_008 * 1_024 * 1_024);
        assert_eq!(budget.capacity(), 3_008 * 1_024 * 1_024 / 100 * 55);
    }
}
