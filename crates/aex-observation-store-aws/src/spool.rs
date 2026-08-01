//! The durable spool, the usage outbox and the regional ingress gate.
//!
//! A spool chunk is what makes an unfinished step 9 *visible and gate-closing*
//! rather than silent. Its `pending` set is a subset of `{outbox, index, wake}`,
//! and it is deleted only when the set is empty. A chunk that exhausts its
//! attempts escalates into an explicit `pipeline_loss` gap over its exact
//! accepted and time ranges; it is never dropped.

use aex_observation_domain::limits;
use aex_wire::types::Timestamp;

/// One outstanding duty on a spool chunk.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum Pending {
    /// The storage usage fact has not been confirmed delivered.
    Outbox,
    /// Not every `acceptedSeq` in the range is present as an `OBS#` item.
    Index,
    /// `regional-stream` has not been woken.
    ///
    /// A wake is a hint, so its only failure mode is latency.
    Wake,
}

impl Pending {
    /// Every duty, in declared order.
    pub const ALL: &'static [Pending] = &[Pending::Outbox, Pending::Index, Pending::Wake];

    /// The durable spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Outbox => "outbox",
            Self::Index => "index",
            Self::Wake => "wake",
        }
    }
}

/// One unacknowledged spool chunk.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SpoolChunk {
    /// The first accepted ordinal the chunk published.
    pub accepted_lo: u64,
    /// The last accepted ordinal the chunk published.
    pub accepted_hi: u64,
    /// When the chunk was written.
    pub accepted_at: Timestamp,
    /// What is still outstanding.
    pub pending: std::collections::BTreeSet<Pending>,
    /// How many times the reconciler has tried.
    pub attempts: u32,
}

impl SpoolChunk {
    /// A chunk with every duty outstanding.
    #[must_use]
    pub fn new(accepted_lo: u64, accepted_hi: u64, accepted_at: Timestamp) -> Self {
        Self {
            accepted_lo,
            accepted_hi,
            accepted_at,
            pending: Pending::ALL.iter().copied().collect(),
            attempts: 0,
        }
    }

    /// Clears one duty.
    pub fn acknowledge(&mut self, duty: Pending) {
        self.pending.remove(&duty);
    }

    /// Whether the chunk and its staged pages may be collected.
    #[must_use]
    pub fn is_settled(&self) -> bool {
        self.pending.is_empty()
    }

    /// Whether the chunk has exhausted its attempts and must escalate to a
    /// `pipeline_loss` gap.
    #[must_use]
    pub const fn must_escalate(&self) -> bool {
        self.attempts >= limits::OBS_SPOOL_MAX_ATTEMPTS
    }

    /// The backoff before the next attempt, capped at fifteen minutes.
    #[must_use]
    pub const fn next_attempt_delay_ms(&self) -> i64 {
        const CEILING_MS: i64 = 15 * 60 * 1_000;
        let shift = if self.attempts > 20 {
            20
        } else {
            self.attempts
        };
        let raw = 1_000i64.saturating_mul(1i64 << shift);
        if raw > CEILING_MS { CEILING_MS } else { raw }
    }
}

/// The regional ingress gate.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum GateState {
    /// Admission proceeds normally.
    Open,
    /// Admission proceeds and a counter is emitted.
    Degraded,
    /// Admission is refused with a retryable `503`.
    Closed,
}

impl GateState {
    /// Every state, in declared order.
    pub const ALL: &'static [GateState] =
        &[GateState::Open, GateState::Degraded, GateState::Closed];

    /// The durable spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::Degraded => "degraded",
            Self::Closed => "closed",
        }
    }

    /// Whether customer OTLP admission is refused.
    ///
    /// The gate never blocks a semantic run: a semantic producer that cannot
    /// admit records an explicit gap and completes, which is why a telemetry
    /// outage cannot fail a run.
    #[must_use]
    pub const fn refuses_customer_admission(self) -> bool {
        matches!(self, Self::Closed)
    }
}

/// What the gate evaluator measured.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct GateEvidence {
    /// Consecutive durable-commit failures.
    pub consecutive_failures: u32,
    /// How long those failures have been running.
    pub failure_span_ms: i64,
    /// The oldest unacknowledged chunk's age.
    pub oldest_spool_age_ms: i64,
    /// How many chunks are unacknowledged.
    pub spool_depth: u64,
    /// The configured spool capacity.
    pub spool_capacity: u64,
    /// How long index verification has been failing.
    pub index_failing_ms: i64,
    /// How long the gate has been in its current state.
    pub dwell_ms: i64,
}

/// Evaluates the next gate state.
///
/// Recovery is one-way through `degraded` and only after the hysteresis dwell,
/// so a flapping dependency cannot oscillate the gate.
#[must_use]
pub fn evaluate(current: GateState, evidence: &GateEvidence) -> GateState {
    let should_close = evidence.oldest_spool_age_ms >= limits::OBS_SPOOL_MAX_AGE_MS
        || (evidence.spool_capacity > 0
            && evidence.spool_depth * 100 >= evidence.spool_capacity * 75)
        || evidence.index_failing_ms >= limits::OBS_INDEX_FAIL_WINDOW_MS;
    let should_degrade = evidence.consecutive_failures >= 2 && evidence.failure_span_ms >= 30_000;
    let dwelt = evidence.dwell_ms >= limits::OBS_GATE_HYSTERESIS_MS;

    match current {
        GateState::Open => {
            if should_close {
                GateState::Closed
            } else if should_degrade {
                GateState::Degraded
            } else {
                GateState::Open
            }
        }
        GateState::Degraded => {
            if should_close {
                GateState::Closed
            } else if should_degrade || !dwelt {
                GateState::Degraded
            } else {
                GateState::Open
            }
        }
        GateState::Closed => {
            if should_close || !dwelt {
                GateState::Closed
            } else {
                GateState::Degraded
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{GateEvidence, GateState, Pending, SpoolChunk, evaluate};
    use aex_observation_domain::limits;
    use aex_wire::types::Timestamp;

    fn instant(millis: i64) -> Timestamp {
        Timestamp::from_unix_millis(millis).expect("representable")
    }

    #[test]
    fn a_chunk_settles_only_when_every_duty_is_acknowledged() {
        let mut chunk = SpoolChunk::new(0, 9, instant(0));
        assert_eq!(chunk.pending.len(), 3);
        for duty in Pending::ALL {
            assert!(!chunk.is_settled());
            chunk.acknowledge(*duty);
        }
        assert!(chunk.is_settled());
        assert_eq!(Pending::Index.as_str(), "index");
        assert_eq!(chunk.accepted_lo, 0);
        assert_eq!(chunk.accepted_hi, 9);
        assert_eq!(chunk.accepted_at, instant(0));
    }

    #[test]
    fn escalation_happens_at_the_declared_attempt_ceiling() {
        let mut chunk = SpoolChunk::new(0, 0, instant(0));
        chunk.attempts = limits::OBS_SPOOL_MAX_ATTEMPTS - 1;
        assert!(!chunk.must_escalate());
        chunk.attempts += 1;
        assert!(chunk.must_escalate());
    }

    #[test]
    fn the_backoff_is_exponential_and_capped() {
        let mut chunk = SpoolChunk::new(0, 0, instant(0));
        assert_eq!(chunk.next_attempt_delay_ms(), 1_000);
        chunk.attempts = 3;
        assert_eq!(chunk.next_attempt_delay_ms(), 8_000);
        chunk.attempts = 30;
        assert_eq!(chunk.next_attempt_delay_ms(), 15 * 60 * 1_000);
    }

    #[test]
    fn the_gate_closes_on_spool_age_depth_or_index_failure() {
        for evidence in [
            GateEvidence {
                oldest_spool_age_ms: limits::OBS_SPOOL_MAX_AGE_MS,
                ..GateEvidence::default()
            },
            GateEvidence {
                spool_depth: 75,
                spool_capacity: 100,
                ..GateEvidence::default()
            },
            GateEvidence {
                index_failing_ms: limits::OBS_INDEX_FAIL_WINDOW_MS,
                ..GateEvidence::default()
            },
        ] {
            assert_eq!(evaluate(GateState::Open, &evidence), GateState::Closed);
        }
        assert!(GateState::Closed.refuses_customer_admission());
        assert!(!GateState::Degraded.refuses_customer_admission());
        assert_eq!(GateState::ALL.len(), 3);
        assert_eq!(GateState::Degraded.as_str(), "degraded");
    }

    #[test]
    fn recovery_is_one_way_through_degraded_and_respects_the_dwell() {
        let healthy = GateEvidence {
            dwell_ms: limits::OBS_GATE_HYSTERESIS_MS,
            ..GateEvidence::default()
        };
        assert_eq!(evaluate(GateState::Closed, &healthy), GateState::Degraded);
        assert_eq!(evaluate(GateState::Degraded, &healthy), GateState::Open);

        let impatient = GateEvidence {
            dwell_ms: limits::OBS_GATE_HYSTERESIS_MS - 1,
            ..GateEvidence::default()
        };
        assert_eq!(
            evaluate(GateState::Closed, &impatient),
            GateState::Closed,
            "a closed gate never reopens inside the hysteresis dwell"
        );
        assert_eq!(
            evaluate(GateState::Degraded, &impatient),
            GateState::Degraded
        );
    }

    #[test]
    fn two_failures_spanning_thirty_seconds_degrade_an_open_gate() {
        let evidence = GateEvidence {
            consecutive_failures: 2,
            failure_span_ms: 30_000,
            ..GateEvidence::default()
        };
        assert_eq!(evaluate(GateState::Open, &evidence), GateState::Degraded);

        let brief = GateEvidence {
            consecutive_failures: 2,
            failure_span_ms: 29_999,
            ..GateEvidence::default()
        };
        assert_eq!(evaluate(GateState::Open, &brief), GateState::Open);
    }
}
