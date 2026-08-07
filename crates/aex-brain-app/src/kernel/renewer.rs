//! `LeaseRenewer` handshake — the renewal/commit interlock (Loom target L2).
//!
//! The renewer runs as an independent supervised task holding no lock. Its only
//! communication with the committing thread is this shared state, and the one property
//! that must hold is: **a fence advance observed by the renewer is observed by the commit
//! before it writes.** Everything else here is bookkeeping.

use super::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use crate::ports::proof::CancelToken;
use aex_brain_domain::ids::Fence;

/// How many consecutive renewal failures cancel the activation.
///
/// Three rather than one: a single throttled write is not evidence the lease is gone, and
/// cancelling on it would abandon healthy work. Three consecutive failures across the
/// five-second interval span most of the fifteen-second TTL, so the activation gives up
/// while it still has time to settle rather than after it has already been fenced.
pub const RENEWAL_FAILURES_BEFORE_CANCEL: usize = 3;

/// Shared state between the renewer task and the committing thread.
#[derive(Debug)]
pub struct RenewalState {
    /// The highest fence any party has observed on the durable record.
    observed_fence: AtomicU64,
    /// The fence this activation holds.
    held_fence: u64,
    /// Consecutive renewal failures.
    failures: AtomicUsize,
    /// Set when the activation must stop.
    cancel: CancelToken,
}

/// What a renewal attempt concluded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RenewalOutcome {
    /// The lease was extended.
    Extended,
    /// The attempt failed but the activation continues.
    Failed {
        /// How many consecutive failures have now occurred.
        consecutive: usize,
    },
    /// The activation has lost the agent and must stop.
    ///
    /// Reached by three consecutive failures or by observing a fence advance. Either way
    /// the activation attempts one final commit, which fails `StaleFence` and is discarded
    /// with a recorded ambiguity diagnostic — it does not pretend the work succeeded.
    Lost,
}

impl RenewalState {
    /// State for an activation holding `fence`.
    #[must_use]
    pub fn new(fence: Fence, cancel: CancelToken) -> Self {
        Self {
            observed_fence: AtomicU64::new(fence.0),
            held_fence: fence.0,
            failures: AtomicUsize::new(0),
            cancel,
        }
    }

    /// Records a successful renewal that observed `current` on the durable record.
    #[must_use]
    pub fn renewed(&self, current: Fence) -> RenewalOutcome {
        self.failures.store(0, Ordering::SeqCst);
        self.observe(current)
    }

    /// Records a failed renewal attempt.
    #[must_use]
    pub fn renewal_failed(&self) -> RenewalOutcome {
        let consecutive = self.failures.fetch_add(1, Ordering::SeqCst) + 1;
        if consecutive >= RENEWAL_FAILURES_BEFORE_CANCEL {
            self.cancel.cancel();
            return RenewalOutcome::Lost;
        }
        RenewalOutcome::Failed { consecutive }
    }

    /// Records a fence reading from any source.
    ///
    /// The store is ordered before the cancel so that a thread which sees the cancel token
    /// set is guaranteed to see the advanced fence too. Reversing the two would let a
    /// commit observe cancellation, re-read the fence, still see its own, and conclude it
    /// was safe to write.
    #[must_use]
    pub fn observe(&self, current: Fence) -> RenewalOutcome {
        self.observed_fence.fetch_max(current.0, Ordering::SeqCst);
        if current.0 > self.held_fence {
            self.cancel.cancel();
            return RenewalOutcome::Lost;
        }
        RenewalOutcome::Extended
    }

    /// Whether this activation may still attempt a write.
    ///
    /// Advisory only. The durable precondition set is what actually rejects a stale owner;
    /// this just avoids paying for a write that is already known to fail.
    #[must_use]
    pub fn may_commit(&self) -> bool {
        !self.cancel.is_cancelled() && self.observed_fence.load(Ordering::SeqCst) <= self.held_fence
    }

    /// The fence this activation holds.
    #[must_use]
    pub const fn held(&self) -> Fence {
        Fence(self.held_fence)
    }

    /// The highest fence observed anywhere.
    #[must_use]
    pub fn observed(&self) -> Fence {
        Fence(self.observed_fence.load(Ordering::SeqCst))
    }

    /// The token that is set when the activation loses the agent.
    #[must_use]
    pub const fn cancel(&self) -> &CancelToken {
        &self.cancel
    }
}
