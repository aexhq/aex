//! The session deletion fence.
//!
//! The guard lives here rather than in `aex-session-domain` because operation
//! admission must fence against it and this crate is the lower of the two
//! (D-26). `aex-session-domain` owns the irreversible delete transition and
//! re-exports these types unchanged, so exactly one deletion state exists in
//! the workspace.

use aex_wire::ids::{OperationId, SessionId};
/// How far into deletion a session is.
///
/// There is no trash or restore window. Once deletion starts, ordinary work
/// never becomes admissible again.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum DeletionState {
    /// Ordinary.
    Live,
    /// The destructive fence has been crossed.
    Deleting,
    /// Gone; only a tombstone remains.
    Deleted,
}

impl DeletionState {
    /// Every state, in lifecycle order.
    pub const ALL: [Self; 3] = [Self::Live, Self::Deleting, Self::Deleted];

    /// Whether no transition leaves this state.
    #[must_use]
    pub const fn is_absorbing(self) -> bool {
        matches!(self, Self::Deleting | Self::Deleted)
    }

    /// Whether ordinary session work may be admitted.
    #[must_use]
    pub const fn admits_work(self) -> bool {
        matches!(self, Self::Live)
    }
}

/// The monotone deletion epoch.
///
/// Irreversible delete advances it exactly once, so a command built against the
/// pre-fence epoch fails its condition rather than committing behind the fence.
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
    /// The operation that claimed irreversible deletion, when one has.
    pub delete_operation: Option<OperationId>,
}

impl DeletionGuard {
    /// The guard a freshly created session carries.
    #[must_use]
    pub const fn live(session: SessionId) -> Self {
        Self {
            session,
            state: DeletionState::Live,
            epoch: DeletionEpoch::INITIAL,
            delete_operation: None,
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
    fn deleting_and_deleted_are_absorbing_and_admit_nothing() {
        for state in DeletionState::ALL {
            assert_eq!(
                state.is_absorbing(),
                matches!(state, DeletionState::Deleting | DeletionState::Deleted)
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
