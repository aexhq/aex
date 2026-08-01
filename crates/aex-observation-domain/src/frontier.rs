//! The per-`(scope, signal)` accepted frontier and the scope deletion fence.
//!
//! The frontier is the fence the removed `projection_commit` used to be: the
//! accepted-order sort key of the base table *is* the fence, so there is no
//! generation identity and no generation-bound cursor. It advances only over a
//! contiguous verified range, which is what makes "everything at or below the
//! snapshot is present" a property rather than a hope.
//!
//! The deletion fence lives on `FRONT#{scope}/DELETION`, never on a TTL. A TTL
//! is space reclamation; it is never a completeness or deletion proof.

use aex_wire::types::Timestamp;

/// Why a frontier advance was refused.
#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
pub enum FrontierError {
    /// The range did not start exactly where the frontier ended.
    #[error("expected the next accepted range to start at {expected}, got {found}")]
    NotContiguous {
        /// The ordinal the frontier expected.
        expected: u64,
        /// The ordinal the range offered.
        found: u64,
    },
    /// The counters would overflow.
    #[error("the frontier counters would overflow")]
    Overflow,
}

/// An inclusive range of accepted ordinals published by one commit.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct AcceptedRange {
    lo: u64,
    hi: u64,
}

impl AcceptedRange {
    /// Builds a range, or `None` when the bounds are inverted.
    #[must_use]
    pub const fn new(lo: u64, hi: u64) -> Option<Self> {
        if lo > hi { None } else { Some(Self { lo, hi }) }
    }

    /// The inclusive lower bound.
    #[must_use]
    pub const fn lo(self) -> u64 {
        self.lo
    }

    /// The inclusive upper bound.
    #[must_use]
    pub const fn hi(self) -> u64 {
        self.hi
    }

    /// How many ordinals the range covers.
    #[must_use]
    pub const fn len(self) -> u64 {
        self.hi - self.lo + 1
    }

    /// Never: an [`AcceptedRange`] always covers at least one ordinal.
    #[must_use]
    pub const fn is_empty(self) -> bool {
        false
    }
}

/// The accepted frontier of one `(scope, signal)` pair.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Frontier {
    next_accepted_seq: u64,
    accepted_at: Timestamp,
    earliest_accepted_at: Option<Timestamp>,
    earliest_accepted_seq: u64,
    count: u64,
    logical_bytes: u64,
    revision: u64,
}

impl Frontier {
    /// A frontier that has accepted nothing.
    ///
    /// `created_at` is what the coverage watermark reports until the first
    /// commit lands; it is never presented as an accepted position.
    #[must_use]
    pub const fn empty(created_at: Timestamp) -> Self {
        Self {
            next_accepted_seq: 0,
            accepted_at: created_at,
            earliest_accepted_at: None,
            earliest_accepted_seq: 0,
            count: 0,
            logical_bytes: 0,
            revision: 0,
        }
    }

    /// The next ordinal a commit may claim.
    #[must_use]
    pub const fn next_accepted_seq(self) -> u64 {
        self.next_accepted_seq
    }

    /// The accepted time of the most recent commit.
    #[must_use]
    pub const fn accepted_at(self) -> Timestamp {
        self.accepted_at
    }

    /// The earliest accepted position still retained.
    ///
    /// This is what `earliestReplay` reports now that Kinesis is removed: the
    /// beginning of the scope's retained data, not a 168-hour horizon.
    #[must_use]
    pub fn earliest_accepted_at(self) -> Timestamp {
        self.earliest_accepted_at.unwrap_or(self.accepted_at)
    }

    /// The earliest accepted ordinal still retained.
    #[must_use]
    pub const fn earliest_accepted_seq(self) -> u64 {
        self.earliest_accepted_seq
    }

    /// How many observations the scope holds for this signal.
    #[must_use]
    pub const fn count(self) -> u64 {
        self.count
    }

    /// How many logical bytes the scope holds for this signal.
    #[must_use]
    pub const fn logical_bytes(self) -> u64 {
        self.logical_bytes
    }

    /// The optimistic-concurrency token the commit condition reads.
    #[must_use]
    pub const fn revision(self) -> u64 {
        self.revision
    }

    /// Advances the frontier over one contiguous accepted range.
    ///
    /// # Errors
    ///
    /// Returns [`FrontierError::NotContiguous`] when the range does not start at
    /// [`Frontier::next_accepted_seq`], and [`FrontierError::Overflow`] when a
    /// counter would wrap. A refused advance changes nothing.
    pub fn advance(
        &mut self,
        range: AcceptedRange,
        count: u64,
        logical_bytes: u64,
        at: Timestamp,
    ) -> Result<(), FrontierError> {
        if range.lo() != self.next_accepted_seq {
            return Err(FrontierError::NotContiguous {
                expected: self.next_accepted_seq,
                found: range.lo(),
            });
        }
        let next = range.hi().checked_add(1).ok_or(FrontierError::Overflow)?;
        let total = self
            .count
            .checked_add(count)
            .ok_or(FrontierError::Overflow)?;
        let bytes = self
            .logical_bytes
            .checked_add(logical_bytes)
            .ok_or(FrontierError::Overflow)?;
        let revision = self
            .revision
            .checked_add(1)
            .ok_or(FrontierError::Overflow)?;
        if self.earliest_accepted_at.is_none() {
            self.earliest_accepted_at = Some(at);
            self.earliest_accepted_seq = range.lo();
        }
        self.next_accepted_seq = next;
        self.accepted_at = at;
        self.count = total;
        self.logical_bytes = bytes;
        self.revision = revision;
        Ok(())
    }

    /// Moves the earliest retained position forward after a retention trim.
    ///
    /// This is the only way the earliest position advances; an append never
    /// moves it, because appending does not make older data less retained.
    pub const fn trim_to(&mut self, seq: u64, at: Timestamp) {
        self.earliest_accepted_seq = seq;
        self.earliest_accepted_at = Some(at);
    }
}

/// Where a scope's deletion is.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum DeletionState {
    /// Nothing is being deleted.
    None,
    /// The denial fence is up; admission, grants and query fail closed.
    Fencing,
    /// Items and objects are being enumerated and removed.
    Deleting,
    /// A separate identity is proving the removal.
    Verifying,
    /// The tombstone is proven.
    Complete,
}

impl DeletionState {
    /// Every state, in declared order.
    pub const ALL: &'static [DeletionState] = &[
        DeletionState::None,
        DeletionState::Fencing,
        DeletionState::Deleting,
        DeletionState::Verifying,
        DeletionState::Complete,
    ];

    /// The durable spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Fencing => "fencing",
            Self::Deleting => "deleting",
            Self::Verifying => "verifying",
            Self::Complete => "complete",
        }
    }

    /// Whether the denial fence is up.
    #[must_use]
    pub const fn is_fenced(self) -> bool {
        !matches!(self, Self::None)
    }
}

/// Why a deletion transition was refused.
#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
pub enum DeletionTransitionError {
    /// The transition would skip a step.
    #[error("a deletion may not move from `{from:?}` to `{to:?}`")]
    OutOfOrder {
        /// The state the scope is in.
        from: DeletionState,
        /// The state that was requested.
        to: DeletionState,
    },
}

/// The scope's deletion state and its monotonic epoch.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ScopeDeletion {
    state: DeletionState,
    epoch: u64,
    requested_at: Option<Timestamp>,
    completed_at: Option<Timestamp>,
}

impl ScopeDeletion {
    /// A scope with nothing deleted.
    #[must_use]
    pub const fn none() -> Self {
        Self {
            state: DeletionState::None,
            epoch: 0,
            requested_at: None,
            completed_at: None,
        }
    }

    /// The current state.
    #[must_use]
    pub const fn state(self) -> DeletionState {
        self.state
    }

    /// The monotonic deletion epoch.
    ///
    /// Every admission, export publication, grant mint and query pins this
    /// value and fails closed once it advances.
    #[must_use]
    pub const fn epoch(self) -> u64 {
        self.epoch
    }

    /// When the current deletion was requested.
    #[must_use]
    pub const fn requested_at(self) -> Option<Timestamp> {
        self.requested_at
    }

    /// When the current deletion was proven complete.
    #[must_use]
    pub const fn completed_at(self) -> Option<Timestamp> {
        self.completed_at
    }

    /// Whether a pinned epoch is still the live one.
    #[must_use]
    pub const fn epoch_is_current(self, pinned: u64) -> bool {
        pinned == self.epoch
    }

    /// Raises the denial fence and advances the epoch.
    ///
    /// # Errors
    ///
    /// Returns [`DeletionTransitionError::OutOfOrder`] when a deletion is
    /// already in flight; a second request never resets one in progress.
    pub fn begin(&mut self, at: Timestamp) -> Result<(), DeletionTransitionError> {
        match self.state {
            DeletionState::None | DeletionState::Complete => {
                self.state = DeletionState::Fencing;
                self.epoch += 1;
                self.requested_at = Some(at);
                self.completed_at = None;
                Ok(())
            }
            from => Err(DeletionTransitionError::OutOfOrder {
                from,
                to: DeletionState::Fencing,
            }),
        }
    }

    /// Moves from `fencing` to `deleting`.
    ///
    /// # Errors
    ///
    /// Returns [`DeletionTransitionError::OutOfOrder`] from any other state.
    pub fn to_deleting(&mut self) -> Result<(), DeletionTransitionError> {
        self.step(DeletionState::Fencing, DeletionState::Deleting)
    }

    /// Moves from `deleting` to `verifying`.
    ///
    /// # Errors
    ///
    /// Returns [`DeletionTransitionError::OutOfOrder`] from any other state.
    pub fn to_verifying(&mut self) -> Result<(), DeletionTransitionError> {
        self.step(DeletionState::Deleting, DeletionState::Verifying)
    }

    /// Records the proven tombstone.
    ///
    /// # Errors
    ///
    /// Returns [`DeletionTransitionError::OutOfOrder`] from any state other
    /// than `verifying`: completion may never skip the delete and verify steps,
    /// because a provider mechanism is never proof.
    pub fn complete(&mut self, at: Timestamp) -> Result<(), DeletionTransitionError> {
        self.step(DeletionState::Verifying, DeletionState::Complete)?;
        self.completed_at = Some(at);
        Ok(())
    }

    fn step(
        &mut self,
        from: DeletionState,
        to: DeletionState,
    ) -> Result<(), DeletionTransitionError> {
        if self.state == from {
            self.state = to;
            Ok(())
        } else {
            Err(DeletionTransitionError::OutOfOrder {
                from: self.state,
                to,
            })
        }
    }
}

impl Default for ScopeDeletion {
    fn default() -> Self {
        Self::none()
    }
}

#[cfg(test)]
mod tests {
    use super::{AcceptedRange, DeletionState, DeletionTransitionError, Frontier, ScopeDeletion};
    use aex_wire::types::Timestamp;

    fn instant(millis: i64) -> Timestamp {
        Timestamp::from_unix_millis(millis).expect("representable")
    }

    #[test]
    fn a_trim_is_the_only_way_the_earliest_position_advances() {
        let mut frontier = Frontier::empty(instant(0));
        frontier
            .advance(AcceptedRange::new(0, 4).expect("range"), 5, 10, instant(5))
            .expect("advances");
        assert_eq!(frontier.earliest_accepted_seq(), 0);
        frontier.trim_to(3, instant(7));
        assert_eq!(frontier.earliest_accepted_seq(), 3);
        assert_eq!(frontier.earliest_accepted_at(), instant(7));
        assert_eq!(frontier.accepted_at(), instant(5));
    }

    #[test]
    fn a_second_deletion_request_never_resets_one_in_flight() {
        let mut deletion = ScopeDeletion::default();
        deletion.begin(instant(1)).expect("fences");
        assert_eq!(
            deletion.begin(instant(2)),
            Err(DeletionTransitionError::OutOfOrder {
                from: DeletionState::Fencing,
                to: DeletionState::Fencing
            })
        );
        assert_eq!(deletion.epoch(), 1);
        assert_eq!(deletion.requested_at(), Some(instant(1)));
        assert!(deletion.state().is_fenced());
        assert_eq!(DeletionState::ALL.len(), 5);
        assert_eq!(DeletionState::Deleting.as_str(), "deleting");
    }

    #[test]
    fn a_completed_deletion_records_when_it_was_proven() {
        let mut deletion = ScopeDeletion::none();
        deletion.begin(instant(1)).expect("fences");
        deletion.to_deleting().expect("deletes");
        deletion.to_verifying().expect("verifies");
        deletion.complete(instant(9)).expect("completes");
        assert_eq!(deletion.completed_at(), Some(instant(9)));
        let range = AcceptedRange::new(2, 6).expect("range");
        assert_eq!(range.len(), 5);
        assert!(!range.is_empty());
        assert_eq!(range.lo(), 2);
        assert_eq!(range.hi(), 6);
        assert!(AcceptedRange::new(6, 2).is_none());
    }
}
