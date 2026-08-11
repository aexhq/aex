//! The production runtime-idle reader.
//!
//! `aex_session_app::ports::ContinuityReader` had exactly one implementation in
//! the tree — `ScriptedPorts` — and every deployable that mounted a route
//! needing it had to refuse instead. Two independent reviews each correctly
//! declined to write a local stand-in, because a locally computed `TrueIdle`
//! would mean two authorities answering "is this session idle" differently, and
//! neither noticed the other had declined. This is the adapter that closes it,
//! and it closes it the way the refusals asked for: **it computes nothing.**
//!
//! `aex-runtime-control` owns the predicate. `IdleAssessment::from_head` builds
//! the evidence, `is_true_idle` applies the 180-second window, and
//! `GenerationState` says whether a generation still holds continuity. This
//! module reads two rows, hands them to those functions, and translates the
//! verdict into the vocabulary `aex-secret-domain` publishes. Every place a
//! translation could have invented a value it refuses instead.
//!
//! Reads are eventually consistent, on purpose (D-11): a rebind re-asserts the
//! custody revision, the session revision and every secret's revocation epoch
//! as transaction conditions, so a stale idle read costs one condition failure
//! and can never commit a wrong effect. The one thing it must not do is claim a
//! busy session is idle, and it cannot: `open_operations` over-counts and never
//! under-counts, so a stale read errs toward "busy".

use aex_runtime_control::generation::GenerationState;
use aex_runtime_control::idle::IdleAssessment;
use aex_runtime_control::store::{
    GenerationView, ReadConsistency, RuntimeActivityStore, RuntimeStoreError,
};
use aex_secret_domain::{TrueIdle, TrueIdleViolation};
use aex_session_app::ports::{ContinuityReader, PortError};
use aex_wire::ids::SessionId;
use aex_wire::types::Timestamp;

use crate::store::RuntimeActivityDynamoStore;

/// Reads runtime continuity and idleness for one request.
///
/// The instant is the edge's receipt time rather than a clock read inside the
/// port, for the same reason `RequestClock` exists: the idle window is measured
/// against the instant the command was built against, so a second clock could
/// let the operation the caller is shown disagree with the evidence the plan
/// was conditioned on.
#[derive(Debug, Clone)]
pub struct RuntimeContinuity {
    activity: RuntimeActivityDynamoStore,
    now: Timestamp,
}

impl RuntimeContinuity {
    /// Binds the reader to one verified request.
    #[must_use]
    pub const fn new(activity: RuntimeActivityDynamoStore, now: Timestamp) -> Self {
        Self { activity, now }
    }

    /// The generation in force for a session, with its head, when there is one.
    ///
    /// Two rows, both eventually consistent: the session's current-generation
    /// pointer and the head it names. A pointer whose head is absent is a
    /// corrupt authority, not an idle session — answering "idle" there would
    /// let a rebind rewrite custody underneath a running guest.
    async fn current(&self, session: SessionId) -> Result<Option<GenerationView>, PortError> {
        let Some(pointer) = self
            .activity
            .load_current_generation(session)
            .await
            .map_err(|error| port_error(&error))?
        else {
            return Ok(None);
        };
        let view = self
            .activity
            // Eventually consistent by policy (D-11). Every value this read
            // produces is re-asserted as a transaction condition, and the head
            // over-counts open work, so a stale read errs toward "busy".
            .load_generation_view(pointer.generation, ReadConsistency::Eventual)
            .await
            .map_err(|error| port_error(&error))?
            .ok_or(PortError::Corrupt {
                kind: "workspace continuity",
                reason: "the session points at a generation whose head does not exist",
            })?;
        Ok(Some(view))
    }
}

/// The one violation an assessment can prove, or `None`.
///
/// The order matters and is the runtime authority's own: a generation is busy
/// before it is merely warm, so outstanding work is reported ahead of a
/// keepalive lease. `ConnectionOpen` is deliberately unreachable from this
/// backing — nothing on the generation head records open guest connections —
/// and it is left unreachable rather than approximated by a nearby field.
fn violation_of(assessment: &IdleAssessment, now: Timestamp) -> Option<TrueIdleViolation> {
    if assessment.is_true_idle(now) {
        return None;
    }
    if assessment.busy_count() > 0 {
        // The head folds Brain's admitted and queued counts into
        // `open_operations` on admission, so from this row the two are one
        // fact. Reporting the settled count as `WorkAdmitted` is the honest
        // half: it is work AEX admitted and has not seen settle.
        return Some(TrueIdleViolation::WorkAdmitted);
    }
    if assessment
        .evidence
        .keepalive_lease
        .as_ref()
        .is_some_and(|lease| lease.expires_at > now)
    {
        return Some(TrueIdleViolation::KeepaliveHeld);
    }
    // Quiescent, unleased, and still inside the 180-second window. Nothing
    // holds the generation; it is cooling down.
    Some(TrueIdleViolation::RecentlyBusy)
}

#[async_trait::async_trait]
impl ContinuityReader for RuntimeContinuity {
    async fn true_idle(&self, session: SessionId) -> Result<TrueIdle, PortError> {
        let Some(view) = self.current(session).await? else {
            // A session with no generation has no guest to disturb, so it is
            // idle by construction. This is the common case for a rebind: the
            // caller stops the session, its generation is reaped, and the
            // credentials are swapped before the next run.
            return Ok(TrueIdle::idle(self.now));
        };
        if !holds_continuity(view.head.state) {
            return Ok(TrueIdle::idle(self.now));
        }
        let assessment = IdleAssessment::from_head(&view.head, self.now);
        Ok(TrueIdle {
            observed_at: self.now,
            violation: violation_of(&assessment, self.now),
        })
    }
}

/// Maps a runtime-activity failure onto the port vocabulary.
///
/// A decode failure is `Corrupt` and everything else is `Unavailable`. Neither
/// is ever flattened into "no generation": answering "no generation" for a row
/// this adapter merely failed to read would report a live guest as idle, which
/// is the one lie this port exists to prevent.
fn port_error(error: &RuntimeStoreError) -> PortError {
    const KIND: &str = "workspace continuity";
    match error {
        RuntimeStoreError::Malformed { .. } => PortError::Corrupt {
            kind: KIND,
            reason: "the stored runtime activity does not decode into the lifecycle vocabulary",
        },
        _ => PortError::Unavailable { kind: KIND },
    }
}

/// The lifecycle states this adapter treats as still holding continuity.
///
/// Exposed so a caller that wants to explain a verdict does not re-derive the
/// rule from `GenerationState` and get a different answer.
#[must_use]
pub const fn holds_continuity(state: GenerationState) -> bool {
    !state.is_terminal()
}

#[cfg(test)]
mod tests {
    use aex_hands_protocol::lifecycle::KeepaliveLease;
    use aex_secret_domain::TrueIdleViolation;
    use aex_wire::types::Timestamp;

    use super::violation_of;
    use aex_runtime_control::idle::{IdleAssessment, TRUE_IDLE_THRESHOLD_MS};

    fn moment(millis: i64) -> Timestamp {
        Timestamp::from_unix_millis(millis).expect("in range")
    }

    fn assessment(open: u32, last_busy_ms: i64, lease: Option<KeepaliveLease>) -> IdleAssessment {
        IdleAssessment {
            evidence: aex_hands_protocol::lifecycle::TrueIdleEvidence {
                generation: aex_wire::ids::PrefixedId::from_uuid7(aex_wire::ids::Uuid7::compose(
                    1, [3; 10],
                )),
                activity_revision: 4,
                admitted: 0,
                queued: 0,
                open,
                keepalive_lease: lease,
                observed_at: moment(1_000_000),
            },
            last_busy_at: moment(last_busy_ms),
        }
    }

    #[test]
    fn outstanding_work_is_never_reported_as_idle() {
        let now = moment(10_000_000);
        assert_eq!(
            violation_of(&assessment(1, 0, None), now),
            Some(TrueIdleViolation::WorkAdmitted),
            "an over-counted open operation must keep a session out of a rebind"
        );
    }

    #[test]
    fn a_quiet_generation_past_the_window_is_idle() {
        let last_busy = 1_000_000;
        let now = moment(last_busy + i64::try_from(TRUE_IDLE_THRESHOLD_MS).expect("in range"));
        assert_eq!(violation_of(&assessment(0, last_busy, None), now), None);
    }

    #[test]
    fn one_millisecond_short_of_the_window_is_not_idle() {
        let last_busy = 1_000_000;
        let now = moment(last_busy + i64::try_from(TRUE_IDLE_THRESHOLD_MS).expect("in range") - 1);
        assert_eq!(
            violation_of(&assessment(0, last_busy, None), now),
            Some(TrueIdleViolation::RecentlyBusy),
            "the window boundary belongs to `aex-runtime-control` and is not softened here"
        );
    }

    #[test]
    fn a_paid_keepalive_lease_is_reported_as_itself() {
        let now = moment(1_000_000);
        let lease = KeepaliveLease {
            expires_at: moment(2_000_000),
            ..sample_lease()
        };
        assert_eq!(
            violation_of(&assessment(0, 0, Some(lease)), now),
            Some(TrueIdleViolation::KeepaliveHeld),
            "a warm generation the customer is paying for is not idle, and says why"
        );
    }

    fn sample_lease() -> KeepaliveLease {
        KeepaliveLease {
            lease_id: "lease-1".to_owned(),
            expires_at: moment(0),
        }
    }
}
