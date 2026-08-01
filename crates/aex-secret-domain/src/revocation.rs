//! Revocation.
//!
//! `revoke` increments the epoch for a `(workspace, name)` in one atomic fence
//! and marks every unrevoked source of that name revoked. That is what makes it
//! **retroactive** across the whole lineage, including clones, which carry the
//! source generation forward: a session that admitted an earlier generation is
//! denied by the epoch comparison alone, without touching its custody row.

use aex_wire::types::Timestamp;

use crate::secret::{SecretName, SecretState, WorkspaceSecret};

/// The monotone revocation epoch of one `(workspace, name)`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct RevocationEpoch(pub u64);

impl RevocationEpoch {
    /// The epoch a fresh name starts at.
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
            None => panic!("revocation epoch overflowed"),
        }
    }
}

/// The result of a revocation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RevokeCommit {
    /// The record after the revocation.
    pub secret: WorkspaceSecret,
    /// The epoch every later admission must present.
    pub epoch: RevocationEpoch,
    /// Whether the record's own state changed to `Revoked`.
    pub state_changed: bool,
}

/// Revokes every source of a name.
///
/// Always increments the epoch, even when the record is already `Revoked`: the
/// epoch is what denies previously admitted custody, and a caller asking again
/// is asking for a fresh fence, not a no-op.
///
/// # Errors
///
/// Returns [`crate::secret::SecretRejection::Deleted`] for a tombstone: there is
/// nothing left to revoke and pretending otherwise would report a fence that
/// protects nothing.
pub fn revoke(
    current: &WorkspaceSecret,
    now: Timestamp,
) -> Result<RevokeCommit, crate::secret::SecretRejection> {
    if current.state == SecretState::Deleted {
        return Err(crate::secret::SecretRejection::Deleted(
            current.name.clone(),
        ));
    }
    let epoch = current.revocation_epoch.next();
    Ok(RevokeCommit {
        secret: WorkspaceSecret {
            state: SecretState::Revoked,
            revocation_epoch: epoch,
            revoked_at: Some(now),
            updated_at: now,
            ..current.clone()
        },
        epoch,
        state_changed: current.state != SecretState::Revoked,
    })
}

/// Whether a name's epoch has moved past what a holder admitted.
#[must_use]
pub const fn is_revoked_since(admitted: RevocationEpoch, current: RevocationEpoch) -> bool {
    admitted.0 < current.0
}

/// The name a revocation fences. Present so a caller cannot pass the wrong
/// record's epoch by accident.
#[must_use]
pub fn fenced_name(commit: &RevokeCommit) -> &SecretName {
    &commit.secret.name
}

#[cfg(test)]
mod tests {
    use super::RevocationEpoch;

    #[test]
    fn the_epoch_advances_by_exactly_one() {
        assert_eq!(RevocationEpoch::INITIAL.next(), RevocationEpoch(1));
        assert_eq!(RevocationEpoch(4).next(), RevocationEpoch(5));
    }
}
