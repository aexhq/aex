//! The session deletion fence.
//!
//! The guard lives here rather than in `aex-session-domain` because operation
//! admission must fence against it and this crate is the lower of the two
//! (D-26). `aex-session-domain` owns the transitions — trash, restore, purge,
//! `purge_complete` — and re-exports these three types unchanged, so exactly one
//! deletion state exists in the workspace.

use aex_wire::ids::{OperationId, SessionId};
use aex_wire::types::Timestamp;

/// How far into deletion a session is.
///
/// `Purging` is absorbing: no transition leaves it except the tombstone.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum DeletionState {
    /// Ordinary.
    Live,
    /// In the recovery window.
    Trashed,
    /// The destructive fence has been crossed.
    Purging,
    /// Gone; only a tombstone remains.
    Purged,
}

impl DeletionState {
    /// Every state, in lifecycle order.
    pub const ALL: [Self; 4] = [Self::Live, Self::Trashed, Self::Purging, Self::Purged];

    /// Whether no transition leaves this state.
    #[must_use]
    pub const fn is_absorbing(self) -> bool {
        matches!(self, Self::Purging | Self::Purged)
    }

    /// Whether ordinary session work may be admitted.
    #[must_use]
    pub const fn admits_work(self) -> bool {
        matches!(self, Self::Live)
    }
}

/// The monotone deletion epoch.
///
/// Trash and purge each advance it by exactly one, so a command built against
/// the pre-fence epoch fails its condition rather than committing behind the
/// fence.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct DeletionEpoch(pub u64);

impl DeletionEpoch {
    /// The epoch a fresh session starts at.
    pub const INITIAL: Self = Self(0);

    /// The next epoch.
    ///
    /// # Panics
    ///
    /// Panics on `u64` overflow, which is a corrupted authority rather than a
    /// customer condition.
    #[must_use]
    pub const fn next(self) -> Self {
        match self.0.checked_add(1) {
            Some(value) => Self(value),
            None => panic!("deletion epoch overflowed"),
        }
    }
}

/// What a session's deletion record says.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DeletionGuard {
    /// The owning session.
    pub session: SessionId,
    /// How far into deletion it is.
    pub state: DeletionState,
    /// The current fence position.
    pub epoch: DeletionEpoch,
    /// When it was trashed, when it has been.
    pub trashed_at: Option<Timestamp>,
    /// The instant after which restore is refused.
    pub recovery_deadline: Option<Timestamp>,
    /// The operation that claimed the purge, when one has.
    pub purge_operation: Option<OperationId>,
}

impl DeletionGuard {
    /// The guard a freshly created session carries.
    #[must_use]
    pub const fn live(session: SessionId) -> Self {
        Self {
            session,
            state: DeletionState::Live,
            epoch: DeletionEpoch::INITIAL,
            trashed_at: None,
            recovery_deadline: None,
            purge_operation: None,
        }
    }

    /// Whether ordinary session work may be admitted.
    #[must_use]
    pub const fn admits_work(&self) -> bool {
        self.state.admits_work()
    }
}

#[cfg(test)]
mod tests {
    use super::{DeletionEpoch, DeletionState};

    #[test]
    fn purging_and_purged_are_absorbing_and_admit_nothing() {
        for state in DeletionState::ALL {
            assert_eq!(
                state.is_absorbing(),
                matches!(state, DeletionState::Purging | DeletionState::Purged)
            );
            assert_eq!(state.admits_work(), state == DeletionState::Live);
        }
    }

    #[test]
    fn the_epoch_advances_by_exactly_one() {
        assert_eq!(DeletionEpoch::INITIAL.next(), DeletionEpoch(1));
        assert_eq!(DeletionEpoch(9).next(), DeletionEpoch(10));
    }
}
