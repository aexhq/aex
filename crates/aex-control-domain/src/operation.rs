//! Durable operations.
//!
//! Two kinds exist on the central plane and **neither is cancelable**.
//! `POST /api/operations/{op}/cancellations` therefore always answers
//! `409 operation_not_cancelable`. That is stated, tested and intentional,
//! rather than the system this replaces where the route could only ever `409`
//! by accident.
//!
//! A lease plus a fence is what makes a worker crash safe: a claim takes a
//! bounded lease and advances the fence, and any write carrying a fence older
//! than the row's is refused rather than merged.

use time::{Duration, OffsetDateTime};
use uuid::Uuid;

use crate::intent::IntentHash;
use crate::scope::ScopeSet;

/// A monotone guard against a stale writer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Fence(u64);

impl Fence {
    /// The fence a freshly inserted operation carries.
    pub const FIRST: Self = Self(1);

    /// Wraps a stored fence.
    #[must_use]
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    /// The stored value.
    #[must_use]
    pub const fn get(self) -> u64 {
        self.0
    }

    /// The next fence.
    #[must_use]
    pub const fn advance(self) -> Self {
        Self(self.0.saturating_add(1))
    }
}

/// Who holds a lease.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct LeaseOwner(String);

impl LeaseOwner {
    /// Wraps a worker identity.
    #[must_use]
    pub fn new(raw: impl Into<String>) -> Self {
        Self(raw.into())
    }

    /// The identity as a string slice.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// A bounded claim on an operation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Lease {
    /// Who holds it.
    pub owner: LeaseOwner,
    /// When it lapses.
    pub expires_at: OffsetDateTime,
}

/// What an operation does.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum OperationKind {
    /// Create the regional half of a workspace.
    WorkspaceProvision,
    /// Remove the regional half of a workspace.
    WorkspaceDelete,
}

/// Whether an operation appears in a public read.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum OperationVisibility {
    /// Returned by `GET /api/operations`.
    Public,
    /// A reconciliation anchor only.
    Internal,
}

impl OperationVisibility {
    /// The database spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Public => "public",
            Self::Internal => "internal",
        }
    }
}

impl OperationKind {
    /// Every kind.
    pub const ALL: [Self; 2] = [Self::WorkspaceProvision, Self::WorkspaceDelete];

    /// The database spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::WorkspaceProvision => "workspace_provision",
            Self::WorkspaceDelete => "workspace_delete",
        }
    }

    /// Resolves a database spelling.
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|it| it.as_str() == text)
    }

    /// Whether the kind appears in a public read.
    ///
    /// Workspace creation returns `201` with the workspace, not `202` with an
    /// operation, so its operation is an internal reconciliation anchor.
    #[must_use]
    pub const fn visibility(self) -> OperationVisibility {
        match self {
            Self::WorkspaceProvision => OperationVisibility::Internal,
            Self::WorkspaceDelete => OperationVisibility::Public,
        }
    }

    /// Whether the kind may be cancelled once accepted.
    ///
    /// Both central kinds are `false`: acceptance has already closed admission
    /// and revoked keys, so there is nothing left that cancelling could undo.
    #[must_use]
    pub const fn cancelable_on_accept(self) -> bool {
        match self {
            Self::WorkspaceProvision | Self::WorkspaceDelete => false,
        }
    }
}

/// Where an operation is in its life.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum OperationStatus {
    /// Accepted, not started.
    Queued,
    /// Claimed by a worker.
    Running,
    /// Finished, with a result.
    Succeeded,
    /// Finished, with an error.
    Failed,
    /// Finished, by cancellation. Unreachable for both central kinds.
    Cancelled,
}

impl OperationStatus {
    /// Every status.
    pub const ALL: [Self; 5] = [
        Self::Queued,
        Self::Running,
        Self::Succeeded,
        Self::Failed,
        Self::Cancelled,
    ];

    /// The database spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Running => "running",
            Self::Succeeded => "succeeded",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
        }
    }

    /// Resolves a database spelling.
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|it| it.as_str() == text)
    }

    /// Whether the status can never change again.
    #[must_use]
    pub const fn is_terminal(self) -> bool {
        matches!(self, Self::Succeeded | Self::Failed | Self::Cancelled)
    }
}

/// A durable operation record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Operation {
    /// Public id payload.
    pub id: Uuid,
    /// What it does.
    pub kind: OperationKind,
    /// Whether a public read returns it.
    pub visibility: OperationVisibility,
    /// The organization it acts in.
    pub organization_id: Uuid,
    /// The workspace it acts on, when there is one.
    pub workspace_id: Option<Uuid>,
    /// The principal that admitted it.
    pub principal_id: Uuid,
    /// The effective scopes at admission, recorded so a later read of the
    /// operation can be authorized against the scope it was admitted under.
    pub scopes: ScopeSet,
    /// Where it is.
    pub status: OperationStatus,
    /// The intent it was admitted for.
    pub intent_hash: IntentHash,
    /// The current fence.
    pub fence: Fence,
    /// How many claims it has had.
    pub attempt: u32,
    /// The current lease, when claimed.
    pub lease: Option<Lease>,
    /// When it was accepted.
    pub created_at: OffsetDateTime,
    /// When it was first claimed.
    pub started_at: Option<OffsetDateTime>,
    /// When it last changed.
    pub updated_at: OffsetDateTime,
    /// When it reached a terminal status.
    pub terminal_at: Option<OffsetDateTime>,
    /// When the scheduler should look at it again.
    pub due_at: Option<OffsetDateTime>,
}

/// Why an operation transition was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum OperationTransition {
    /// The operation already reached a terminal status.
    #[error("the operation is already terminal")]
    AlreadyTerminal,
    /// Another worker holds an unexpired lease.
    #[error("another worker holds the lease")]
    Leased,
    /// The renewing worker is not the lease holder.
    #[error("the lease belongs to another worker")]
    NotLeaseHolder,
    /// The supplied fence does not match the row's.
    #[error("fence {supplied} does not match the recorded {recorded}")]
    StaleFence {
        /// What the caller supplied.
        supplied: u64,
        /// What the row holds.
        recorded: u64,
    },
    /// The kind is not cancelable.
    #[error("a `{kind}` operation cannot be cancelled once accepted", kind = .kind.as_str())]
    NotCancelable {
        /// Which kind refused.
        kind: OperationKind,
    },
    /// The attempt ceiling was reached.
    #[error("the operation reached its attempt ceiling of {ceiling}")]
    AttemptCeiling {
        /// The ceiling.
        ceiling: u32,
    },
}

impl Operation {
    /// The most claims one operation may have.
    pub const MAX_ATTEMPTS: u32 = 100;

    /// Claims the operation for `owner`.
    ///
    /// # Errors
    ///
    /// Returns [`OperationTransition::AlreadyTerminal`] for a finished
    /// operation, [`OperationTransition::Leased`] while another worker's lease
    /// is live, and [`OperationTransition::AttemptCeiling`] at the ceiling.
    pub fn claim(
        &self,
        owner: LeaseOwner,
        now: OffsetDateTime,
        lease: Duration,
    ) -> Result<Self, OperationTransition> {
        if self.status.is_terminal() {
            return Err(OperationTransition::AlreadyTerminal);
        }
        if let Some(current) = &self.lease
            && current.expires_at > now
            && current.owner != owner
        {
            return Err(OperationTransition::Leased);
        }
        if self.attempt >= Self::MAX_ATTEMPTS {
            return Err(OperationTransition::AttemptCeiling {
                ceiling: Self::MAX_ATTEMPTS,
            });
        }
        Ok(Self {
            status: OperationStatus::Running,
            fence: self.fence.advance(),
            attempt: self.attempt + 1,
            lease: Some(Lease {
                owner,
                expires_at: now + lease,
            }),
            started_at: self.started_at.or(Some(now)),
            updated_at: now,
            due_at: Some(now + lease),
            ..self.clone()
        })
    }

    /// Extends the current lease.
    ///
    /// # Errors
    ///
    /// Returns [`OperationTransition::AlreadyTerminal`] for a finished
    /// operation and [`OperationTransition::NotLeaseHolder`] for anyone but the
    /// holder. Renewal does **not** advance the fence: it is the same attempt.
    pub fn renew(
        &self,
        owner: &LeaseOwner,
        now: OffsetDateTime,
        lease: Duration,
    ) -> Result<Self, OperationTransition> {
        if self.status.is_terminal() {
            return Err(OperationTransition::AlreadyTerminal);
        }
        let held = self
            .lease
            .as_ref()
            .filter(|current| current.owner == *owner)
            .ok_or(OperationTransition::NotLeaseHolder)?;
        if held.expires_at <= now {
            return Err(OperationTransition::NotLeaseHolder);
        }
        Ok(Self {
            lease: Some(Lease {
                owner: owner.clone(),
                expires_at: now + lease,
            }),
            updated_at: now,
            due_at: Some(now + lease),
            ..self.clone()
        })
    }

    /// Marks the operation succeeded.
    ///
    /// # Errors
    ///
    /// Returns [`OperationTransition::AlreadyTerminal`] for a finished
    /// operation and [`OperationTransition::StaleFence`] for a writer whose
    /// claim has since been stolen.
    pub fn succeed(&self, fence: Fence, now: OffsetDateTime) -> Result<Self, OperationTransition> {
        self.finish(OperationStatus::Succeeded, fence, now)
    }

    /// Marks the operation failed.
    ///
    /// # Errors
    ///
    /// Identical to [`Operation::succeed`].
    pub fn fail(&self, fence: Fence, now: OffsetDateTime) -> Result<Self, OperationTransition> {
        self.finish(OperationStatus::Failed, fence, now)
    }

    /// The shared terminal transition.
    fn finish(
        &self,
        status: OperationStatus,
        fence: Fence,
        now: OffsetDateTime,
    ) -> Result<Self, OperationTransition> {
        if self.status.is_terminal() {
            return Err(OperationTransition::AlreadyTerminal);
        }
        if fence != self.fence {
            return Err(OperationTransition::StaleFence {
                supplied: fence.get(),
                recorded: self.fence.get(),
            });
        }
        Ok(Self {
            status,
            lease: None,
            terminal_at: Some(now),
            updated_at: now,
            due_at: None,
            ..self.clone()
        })
    }

    /// Attempts to cancel.
    ///
    /// # Errors
    ///
    /// Always returns [`OperationTransition::NotCancelable`] for both central
    /// kinds, or [`OperationTransition::AlreadyTerminal`] when the operation has
    /// already finished. There is no success path; the route is honest about it.
    pub fn cancel(&self, _now: OffsetDateTime) -> Result<Self, OperationTransition> {
        if self.status.is_terminal() {
            return Err(OperationTransition::AlreadyTerminal);
        }
        if self.kind.cancelable_on_accept() {
            unreachable!("no central operation kind is cancelable on accept");
        }
        Err(OperationTransition::NotCancelable { kind: self.kind })
    }
}

#[cfg(test)]
mod tests {
    use super::{
        Fence, LeaseOwner, Operation, OperationKind, OperationStatus, OperationTransition,
        OperationVisibility,
    };
    use crate::intent::IntentHash;
    use crate::scope::ScopeSet;
    use time::{Duration, OffsetDateTime};
    use uuid::Uuid;

    fn operation(kind: OperationKind) -> Operation {
        Operation {
            id: Uuid::from_u128(1),
            kind,
            visibility: kind.visibility(),
            organization_id: Uuid::from_u128(2),
            workspace_id: Some(Uuid::from_u128(3)),
            principal_id: Uuid::from_u128(4),
            scopes: ScopeSet::MEMBER,
            status: OperationStatus::Queued,
            intent_hash: IntentHash::from_bytes([0_u8; 32]),
            fence: Fence::FIRST,
            attempt: 0,
            lease: None,
            created_at: OffsetDateTime::UNIX_EPOCH,
            started_at: None,
            updated_at: OffsetDateTime::UNIX_EPOCH,
            terminal_at: None,
            due_at: None,
        }
    }

    #[test]
    fn neither_central_kind_is_cancelable() {
        for kind in OperationKind::ALL {
            assert!(!kind.cancelable_on_accept(), "{kind:?}");
            assert_eq!(
                operation(kind).cancel(OffsetDateTime::UNIX_EPOCH),
                Err(OperationTransition::NotCancelable { kind })
            );
        }
    }

    #[test]
    fn provisioning_is_internal_and_deletion_is_public() {
        assert_eq!(
            OperationKind::WorkspaceProvision.visibility(),
            OperationVisibility::Internal
        );
        assert_eq!(
            OperationKind::WorkspaceDelete.visibility(),
            OperationVisibility::Public
        );
    }

    #[test]
    fn a_claim_advances_the_fence_and_a_renewal_does_not() {
        let now = OffsetDateTime::UNIX_EPOCH;
        let lease = Duration::seconds(60);
        let owner = LeaseOwner::new("worker-a");
        let claimed = operation(OperationKind::WorkspaceDelete)
            .claim(owner.clone(), now, lease)
            .expect("a queued operation is claimable");
        assert_eq!(claimed.status, OperationStatus::Running);
        assert_eq!(claimed.fence, Fence::FIRST.advance());
        assert_eq!(claimed.attempt, 1);

        let renewed = claimed
            .renew(&owner, now + Duration::seconds(30), lease)
            .expect("the holder renews");
        assert_eq!(renewed.fence, claimed.fence, "renewal is the same attempt");
        assert_eq!(renewed.attempt, claimed.attempt);
    }

    #[test]
    fn a_live_lease_blocks_another_worker_and_a_lapsed_one_does_not() {
        let now = OffsetDateTime::UNIX_EPOCH;
        let lease = Duration::seconds(60);
        let claimed = operation(OperationKind::WorkspaceDelete)
            .claim(LeaseOwner::new("worker-a"), now, lease)
            .expect("first claim");
        assert_eq!(
            claimed.claim(
                LeaseOwner::new("worker-b"),
                now + Duration::seconds(30),
                lease
            ),
            Err(OperationTransition::Leased)
        );
        let stolen = claimed
            .claim(
                LeaseOwner::new("worker-b"),
                now + Duration::seconds(61),
                lease,
            )
            .expect("a lapsed lease is stealable");
        assert_eq!(stolen.fence, claimed.fence.advance());
        assert_eq!(stolen.attempt, 2);
    }

    #[test]
    fn a_stolen_claim_makes_the_old_workers_completion_stale() {
        let now = OffsetDateTime::UNIX_EPOCH;
        let lease = Duration::seconds(60);
        let claimed = operation(OperationKind::WorkspaceDelete)
            .claim(LeaseOwner::new("worker-a"), now, lease)
            .expect("first claim");
        let stolen = claimed
            .claim(
                LeaseOwner::new("worker-b"),
                now + Duration::seconds(61),
                lease,
            )
            .expect("steal");
        assert_eq!(
            stolen.succeed(claimed.fence, now + Duration::seconds(70)),
            Err(OperationTransition::StaleFence {
                supplied: claimed.fence.get(),
                recorded: stolen.fence.get()
            })
        );
        assert!(
            stolen
                .succeed(stolen.fence, now + Duration::seconds(70))
                .is_ok()
        );
    }

    #[test]
    fn a_terminal_operation_never_re_enters_a_non_terminal_status() {
        let now = OffsetDateTime::UNIX_EPOCH;
        let done = operation(OperationKind::WorkspaceDelete)
            .succeed(Fence::FIRST, now)
            .expect("a queued operation can settle");
        assert!(done.status.is_terminal());
        for outcome in [
            done.claim(LeaseOwner::new("w"), now, Duration::seconds(1)),
            done.renew(&LeaseOwner::new("w"), now, Duration::seconds(1)),
            done.succeed(done.fence, now),
            done.fail(done.fence, now),
            done.cancel(now),
        ] {
            assert_eq!(outcome, Err(OperationTransition::AlreadyTerminal));
        }
    }

    #[test]
    fn a_non_holder_cannot_renew() {
        let now = OffsetDateTime::UNIX_EPOCH;
        let claimed = operation(OperationKind::WorkspaceDelete)
            .claim(LeaseOwner::new("worker-a"), now, Duration::seconds(60))
            .expect("claim");
        assert_eq!(
            claimed.renew(&LeaseOwner::new("worker-b"), now, Duration::seconds(60)),
            Err(OperationTransition::NotLeaseHolder)
        );
    }

    #[test]
    fn the_attempt_ceiling_stops_a_hot_retry_loop() {
        let now = OffsetDateTime::UNIX_EPOCH;
        let mut operation = operation(OperationKind::WorkspaceDelete);
        operation.attempt = Operation::MAX_ATTEMPTS;
        assert_eq!(
            operation.claim(LeaseOwner::new("w"), now, Duration::seconds(60)),
            Err(OperationTransition::AttemptCeiling {
                ceiling: Operation::MAX_ATTEMPTS
            })
        );
    }

    #[test]
    fn the_status_and_kind_spellings_round_trip() {
        for status in OperationStatus::ALL {
            assert_eq!(OperationStatus::parse(status.as_str()), Some(status));
        }
        for kind in OperationKind::ALL {
            assert_eq!(OperationKind::parse(kind.as_str()), Some(kind));
        }
        assert_eq!(OperationStatus::parse("paused"), None);
        assert_eq!(OperationKind::parse("session_delete"), None);
    }
}
