//! The accepted sequence and the four-stage frontier.
//!
//! `settled <= published <= projected <= accepted`, always. Every stage advances
//! only contiguously, so a gap can never be stepped over — a stalled frontier is
//! the honest signal that the customer's coverage vector copies.

use std::fmt;

use serde::{Deserialize, Serialize};

use crate::meter::Category;
use crate::wire_pending::{RegionId, Timestamp, WorkspaceId};

/// A one-based, contiguous position in one `(region, workspace, category)`
/// sequence.
///
/// Zero is not a position; it is the "nothing admitted yet" state and is
/// represented by [`AcceptedSequence::ORIGIN`].
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default, Serialize, Deserialize,
)]
#[serde(transparent)]
pub struct AcceptedSequence(u64);

impl AcceptedSequence {
    /// The empty position: nothing has been admitted.
    pub const ORIGIN: Self = Self(0);

    /// Builds a one-based sequence position.
    ///
    /// # Errors
    ///
    /// Returns [`FrontierError::NotAPosition`] for zero; use
    /// [`AcceptedSequence::ORIGIN`] for the empty state.
    pub const fn new(value: u64) -> Result<Self, FrontierError> {
        if value == 0 {
            return Err(FrontierError::NotAPosition);
        }
        Ok(Self(value))
    }

    /// The underlying position.
    #[must_use]
    pub const fn get(self) -> u64 {
        self.0
    }

    /// The position immediately after this one.
    ///
    /// # Errors
    ///
    /// Returns [`FrontierError::SequenceExhausted`] at `u64::MAX`.
    pub const fn next(self) -> Result<Self, FrontierError> {
        match self.0.checked_add(1) {
            Some(value) => Ok(Self(value)),
            None => Err(FrontierError::SequenceExhausted),
        }
    }
}

impl fmt::Display for AcceptedSequence {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Why a record could not be folded.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PoisonReason {
    /// The stream image could not be decoded into a fact.
    Undecodable,
    /// The fact's meter belongs to a different authority than this worker owns.
    CategoryEscape,
    /// The fact violated an invariant the authority guarantees.
    InvariantViolated,
    /// A settlement receipt disagreed with the fact's pinned pricing version.
    PricingVersionMismatch,
    /// The producer offered one identity with two different intents.
    IdentityConflict,
}

impl PoisonReason {
    /// Every poison reason, in a stable order.
    pub const ALL: [Self; 5] = [
        Self::Undecodable,
        Self::CategoryEscape,
        Self::InvariantViolated,
        Self::PricingVersionMismatch,
        Self::IdentityConflict,
    ];

    /// The stable identifier written to a quarantine row and a dead-letter body.
    #[must_use]
    pub const fn id(self) -> &'static str {
        match self {
            Self::Undecodable => "undecodable",
            Self::CategoryEscape => "category_escape",
            Self::InvariantViolated => "invariant_violated",
            Self::PricingVersionMismatch => "pricing_version_mismatch",
            Self::IdentityConflict => "identity_conflict",
        }
    }
}

impl fmt::Display for PoisonReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.id())
    }
}

/// Whether a frontier is advancing or parked.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum FrontierState {
    /// Normal operation.
    Advancing,
    /// Parked at a record that could not be folded.
    Quarantined {
        /// Where the fold stopped.
        at: AcceptedSequence,
        /// Why it stopped.
        reason: PoisonReason,
    },
}

/// Why a frontier advance was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum FrontierError {
    /// Zero was offered as a one-based position.
    #[error("0 is not a sequence position; the empty state is `AcceptedSequence::ORIGIN`")]
    NotAPosition,
    /// The sequence reached `u64::MAX`.
    #[error("the accepted sequence is exhausted")]
    SequenceExhausted,
    /// The advance skipped a position.
    #[error("`{stage}` would jump from {from} to {to}; every stage advances contiguously")]
    NotContiguous {
        /// Which stage refused.
        stage: &'static str,
        /// Where the stage currently is.
        from: u64,
        /// Where the advance would have moved it.
        to: u64,
    },
    /// The advance would have overtaken the stage it follows.
    #[error("`{stage}` cannot pass `{ahead_of}` at {limit}")]
    Overtake {
        /// Which stage refused.
        stage: &'static str,
        /// The stage it must stay behind.
        ahead_of: &'static str,
        /// The position it may not pass.
        limit: u64,
    },
    /// The frontier is parked.
    #[error("frontier is quarantined at {at} ({reason}); only admission may advance")]
    Quarantined {
        /// Where the fold stopped.
        at: u64,
        /// Why it stopped.
        reason: PoisonReason,
    },
}

/// The four-stage position of one `(region, workspace, category)` sequence.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Frontier {
    /// The region this sequence belongs to.
    pub region: RegionId,
    /// The workspace this sequence belongs to.
    pub workspace: WorkspaceId,
    /// The authority category this sequence belongs to.
    pub category: Category,
    /// The last admitted fact.
    pub accepted: AcceptedSequence,
    /// The last fact folded into the query projection.
    pub projected: AcceptedSequence,
    /// The last fact delivered to the central settlement queue.
    pub published: AcceptedSequence,
    /// The last fact covered by a committed settlement receipt.
    pub settled: AcceptedSequence,
    /// The latest service-time end this sequence has admitted.
    pub service_through: Option<Timestamp>,
    /// Whether the frontier is advancing or parked.
    pub state: FrontierState,
}

impl Frontier {
    /// An empty frontier for one sequence.
    #[must_use]
    pub const fn empty(region: RegionId, workspace: WorkspaceId, category: Category) -> Self {
        Self {
            region,
            workspace,
            category,
            accepted: AcceptedSequence::ORIGIN,
            projected: AcceptedSequence::ORIGIN,
            published: AcceptedSequence::ORIGIN,
            settled: AcceptedSequence::ORIGIN,
            service_through: None,
            state: FrontierState::Advancing,
        }
    }

    /// Whether `settled <= published <= projected <= accepted` holds.
    #[must_use]
    pub const fn invariant(&self) -> bool {
        self.settled.0 <= self.published.0
            && self.published.0 <= self.projected.0
            && self.projected.0 <= self.accepted.0
    }

    /// Whether the frontier is parked.
    #[must_use]
    pub const fn is_quarantined(&self) -> bool {
        matches!(self.state, FrontierState::Quarantined { .. })
    }

    /// Admits the next fact.
    ///
    /// Admission is the one advance a quarantined frontier still accepts: a
    /// fact is money evidence and must be stored even when the fold is parked.
    ///
    /// # Errors
    ///
    /// Returns [`FrontierError::NotContiguous`] unless `at == accepted + 1`.
    pub fn admit(&self, at: AcceptedSequence) -> Result<Self, FrontierError> {
        self.step("accepted", self.accepted, at, None)
            .map(|accepted| Self {
                accepted,
                ..self.clone()
            })
    }

    /// Records the latest service-time end this sequence has admitted.
    #[must_use]
    pub fn observe_service_through(&self, at: Timestamp) -> Self {
        let service_through = match self.service_through {
            Some(existing) if existing >= at => Some(existing),
            _ => Some(at),
        };
        Self {
            service_through,
            ..self.clone()
        }
    }

    /// Folds the next fact into the query projection.
    ///
    /// # Errors
    ///
    /// Returns [`FrontierError`] when the frontier is quarantined, when the
    /// advance is not contiguous, or when it would pass `accepted`.
    pub fn project(&self, at: AcceptedSequence) -> Result<Self, FrontierError> {
        self.guard_advancing()?;
        self.step("projected", self.projected, at, Some(("accepted", self.accepted)))
            .map(|projected| Self {
                projected,
                ..self.clone()
            })
    }

    /// Publishes the next fact to the central settlement queue.
    ///
    /// # Errors
    ///
    /// Returns [`FrontierError`] when the frontier is quarantined, when the
    /// advance is not contiguous, or when it would pass `projected`.
    pub fn publish(&self, at: AcceptedSequence) -> Result<Self, FrontierError> {
        self.guard_advancing()?;
        self.step(
            "published",
            self.published,
            at,
            Some(("projected", self.projected)),
        )
        .map(|published| Self {
            published,
            ..self.clone()
        })
    }

    /// Records a committed settlement receipt for the next fact.
    ///
    /// # Errors
    ///
    /// Returns [`FrontierError`] when the frontier is quarantined, when the
    /// advance is not contiguous, or when it would pass `published`.
    pub fn settle(&self, at: AcceptedSequence) -> Result<Self, FrontierError> {
        self.guard_advancing()?;
        self.step(
            "settled",
            self.settled,
            at,
            Some(("published", self.published)),
        )
        .map(|settled| Self {
            settled,
            ..self.clone()
        })
    }

    /// Parks the frontier at a record that could not be folded.
    ///
    /// Nothing is ever skipped to unblock a frontier.
    #[must_use]
    pub fn quarantine(&self, at: AcceptedSequence, reason: PoisonReason) -> Self {
        Self {
            state: FrontierState::Quarantined { at, reason },
            ..self.clone()
        }
    }

    /// Releases a quarantine once the parked record has been resolved.
    #[must_use]
    pub fn release(&self) -> Self {
        Self {
            state: FrontierState::Advancing,
            ..self.clone()
        }
    }

    const fn guard_advancing(&self) -> Result<(), FrontierError> {
        match self.state {
            FrontierState::Advancing => Ok(()),
            FrontierState::Quarantined { at, reason } => Err(FrontierError::Quarantined {
                at: at.0,
                reason,
            }),
        }
    }

    fn step(
        &self,
        stage: &'static str,
        current: AcceptedSequence,
        at: AcceptedSequence,
        behind: Option<(&'static str, AcceptedSequence)>,
    ) -> Result<AcceptedSequence, FrontierError> {
        if at.0 != current.0 + 1 {
            return Err(FrontierError::NotContiguous {
                stage,
                from: current.0,
                to: at.0,
            });
        }
        if let Some((ahead_of, limit)) = behind
            && at.0 > limit.0
        {
            return Err(FrontierError::Overtake {
                stage,
                ahead_of,
                limit: limit.0,
            });
        }
        Ok(at)
    }
}

#[cfg(test)]
mod tests {
    use super::{AcceptedSequence, Frontier, FrontierError, PoisonReason};
    use crate::meter::Category;
    use crate::wire_pending::{RegionId, WorkspaceId};

    fn frontier() -> Frontier {
        Frontier::empty(
            RegionId::parse("eu-west-1").expect("region"),
            WorkspaceId::parse("ws-1").expect("workspace"),
            Category::Compute,
        )
    }

    fn seq(value: u64) -> AcceptedSequence {
        AcceptedSequence::new(value).expect("position")
    }

    #[test]
    fn the_four_stages_advance_in_order_and_hold_the_invariant() {
        let mut state = frontier();
        for position in 1..=3 {
            state = state.admit(seq(position)).expect("admits");
            assert!(state.invariant());
        }
        for position in 1..=3 {
            state = state.project(seq(position)).expect("projects");
            state = state.publish(seq(position)).expect("publishes");
            state = state.settle(seq(position)).expect("settles");
            assert!(state.invariant());
        }
        assert_eq!(state.settled, seq(3));
    }

    #[test]
    fn no_stage_may_skip_a_position() {
        let state = frontier().admit(seq(1)).expect("admits");
        assert!(matches!(
            state.admit(seq(3)),
            Err(FrontierError::NotContiguous { stage: "accepted", .. })
        ));
        assert!(matches!(
            state.project(seq(2)),
            Err(FrontierError::NotContiguous {
                stage: "projected",
                ..
            })
        ));
    }

    #[test]
    fn no_stage_may_overtake_the_stage_it_follows() {
        let state = frontier()
            .admit(seq(1))
            .expect("admits")
            .admit(seq(2))
            .expect("admits");
        let projected = state.project(seq(1)).expect("projects");
        assert!(matches!(
            projected.publish(seq(1)),
            Err(FrontierError::Overtake { .. })
        ));
        let published = projected.project(seq(2)).expect("projects").publish(seq(1));
        assert!(published.is_ok());
    }

    #[test]
    fn a_quarantined_frontier_still_admits_but_never_folds() {
        let state = frontier()
            .admit(seq(1))
            .expect("admits")
            .quarantine(seq(1), PoisonReason::Undecodable);
        assert!(state.is_quarantined());
        assert!(state.admit(seq(2)).is_ok());
        for refused in [
            state.project(seq(1)),
            state.publish(seq(1)),
            state.settle(seq(1)),
        ] {
            assert!(matches!(refused, Err(FrontierError::Quarantined { .. })));
        }
        assert!(state.release().project(seq(1)).is_ok());
    }

    #[test]
    fn zero_is_not_a_position() {
        assert_eq!(AcceptedSequence::new(0), Err(FrontierError::NotAPosition));
        assert_eq!(AcceptedSequence::ORIGIN.get(), 0);
    }

    #[test]
    fn service_through_only_moves_forward() {
        use crate::wire_pending::Timestamp;
        let later = Timestamp::from_unix_millis(2_000).expect("representable");
        let earlier = Timestamp::from_unix_millis(1_000).expect("representable");
        let state = frontier()
            .observe_service_through(later)
            .observe_service_through(earlier);
        assert_eq!(state.service_through, Some(later));
    }
}
