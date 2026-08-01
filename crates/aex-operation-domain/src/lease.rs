//! Claim, lease and fence.
//!
//! A work item carries at most one [`Lease`]. The fence increments on every
//! ownership change and **never** on a renew (D-18): renewal churn would
//! invalidate the legitimate owner's in-flight writes. Because the fence is a
//! transaction condition, a worker paused past its lease cannot commit even
//! while it still believes it owns the item.

use aex_wire::ids::{OperationId, Uuid7};
use aex_wire::types::Timestamp;
use time::Duration;

use crate::cursor::ContinuationCursor;
use crate::due::DueShard;
use crate::operation::{FailureClass, OperationFailure, OperationKind, OperationResult, Progress};

/// The monotone ownership fence.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Fence(pub u64);

impl Fence {
    /// The fence an unclaimed item carries.
    pub const INITIAL: Self = Self(0);

    /// The next fence.
    ///
    /// # Panics
    ///
    /// Panics on `u64` overflow, which is a corrupted authority rather than a
    /// customer condition.
    #[must_use]
    pub const fn next(self) -> Self {
        match self.0.checked_add(1) {
            Some(value) => Self(value),
            None => panic!("work fence overflowed"),
        }
    }
}

/// Who holds a lease.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct OwnerId(pub Uuid7);

/// One work item identity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct WorkId(pub Uuid7);

impl WorkId {
    /// The bytes the shard and backoff functions hash.
    #[must_use]
    pub const fn as_bytes(&self) -> &[u8; 16] {
        self.0.as_bytes()
    }
}

/// The producer-derived deduplication identity of a work item.
///
/// A hint that arrives twice must not become two units of work, so the identity
/// is derived from the operation and its step rather than minted per enqueue.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct DedupIdentity {
    /// The operation the item belongs to.
    pub operation: OperationId,
    /// Which step of that operation.
    pub step: u32,
}

/// One exclusive claim on a work item.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Lease {
    /// Who holds it.
    pub owner: OwnerId,
    /// The fence the holder must present on every write.
    pub fence: Fence,
    /// When it lapses.
    pub expires_at: Timestamp,
}

impl Lease {
    /// Whether the lease is still held at `now`. Expiry is inclusive of its own
    /// instant: at `expires_at` the lease is gone.
    #[must_use]
    pub const fn is_live_at(&self, now: Timestamp) -> bool {
        now.unix_millis() < self.expires_at.unix_millis()
    }
}

/// Where a work item is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum WorkState {
    /// Available to claim.
    Runnable,
    /// Someone holds it.
    Claimed,
    /// Finished.
    Completed,
    /// Attempts exhausted; never redriven again (D-10).
    Poison,
}

impl WorkState {
    /// Every state, in lifecycle order.
    pub const ALL: [Self; 4] = [Self::Runnable, Self::Claimed, Self::Completed, Self::Poison];

    /// Whether no transition leaves this state.
    #[must_use]
    pub const fn is_terminal(self) -> bool {
        matches!(self, Self::Completed | Self::Poison)
    }
}

/// One unit of continued work.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkItem {
    /// The item identity.
    pub id: WorkId,
    /// The operation it advances.
    pub operation: OperationId,
    /// What that operation does.
    pub kind: OperationKind,
    /// The due-scan shard it lives in.
    pub shard: DueShard,
    /// When it becomes claimable.
    pub due_at: Timestamp,
    /// Scheduling priority; lower runs first.
    pub priority: u16,
    /// How many attempts have been made.
    pub attempt: u16,
    /// How many attempts are allowed.
    pub max_attempts: u16,
    /// The current lease, when the item is claimed.
    pub lease: Option<Lease>,
    /// Where the item is.
    pub state: WorkState,
    /// Its producer-derived deduplication identity.
    pub dedup: DedupIdentity,
    /// Whether the owning operation has recorded a cancel.
    pub cancel_requested: bool,
}

impl WorkItem {
    /// The highest fence the item has ever issued.
    #[must_use]
    pub fn fence(&self) -> Fence {
        self.lease.map_or(Fence::INITIAL, |lease| lease.fence)
    }
}

/// What a claim attempt decided.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClaimOutcome {
    /// The item was free and is now held.
    Claimed(Lease),
    /// The caller already held it and extended its own lease.
    Renewed(Lease),
    /// A previous holder's lease had expired and the item was taken.
    Stolen {
        /// Who held it.
        previous: OwnerId,
        /// The new lease.
        lease: Lease,
    },
    /// The claim was refused.
    Denied(ClaimDenial),
}

impl ClaimOutcome {
    /// The lease the outcome grants, when it grants one.
    #[must_use]
    pub const fn lease(&self) -> Option<Lease> {
        match self {
            Self::Claimed(lease) | Self::Renewed(lease) | Self::Stolen { lease, .. } => {
                Some(*lease)
            }
            Self::Denied(_) => None,
        }
    }
}

/// Why a claim was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClaimDenial {
    /// Someone else holds a live lease.
    HeldByOther {
        /// Who.
        owner: OwnerId,
        /// Until when.
        expires_at: Timestamp,
    },
    /// The item is not due yet.
    NotDue {
        /// When it becomes due.
        due_at: Timestamp,
    },
    /// The item is finished.
    Terminal(WorkState),
    /// The owning operation has been cancelled.
    CancelRequested,
    /// The item has used up its attempts.
    AttemptsExhausted {
        /// Attempts made.
        attempt: u16,
        /// Attempts allowed.
        max: u16,
    },
}

/// Claims a work item.
///
/// Order is fixed: terminal, then cancel, then due, then attempts, then
/// ownership. An expired-lease takeover is reported as [`ClaimOutcome::Stolen`]
/// rather than a plain claim, so a takeover is always visible.
#[must_use]
pub fn claim(item: &WorkItem, owner: OwnerId, now: Timestamp, ttl: Duration) -> ClaimOutcome {
    if item.state.is_terminal() {
        return ClaimOutcome::Denied(ClaimDenial::Terminal(item.state));
    }
    if item.cancel_requested {
        return ClaimOutcome::Denied(ClaimDenial::CancelRequested);
    }
    if now.unix_millis() < item.due_at.unix_millis() {
        return ClaimOutcome::Denied(ClaimDenial::NotDue {
            due_at: item.due_at,
        });
    }
    if item.attempt >= item.max_attempts {
        return ClaimOutcome::Denied(ClaimDenial::AttemptsExhausted {
            attempt: item.attempt,
            max: item.max_attempts,
        });
    }
    let expires_at = advance(now, ttl);
    match item.lease {
        Some(existing) if existing.is_live_at(now) => {
            if existing.owner == owner {
                ClaimOutcome::Renewed(Lease {
                    owner,
                    fence: existing.fence,
                    expires_at,
                })
            } else {
                ClaimOutcome::Denied(ClaimDenial::HeldByOther {
                    owner: existing.owner,
                    expires_at: existing.expires_at,
                })
            }
        }
        Some(expired) => ClaimOutcome::Stolen {
            previous: expired.owner,
            lease: Lease {
                owner,
                fence: expired.fence.next(),
                expires_at,
            },
        },
        None => ClaimOutcome::Claimed(Lease {
            owner,
            fence: item.fence().next(),
            expires_at,
        }),
    }
}

/// Extends a lease the caller already holds.
///
/// The fence is preserved, so a renewal never invalidates the holder's own
/// in-flight writes.
///
/// # Errors
///
/// Returns [`FenceRejection`] on a wrong owner, a stale fence or an already
/// expired lease.
pub fn renew(
    item: &WorkItem,
    owner: OwnerId,
    fence: Fence,
    now: Timestamp,
    ttl: Duration,
) -> Result<Lease, FenceRejection> {
    let held = check_fence(item, owner, fence, now)?;
    Ok(Lease {
        owner,
        fence: held.fence,
        expires_at: advance(now, ttl),
    })
}

/// What a fenced step produced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StepOutcome {
    /// The step advanced and the operation should run again.
    Progressed {
        /// Where to resume.
        cursor: ContinuationCursor,
        /// When to run again.
        next_due: Timestamp,
        /// How far the operation has got.
        progress: Progress,
    },
    /// The operation is done.
    Finished(OperationResult),
    /// The step failed.
    Failed {
        /// How the failure should be treated.
        class: FailureClass,
        /// Why.
        error: OperationFailure,
    },
    /// The operation observed its cancel request.
    Cancelled,
}

/// The work item after a completed step.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkCommit {
    /// The item after the step.
    pub item: WorkItem,
    /// What the step produced.
    pub outcome: StepOutcome,
}

/// Records the result of a fenced step.
///
/// A `Failed { Retryable }` step that has used its last attempt becomes
/// [`WorkState::Poison`] and the operation `Failed { PoisonManualReview }`; it is
/// never redriven (D-10).
///
/// # Errors
///
/// Returns [`FenceRejection`] on a wrong owner, a stale fence or an expired
/// lease.
pub fn complete(
    item: &WorkItem,
    fence: Fence,
    outcome: StepOutcome,
    now: Timestamp,
) -> Result<WorkCommit, FenceRejection> {
    let held = item.lease.ok_or(FenceRejection::WrongOwner {
        current: None,
        presented: OwnerId(Uuid7::compose(0, [0; 10])),
    })?;
    check_fence(item, held.owner, fence, now)?;

    let mut next = item.clone();
    let mut settled = outcome;
    match &settled {
        StepOutcome::Progressed { next_due, .. } => {
            next.state = WorkState::Runnable;
            next.due_at = *next_due;
            next.lease = None;
            next.attempt = 0;
        }
        StepOutcome::Finished(_) | StepOutcome::Cancelled => {
            next.state = WorkState::Completed;
            next.lease = None;
        }
        StepOutcome::Failed { class, error } => match class {
            FailureClass::Retryable => {
                let attempt = next.attempt.saturating_add(1);
                next.attempt = attempt;
                next.lease = None;
                if attempt >= next.max_attempts {
                    next.state = WorkState::Poison;
                    settled = StepOutcome::Failed {
                        class: FailureClass::PoisonManualReview,
                        error: OperationFailure {
                            class: FailureClass::PoisonManualReview,
                            ..error.clone()
                        },
                    };
                } else {
                    next.state = WorkState::Runnable;
                }
            }
            FailureClass::Terminal | FailureClass::PoisonManualReview => {
                next.state = WorkState::Poison;
                next.lease = None;
            }
        },
    }
    Ok(WorkCommit {
        item: next,
        outcome: settled,
    })
}

/// Why a fenced write was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum FenceRejection {
    /// The presented fence is below the item's.
    #[error("presented fence {presented:?} is below the current {current:?}")]
    StaleFence {
        /// The item's fence.
        current: Fence,
        /// What the caller presented.
        presented: Fence,
    },
    /// The caller does not hold the item.
    #[error("presented owner {presented:?} does not hold the item (current {current:?})")]
    WrongOwner {
        /// Who holds it.
        current: Option<OwnerId>,
        /// Who asked.
        presented: OwnerId,
    },
    /// The lease had already lapsed.
    #[error("lease expired at {expired_at:?}")]
    LeaseExpired {
        /// When it lapsed.
        expired_at: Timestamp,
    },
}

fn check_fence(
    item: &WorkItem,
    owner: OwnerId,
    fence: Fence,
    now: Timestamp,
) -> Result<Lease, FenceRejection> {
    let held = item.lease.ok_or(FenceRejection::WrongOwner {
        current: None,
        presented: owner,
    })?;
    if held.owner != owner {
        return Err(FenceRejection::WrongOwner {
            current: Some(held.owner),
            presented: owner,
        });
    }
    if fence != held.fence {
        return Err(FenceRejection::StaleFence {
            current: held.fence,
            presented: fence,
        });
    }
    if !held.is_live_at(now) {
        return Err(FenceRejection::LeaseExpired {
            expired_at: held.expires_at,
        });
    }
    Ok(held)
}

fn advance(now: Timestamp, ttl: Duration) -> Timestamp {
    let millis = now
        .unix_millis()
        .saturating_add(i64::try_from(ttl.whole_milliseconds()).unwrap_or(i64::MAX));
    Timestamp::from_unix_millis(millis)
        .unwrap_or_else(|_| unreachable!("a bounded lease TTL keeps the instant in range"))
}

#[cfg(test)]
mod tests {
    use aex_wire::ids::{OperationId, PrefixedId as _, Uuid7};
    use aex_wire::types::Timestamp;
    use time::Duration;

    use super::{
        ClaimDenial, ClaimOutcome, DedupIdentity, Fence, FenceRejection, Lease, OwnerId,
        StepOutcome, WorkId, WorkItem, WorkState, claim, complete, renew,
    };
    use crate::due::DueShard;
    use crate::operation::{FailureClass, OperationFailure, OperationKind, OperationResult};

    fn moment(millis: i64) -> Timestamp {
        Timestamp::from_unix_millis(millis).expect("in range")
    }

    fn owner(tag: u8) -> OwnerId {
        OwnerId(Uuid7::compose(1, [tag; 10]))
    }

    fn item() -> WorkItem {
        let operation = OperationId::from_uuid7(Uuid7::compose(1, [1; 10]));
        WorkItem {
            id: WorkId(Uuid7::compose(1, [2; 10])),
            operation,
            kind: OperationKind::ContentGc,
            shard: DueShard(0),
            due_at: moment(0),
            priority: 0,
            attempt: 0,
            max_attempts: 3,
            lease: None,
            state: WorkState::Runnable,
            dedup: DedupIdentity { operation, step: 0 },
            cancel_requested: false,
        }
    }

    #[test]
    fn a_first_claim_advances_the_fence_and_a_renew_does_not() {
        let ClaimOutcome::Claimed(lease) =
            claim(&item(), owner(1), moment(0), Duration::seconds(30))
        else {
            panic!("expected a claim");
        };
        assert_eq!(lease.fence, Fence(1));

        let mut held = item();
        held.state = WorkState::Claimed;
        held.lease = Some(lease);
        let renewed =
            renew(&held, owner(1), Fence(1), moment(1), Duration::seconds(30)).expect("renews");
        assert_eq!(renewed.fence, Fence(1));
        assert_eq!(renewed.expires_at, moment(30_001));
    }

    #[test]
    fn a_live_lease_denies_another_owner_and_an_expired_one_is_stolen() {
        let mut held = item();
        held.state = WorkState::Claimed;
        held.lease = Some(Lease {
            owner: owner(1),
            fence: Fence(4),
            expires_at: moment(1_000),
        });
        assert_eq!(
            claim(&held, owner(2), moment(999), Duration::seconds(30)),
            ClaimOutcome::Denied(ClaimDenial::HeldByOther {
                owner: owner(1),
                expires_at: moment(1_000)
            })
        );
        let ClaimOutcome::Stolen { previous, lease } =
            claim(&held, owner(2), moment(1_000), Duration::seconds(30))
        else {
            panic!("expected a steal at the expiry instant");
        };
        assert_eq!(previous, owner(1));
        assert_eq!(lease.fence, Fence(5));
    }

    #[test]
    fn a_stale_fence_is_rejected_and_writes_nothing() {
        let mut held = item();
        held.state = WorkState::Claimed;
        held.lease = Some(Lease {
            owner: owner(1),
            fence: Fence(4),
            expires_at: moment(1_000),
        });
        assert_eq!(
            complete(
                &held,
                Fence(3),
                StepOutcome::Finished(OperationResult::receipt()),
                moment(1)
            ),
            Err(FenceRejection::StaleFence {
                current: Fence(4),
                presented: Fence(3)
            })
        );
        assert_eq!(
            renew(&held, owner(2), Fence(4), moment(1), Duration::seconds(1)),
            Err(FenceRejection::WrongOwner {
                current: Some(owner(1)),
                presented: owner(2)
            })
        );
    }

    #[test]
    fn the_last_retryable_attempt_becomes_poison() {
        let mut held = item();
        held.state = WorkState::Claimed;
        held.attempt = 2;
        held.lease = Some(Lease {
            owner: owner(1),
            fence: Fence(1),
            expires_at: moment(1_000),
        });
        let commit = complete(
            &held,
            Fence(1),
            StepOutcome::Failed {
                class: FailureClass::Retryable,
                error: OperationFailure::bare(
                    aex_wire::error::ErrorCode::InternalError,
                    FailureClass::Retryable,
                ),
            },
            moment(1),
        )
        .expect("completes");
        assert_eq!(commit.item.state, WorkState::Poison);
        assert!(matches!(
            commit.outcome,
            StepOutcome::Failed {
                class: FailureClass::PoisonManualReview,
                ..
            }
        ));
        assert_eq!(
            claim(&commit.item, owner(1), moment(2), Duration::seconds(1)),
            ClaimOutcome::Denied(ClaimDenial::Terminal(WorkState::Poison))
        );
    }
}
