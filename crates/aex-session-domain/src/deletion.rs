//! Trash, restore and purge.
//!
//! Trash is the linearization point: it advances the deletion epoch, sets
//! `WorkAdmission::Trashing`, cancels pending approvals and agent work, removes
//! the session from ordinary list and read, and starts the recovery window.
//! Purge advances the epoch again and is **absorbing**.
//!
//! Every non-purge write conditions on `DeletionState::Live` and the exact
//! epoch, so a concurrent in-flight command fails its condition rather than
//! committing after the fence.
//!
//! The state, epoch and guard types themselves live in `aex-operation-domain`
//! (D-26), because operation admission must fence against them and that crate is
//! lower in the graph. They are re-exported here unchanged.

pub use aex_operation_domain::{DeletionEpoch, DeletionGuard, DeletionState};

use aex_wire::ids::{GenerationId, OperationId, SessionId};
use aex_wire::types::Timestamp;
use time::Duration;

use crate::lineage::PurgeCascade;
use crate::session::WorkAdmission;

/// What a session leaves behind.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionTombstone {
    /// Which session.
    pub session: SessionId,
    /// The operation that purged it.
    pub purged_by: OperationId,
    /// The epoch the purge completed at.
    pub epoch: DeletionEpoch,
    /// When it completed.
    pub purged_at: Timestamp,
}

/// The durable evidence a purge must present before it may complete.
///
/// Every field is a fact somebody else made durable; the domain only checks that
/// all of them are present. That is why cleanup is a checklist here and a set of
/// conditions in the transaction, not a sequence of calls.
///
/// The fields are flags rather than a richer type on purpose: each is a separate
/// durable fact produced by a different owner, so collapsing them into a state
/// machine would invent an ordering the producers do not have.
#[allow(
    clippy::struct_excessive_bools,
    reason = "each flag is an independent durable fact with its own producer"
)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PurgeEvidence {
    /// Every fence has committed.
    pub fences_committed: bool,
    /// Every non-purge session operation is cancelled or terminally fenced.
    pub operations_settled: bool,
    /// Every provider generation is confirmed terminated.
    pub generations_terminated: Vec<GenerationId>,
    /// No grant can still be minted.
    pub grants_closed: bool,
    /// Owner and root edges are removed.
    pub edges_removed: bool,
    /// Unpinned content is purged.
    pub content_purged: bool,
    /// Descendants are detached or cascaded.
    pub descendants_settled: bool,
    /// The content-free denial fact is durable.
    pub denial_durable: bool,
    /// And projected.
    pub denial_projected: bool,
}

impl PurgeEvidence {
    /// The first missing item, when anything is missing.
    #[must_use]
    pub fn missing(&self) -> Option<&'static str> {
        if !self.fences_committed {
            return Some("fences_committed");
        }
        if !self.operations_settled {
            return Some("operations_settled");
        }
        if !self.grants_closed {
            return Some("grants_closed");
        }
        if !self.edges_removed {
            return Some("edges_removed");
        }
        if !self.content_purged {
            return Some("content_purged");
        }
        if !self.descendants_settled {
            return Some("descendants_settled");
        }
        if !self.denial_durable {
            return Some("denial_durable");
        }
        if !self.denial_projected {
            return Some("denial_projected");
        }
        None
    }
}

/// The guard and admission a trash produces.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrashCommit {
    /// The guard after the trash.
    pub guard: DeletionGuard,
    /// The admission the session is left in.
    pub admission: WorkAdmission,
}

/// The guard and admission a restore produces.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RestoreCommit {
    /// The guard after the restore.
    pub guard: DeletionGuard,
    /// The admission the session is left in.
    pub admission: WorkAdmission,
}

/// The guard, admission and closure a purge produces.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PurgeCommit {
    /// The guard after the purge claim.
    pub guard: DeletionGuard,
    /// The admission the session is left in.
    pub admission: WorkAdmission,
    /// How descendants are treated.
    pub cascade: PurgeCascade,
    /// The immutable closure the fence supplied. Never derived here.
    pub closure: Vec<SessionId>,
}

/// Why a deletion transition was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum DeletionRejection {
    /// The session is already trashed.
    #[error("session is already trashed")]
    AlreadyTrashed,
    /// The session is not in the recovery window.
    #[error("session deletion state is {0:?}")]
    NotTrashed(DeletionState),
    /// The recovery window has closed.
    #[error("recovery window closed at {deadline:?}")]
    RecoveryWindowElapsed {
        /// When it closed.
        deadline: Timestamp,
    },
    /// A purge already holds the session.
    #[error("purge {0} is in progress")]
    PurgeInProgress(OperationId),
    /// The session is gone.
    #[error("session was purged by {0}")]
    Purged(OperationId),
    /// The purge evidence is incomplete.
    #[error("purge evidence is missing `{item}`")]
    EvidenceIncomplete {
        /// The first missing item.
        item: &'static str,
    },
}

/// Moves a session into the recovery window.
///
/// # Errors
///
/// Returns [`DeletionRejection`] when the session is already trashed, is being
/// purged, or is gone.
pub fn trash(
    guard: &DeletionGuard,
    operation: OperationId,
    now: Timestamp,
    window: Duration,
) -> Result<TrashCommit, DeletionRejection> {
    match guard.state {
        DeletionState::Trashed => Err(DeletionRejection::AlreadyTrashed),
        DeletionState::Purging => Err(DeletionRejection::PurgeInProgress(
            guard.purge_operation.unwrap_or(operation),
        )),
        DeletionState::Purged => Err(DeletionRejection::Purged(
            guard.purge_operation.unwrap_or(operation),
        )),
        DeletionState::Live => {
            let deadline = advance(now, window);
            Ok(TrashCommit {
                guard: DeletionGuard {
                    state: DeletionState::Trashed,
                    epoch: guard.epoch.next(),
                    trashed_at: Some(now),
                    recovery_deadline: Some(deadline),
                    purge_operation: None,
                    ..*guard
                },
                admission: WorkAdmission::Trashing,
            })
        }
    }
}

/// Brings a session back out of the recovery window.
///
/// # Errors
///
/// Returns [`DeletionRejection`] when the session is not trashed, when the
/// window has closed, or when a purge already claimed it.
pub fn restore(
    guard: &DeletionGuard,
    operation: OperationId,
    now: Timestamp,
) -> Result<RestoreCommit, DeletionRejection> {
    let _ = operation;
    match guard.state {
        DeletionState::Trashed => {
            if let Some(claim) = guard.purge_operation {
                return Err(DeletionRejection::PurgeInProgress(claim));
            }
            if let Some(deadline) = guard.recovery_deadline
                && now.unix_millis() > deadline.unix_millis()
            {
                return Err(DeletionRejection::RecoveryWindowElapsed { deadline });
            }
            Ok(RestoreCommit {
                guard: DeletionGuard {
                    state: DeletionState::Live,
                    epoch: guard.epoch.next(),
                    trashed_at: None,
                    recovery_deadline: None,
                    purge_operation: None,
                    ..*guard
                },
                admission: WorkAdmission::Open,
            })
        }
        other => Err(DeletionRejection::NotTrashed(other)),
    }
}

/// Claims the destructive fence.
///
/// The closure member list is supplied by the caller's fence and is never
/// derived here: deriving it would let a concurrent clone escape the closure
/// between the derivation and the commit.
///
/// # Errors
///
/// Returns [`DeletionRejection`] when a purge already holds the session or the
/// session is gone.
pub fn purge(
    guard: &DeletionGuard,
    operation: OperationId,
    cascade: PurgeCascade,
    closure: &[SessionId],
    now: Timestamp,
) -> Result<PurgeCommit, DeletionRejection> {
    let _ = now;
    match guard.state {
        DeletionState::Purging => Err(DeletionRejection::PurgeInProgress(
            guard.purge_operation.unwrap_or(operation),
        )),
        DeletionState::Purged => Err(DeletionRejection::Purged(
            guard.purge_operation.unwrap_or(operation),
        )),
        DeletionState::Live | DeletionState::Trashed => Ok(PurgeCommit {
            guard: DeletionGuard {
                state: DeletionState::Purging,
                epoch: guard.epoch.next(),
                purge_operation: Some(operation),
                ..*guard
            },
            admission: WorkAdmission::Purging,
            cascade,
            closure: closure.to_vec(),
        }),
    }
}

/// Completes a purge against its full evidence predicate.
///
/// # Errors
///
/// Returns [`DeletionRejection::EvidenceIncomplete`] naming the first missing
/// item, and [`DeletionRejection::NotTrashed`] when the session never entered
/// `Purging`. A retryable cleanup failure is expressed by *not calling this*
/// (D-24): the operation stays running rather than failing and stranding a
/// session whose admission is already closed.
pub fn purge_complete(
    guard: &DeletionGuard,
    evidence: &PurgeEvidence,
    now: Timestamp,
) -> Result<SessionTombstone, DeletionRejection> {
    if guard.state != DeletionState::Purging {
        return Err(DeletionRejection::NotTrashed(guard.state));
    }
    if let Some(item) = evidence.missing() {
        return Err(DeletionRejection::EvidenceIncomplete { item });
    }
    let purged_by = guard
        .purge_operation
        .ok_or(DeletionRejection::NotTrashed(guard.state))?;
    Ok(SessionTombstone {
        session: guard.session,
        purged_by,
        epoch: guard.epoch,
        purged_at: now,
    })
}

fn advance(now: Timestamp, window: Duration) -> Timestamp {
    let millis = now
        .unix_millis()
        .saturating_add(i64::try_from(window.whole_milliseconds()).unwrap_or(i64::MAX));
    Timestamp::from_unix_millis(millis)
        .unwrap_or_else(|_| unreachable!("a bounded recovery window keeps the instant in range"))
}

#[cfg(test)]
mod tests {
    use aex_wire::ids::{OperationId, PrefixedId as _, SessionId, Uuid7};
    use aex_wire::types::Timestamp;
    use time::Duration;

    use super::{
        DeletionGuard, DeletionRejection, DeletionState, PurgeEvidence, purge, purge_complete,
        restore, trash,
    };
    use crate::lineage::PurgeCascade;
    use crate::session::WorkAdmission;

    fn moment(millis: i64) -> Timestamp {
        Timestamp::from_unix_millis(millis).expect("in range")
    }

    fn session() -> SessionId {
        SessionId::from_uuid7(Uuid7::compose(1, [1; 10]))
    }

    fn operation(tag: u8) -> OperationId {
        OperationId::from_uuid7(Uuid7::compose(1, [tag; 10]))
    }

    fn complete_evidence() -> PurgeEvidence {
        PurgeEvidence {
            fences_committed: true,
            operations_settled: true,
            generations_terminated: Vec::new(),
            grants_closed: true,
            edges_removed: true,
            content_purged: true,
            descendants_settled: true,
            denial_durable: true,
            denial_projected: true,
        }
    }

    #[test]
    fn trash_advances_the_epoch_and_closes_admission() {
        let guard = DeletionGuard::live(session());
        let commit = trash(&guard, operation(2), moment(0), Duration::days(7)).expect("trashes");
        assert_eq!(commit.guard.state, DeletionState::Trashed);
        assert_eq!(commit.guard.epoch, guard.epoch.next());
        assert_eq!(commit.admission, WorkAdmission::Trashing);
        assert_eq!(
            trash(&commit.guard, operation(2), moment(1), Duration::days(7)),
            Err(DeletionRejection::AlreadyTrashed)
        );
    }

    #[test]
    fn restore_works_only_inside_the_window_and_before_a_claim() {
        let guard = DeletionGuard::live(session());
        let trashed = trash(&guard, operation(2), moment(0), Duration::milliseconds(100))
            .expect("trashes")
            .guard;
        assert_eq!(
            restore(&trashed, operation(3), moment(101)),
            Err(DeletionRejection::RecoveryWindowElapsed {
                deadline: moment(100)
            })
        );
        let restored = restore(&trashed, operation(3), moment(50)).expect("restores");
        assert_eq!(restored.guard.state, DeletionState::Live);
        assert_eq!(restored.guard.epoch, trashed.epoch.next());
        assert_eq!(restored.admission, WorkAdmission::Open);

        let claimed = purge(
            &trashed,
            operation(4),
            PurgeCascade::DetachDescendants,
            &[],
            moment(10),
        )
        .expect("claims")
        .guard;
        assert_eq!(
            restore(&claimed, operation(3), moment(20)),
            Err(DeletionRejection::NotTrashed(DeletionState::Purging))
        );
    }

    #[test]
    fn purging_is_absorbing() {
        let guard = DeletionGuard::live(session());
        let purging = purge(
            &guard,
            operation(4),
            PurgeCascade::PurgeClosure,
            &[session()],
            moment(0),
        )
        .expect("claims");
        assert_eq!(purging.guard.state, DeletionState::Purging);
        assert_eq!(purging.closure, vec![session()]);
        assert_eq!(
            trash(&purging.guard, operation(2), moment(1), Duration::days(1)),
            Err(DeletionRejection::PurgeInProgress(operation(4)))
        );
        assert_eq!(
            purge(
                &purging.guard,
                operation(5),
                PurgeCascade::PurgeClosure,
                &[],
                moment(1)
            ),
            Err(DeletionRejection::PurgeInProgress(operation(4)))
        );
    }

    #[test]
    fn completion_requires_the_whole_evidence_predicate() {
        let guard = DeletionGuard::live(session());
        let purging = purge(
            &guard,
            operation(4),
            PurgeCascade::DetachDescendants,
            &[],
            moment(0),
        )
        .expect("claims")
        .guard;

        let mut incomplete = complete_evidence();
        incomplete.denial_projected = false;
        assert_eq!(
            purge_complete(&purging, &incomplete, moment(1)),
            Err(DeletionRejection::EvidenceIncomplete {
                item: "denial_projected"
            })
        );

        let tombstone =
            purge_complete(&purging, &complete_evidence(), moment(1)).expect("completes");
        assert_eq!(tombstone.session, session());
        assert_eq!(tombstone.purged_by, operation(4));
    }
}
