//! Graceful drain.
//!
//! `SIGTERM` starts it. Readiness fails immediately so the load balancer stops sending work;
//! liveness keeps passing, because failing liveness would have the orchestrator kill the
//! task along with the non-replayable effects it is trying to finish.
//!
//! The asymmetry between replay-safe and non-replayable work is the whole design. Work that
//! can be redone is abandoned at once and its lease released, so a surviving task picks it
//! up in milliseconds. Work that cannot — a dispatched provider request — runs until its own
//! deadline or until the commit margin, and anything that still cannot commit settles
//! `OutcomeUnknown` and terminalizes the run `interrupted`. That is honest; reporting a
//! clean cancellation for a request that may have been served is not.

use aex_brain_application::ports::ReleaseDisposition;

/// How long the orchestrator gives the task to stop.
pub const STOP_TIMEOUT: core::time::Duration = core::time::Duration::from_mins(2);

/// How much of that is reserved for committing what is already finished.
pub const COMMIT_MARGIN: core::time::Duration = core::time::Duration::from_secs(30);

/// Why graceful drain refused to claim a clean exit.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum DrainError {
    /// The receive supervisor panicked instead of completing or accepting cancellation.
    #[error("the wake pump failed while draining: {reason}")]
    PumpFailed {
        /// Bounded join failure description.
        reason: String,
    },
    /// The pump was joined, but an admitted activation remained registered.
    #[error("the wake pump stopped with {in_flight} admitted activation(s) still in flight")]
    ActivationsRemain {
        /// Count observed after the pump join completed.
        in_flight: usize,
    },
}

/// The stages drain moves through, in order.
///
/// Ordered and exhaustive so a shutdown that stalls can be reported as "stuck at stage N"
/// rather than as "did not exit".
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Stage {
    /// Readiness fails; liveness still passes.
    FailReadiness,
    /// The queue receive loop stops and unstarted local deliveries are released.
    StopReceiving,
    /// Replay-safe in-flight work is abandoned and its leases released.
    AbandonReplaySafe,
    /// Dispatched non-replayable effects run to their deadline or the commit margin.
    AwaitNonReplayable,
    /// No activation future remains able to use a lease; cooperative paths released theirs,
    /// while an explicitly aborted path is fenced by process exit and its bounded TTL.
    ReleaseLeases,
    /// Caches are dropped and telemetry flushed.
    Flush,
    /// The process exits zero.
    Exit,
}

impl Stage {
    /// Every stage, in the order drain performs them.
    pub const ORDER: [Self; 7] = [
        Self::FailReadiness,
        Self::StopReceiving,
        Self::AbandonReplaySafe,
        Self::AwaitNonReplayable,
        Self::ReleaseLeases,
        Self::Flush,
        Self::Exit,
    ];

    /// The stage after this one.
    #[must_use]
    pub const fn next(self) -> Option<Self> {
        match self {
            Self::FailReadiness => Some(Self::StopReceiving),
            Self::StopReceiving => Some(Self::AbandonReplaySafe),
            Self::AbandonReplaySafe => Some(Self::AwaitNonReplayable),
            Self::AwaitNonReplayable => Some(Self::ReleaseLeases),
            Self::ReleaseLeases => Some(Self::Flush),
            Self::Flush => Some(Self::Exit),
            Self::Exit => None,
        }
    }
}

/// How long a dispatched non-replayable effect may still run.
///
/// The commit margin is subtracted because finishing the request and then failing to record
/// the outcome is the worst of both: the provider was paid and the run is interrupted
/// anyway.
#[must_use]
pub fn non_replayable_budget(
    elapsed: core::time::Duration,
    effect_deadline: core::time::Duration,
) -> core::time::Duration {
    let remaining = STOP_TIMEOUT
        .saturating_sub(elapsed)
        .saturating_sub(COMMIT_MARGIN);
    effect_deadline.min(remaining)
}

/// What one in-flight unit of work does when drain starts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Disposition {
    /// Abandon now and release the lease. A surviving task claims it immediately.
    Abandon,
    /// Run to `budget`, then settle from durable evidence.
    Finish {
        /// How long it may still run.
        budget: core::time::Duration,
    },
}

/// What `replayable` work with `effect_deadline` left should do.
#[must_use]
pub fn disposition(
    replayable: bool,
    elapsed: core::time::Duration,
    effect_deadline: core::time::Duration,
) -> Disposition {
    if replayable {
        return Disposition::Abandon;
    }
    let budget = non_replayable_budget(elapsed, effect_deadline);
    if budget.is_zero() {
        // Out of time. The effect settles `OutcomeUnknown` and the run terminalizes
        // `interrupted`, which is honest; a clean cancellation would not be.
        return Disposition::Abandon;
    }
    Disposition::Finish { budget }
}

/// The disposition every released lease carries during drain.
///
/// [`ReleaseDisposition::Drain`] removes the durable owner and expiry under their exact
/// fence, so a surviving task claims immediately rather than waiting out the whole
/// 15-second TTL. Over a fleet-wide roll that difference is the whole recovery time.
#[must_use]
pub const fn release_disposition() -> ReleaseDisposition {
    ReleaseDisposition::Drain
}

#[cfg(test)]
mod tests {
    use super::{
        COMMIT_MARGIN, Disposition, STOP_TIMEOUT, Stage, disposition, non_replayable_budget,
        release_disposition,
    };
    use aex_brain_application::ports::ReleaseDisposition;

    const fn secs(value: u64) -> core::time::Duration {
        core::time::Duration::from_secs(value)
    }

    /// The stages are ordered and exhaustive, so a stalled shutdown is reportable as a
    /// stage rather than as a silence.
    #[test]
    fn the_stages_form_one_total_order_ending_in_exit() {
        let mut walked = vec![Stage::FailReadiness];
        while let Some(next) = walked.last().and_then(|stage| stage.next()) {
            walked.push(next);
        }
        assert_eq!(walked, Stage::ORDER.to_vec());
        assert_eq!(Stage::Exit.next(), None);
        for pair in Stage::ORDER.windows(2) {
            assert!(pair[0] < pair[1], "{pair:?}");
        }
    }

    /// Readiness fails first and liveness is never touched: failing liveness gets a task
    /// killed along with the effects it is trying to finish.
    #[test]
    fn readiness_fails_before_anything_else_happens() {
        assert_eq!(Stage::ORDER[0], Stage::FailReadiness);
        assert_eq!(Stage::ORDER[1], Stage::StopReceiving);
    }

    /// Replay-safe work is abandoned at once: a surviving task redoes it in milliseconds,
    /// and holding it only delays the shutdown.
    #[test]
    fn replay_safe_work_is_abandoned_immediately() {
        assert_eq!(disposition(true, secs(0), secs(60)), Disposition::Abandon);
    }

    /// A dispatched non-replayable effect runs on, but never into the commit margin.
    #[test]
    fn a_non_replayable_effect_keeps_the_commit_margin_clear() {
        let Disposition::Finish { budget } = disposition(false, secs(0), secs(600)) else {
            panic!("a fresh drain leaves time");
        };
        assert_eq!(budget, STOP_TIMEOUT.saturating_sub(COMMIT_MARGIN));

        let Disposition::Finish { budget } = disposition(false, secs(0), secs(5)) else {
            panic!("a short deadline still finishes");
        };
        assert_eq!(budget, secs(5), "the effect's own deadline binds first");
    }

    /// Out of time, the effect settles from durable evidence rather than being reported as
    /// a clean cancellation.
    #[test]
    fn an_effect_with_no_time_left_is_abandoned_to_settle_honestly() {
        assert_eq!(
            disposition(false, STOP_TIMEOUT.saturating_sub(COMMIT_MARGIN), secs(600)),
            Disposition::Abandon
        );
        assert_eq!(
            non_replayable_budget(STOP_TIMEOUT, secs(600)),
            core::time::Duration::ZERO
        );
    }

    /// Drain expires the lease immediately. Over a fleet-wide roll, waiting out the TTL
    /// instead is the whole recovery time.
    #[test]
    fn every_drained_lease_is_released_so_a_survivor_claims_at_once() {
        assert_eq!(release_disposition(), ReleaseDisposition::Drain);
    }

    #[test]
    fn the_margins_leave_a_usable_window() {
        assert!(COMMIT_MARGIN < STOP_TIMEOUT);
        assert_eq!(STOP_TIMEOUT.as_secs(), 120);
        assert_eq!(COMMIT_MARGIN.as_secs(), 30);
    }
}
