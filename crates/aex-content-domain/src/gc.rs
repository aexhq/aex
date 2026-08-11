//! The sweep decision.
//!
//! `sweep_decision` is a pure predicate. It never deletes anything: it returns
//! either a surviving retain reason, a grace deadline, or the exact conditions
//! the lifecycle worker must re-evaluate strongly consistently immediately
//! before deleting one exact unversioned key.

use aex_wire::ids::WorkspaceId;
use aex_wire::types::Timestamp;
use time::Duration;

use crate::digest::ContentDigest;
use crate::pin::{Pin, PinSet};
use crate::placement::ContentObjectKey;

/// How long a staged, unreferenced body is kept before it may be swept, in
/// milliseconds. The single source of the window; [`STAGED_ORPHAN_GRACE`] is
/// derived from it.
pub const STAGED_ORPHAN_GRACE_MS: i64 = 24 * 60 * 60 * 1_000;

/// How long a staged, unreferenced body is kept before it may be swept.
pub const STAGED_ORPHAN_GRACE: Duration = Duration::milliseconds(STAGED_ORPHAN_GRACE_MS);

/// The garbage-collection epoch a decision was computed under.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct GcEpoch(pub u64);

impl GcEpoch {
    /// The epoch a fresh workspace starts at.
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
            None => panic!("gc epoch overflowed"),
        }
    }
}

/// Why a body survives a sweep.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum RetainReason {
    /// A registry pointer reaches it.
    RegistryPin,
    /// An unexpired download grant reaches it.
    GrantPin,
    /// A garbage-collection pin holds it.
    GcPin,
    /// The authority has already moved past the epoch the decision assumed.
    NewerEpoch {
        /// The epoch the authority is on now.
        current: GcEpoch,
    },
}

/// One condition the lifecycle worker must re-evaluate before deleting.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum GcCondition {
    /// The workspace's epoch still equals the epoch the decision was computed
    /// under.
    EpochUnchanged {
        /// The owning workspace.
        workspace: WorkspaceId,
        /// The expected epoch.
        expected: GcEpoch,
    },
    /// No direct pin reaches the body.
    NoDirectPin {
        /// The owning workspace.
        workspace: WorkspaceId,
        /// The body.
        digest: ContentDigest,
    },
    /// The exact unversioned object key is still the one the decision saw.
    ObjectKeyUnchanged {
        /// The key that will be deleted.
        key: ContentObjectKey,
    },
}

/// What a sweep decides about one body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SweepDecision {
    /// The body survives.
    Retain(RetainReason),
    /// The body is unreferenced but still inside its grace window.
    WaitGrace {
        /// The instant the grace window closes.
        until: Timestamp,
    },
    /// The body may be deleted once these conditions still hold.
    DeleteUnderFence {
        /// The conditions, in canonical order.
        conditions: Vec<GcCondition>,
    },
}

/// One body a sweep is considering.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SweepCandidate {
    /// The owning workspace.
    pub workspace: WorkspaceId,
    /// The body.
    pub digest: ContentDigest,
    /// Its unversioned object key, when it is stored as an object.
    pub key: Option<ContentObjectKey>,
    /// When the body was first staged.
    pub staged_at: Timestamp,
    /// Which epoch the authority was on when the candidate was collected.
    pub observed_epoch: GcEpoch,
    /// Every direct authority the candidate is reachable from.
    pub reachable_from: Vec<ReachableFrom>,
}

/// One reachability edge the caller computed for a candidate.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ReachableFrom {
    /// A pin reaches the body directly.
    Direct(Pin),
}

impl ReachableFrom {
    /// The pin supplying the reachability.
    #[must_use]
    pub const fn pin(&self) -> &Pin {
        match self {
            Self::Direct(pin) => pin,
        }
    }
}

/// Decides the fate of one candidate.
///
/// The order is fixed and total: a stale epoch is reported first because a
/// decision computed under an older epoch can never authorize a deletion; then
/// any live pin retains; then the grace window; only then is deletion possible,
/// and even then only under an explicit fence.
#[must_use]
pub fn sweep_decision(
    candidate: &SweepCandidate,
    epoch: GcEpoch,
    pins: &PinSet,
    now: Timestamp,
) -> SweepDecision {
    if candidate.observed_epoch < epoch {
        return SweepDecision::Retain(RetainReason::NewerEpoch { current: epoch });
    }

    let mut retained: Option<RetainReason> = None;
    for edge in &candidate.reachable_from {
        let pin = edge.pin();
        if !pin.is_live_at(now) || !pins.iter().any(|held| held == pin) {
            continue;
        }
        let reason = pin.retain_reason();
        retained = Some(match retained {
            Some(existing) if existing <= reason => existing,
            _ => reason,
        });
    }
    if let Some(reason) = retained {
        return SweepDecision::Retain(reason);
    }

    let grace_end = candidate.staged_at.unix_millis() + STAGED_ORPHAN_GRACE_MS;
    if now.unix_millis() < grace_end {
        let until = Timestamp::from_unix_millis(grace_end)
            .unwrap_or_else(|_| unreachable!("a staged instant plus 24 hours is in range"));
        return SweepDecision::WaitGrace { until };
    }

    let mut conditions = vec![
        GcCondition::EpochUnchanged {
            workspace: candidate.workspace,
            expected: epoch,
        },
        GcCondition::NoDirectPin {
            workspace: candidate.workspace,
            digest: candidate.digest,
        },
    ];
    if let Some(key) = &candidate.key {
        conditions.push(GcCondition::ObjectKeyUnchanged { key: key.clone() });
    }
    SweepDecision::DeleteUnderFence { conditions }
}

#[cfg(test)]
mod tests {
    use aex_wire::ids::{PrefixedId as _, Uuid7, WorkspaceId};
    use aex_wire::types::Timestamp;

    use super::{
        GcEpoch, ReachableFrom, RetainReason, STAGED_ORPHAN_GRACE_MS, SweepCandidate,
        SweepDecision, sweep_decision,
    };
    use crate::digest::ContentDigest;
    use crate::pin::{Pin, PinSet};

    fn moment(millis: i64) -> Timestamp {
        Timestamp::from_unix_millis(millis).expect("in range")
    }

    fn candidate(reachable: Vec<ReachableFrom>) -> SweepCandidate {
        SweepCandidate {
            workspace: WorkspaceId::from_uuid7(Uuid7::compose(1, [1; 10])),
            digest: ContentDigest::of(b"body"),
            key: None,
            staged_at: moment(0),
            observed_epoch: GcEpoch(3),
            reachable_from: reachable,
        }
    }

    fn grant_pin() -> Pin {
        Pin::Grant {
            grant: crate::GrantId(Uuid7::compose(1, [2; 10])),
            digest: ContentDigest::of(b"body"),
            expires_at: moment(STAGED_ORPHAN_GRACE_MS * 2),
        }
    }

    #[test]
    fn a_stale_epoch_never_authorizes_a_deletion() {
        let decision = sweep_decision(
            &candidate(Vec::new()),
            GcEpoch(4),
            &PinSet::new(),
            moment(i64::from(u32::MAX)),
        );
        assert_eq!(
            decision,
            SweepDecision::Retain(RetainReason::NewerEpoch {
                current: GcEpoch(4)
            })
        );
    }

    #[test]
    fn a_live_pin_retains() {
        let pins: PinSet = [grant_pin()].into_iter().collect();
        let decision = sweep_decision(
            &candidate(vec![ReachableFrom::Direct(grant_pin())]),
            GcEpoch(3),
            &pins,
            moment(0),
        );
        assert_eq!(decision, SweepDecision::Retain(RetainReason::GrantPin));
    }

    #[test]
    fn an_unpinned_body_waits_out_its_grace_then_deletes_under_a_fence() {
        let grace = STAGED_ORPHAN_GRACE_MS;
        let waiting = sweep_decision(
            &candidate(Vec::new()),
            GcEpoch(3),
            &PinSet::new(),
            moment(grace - 1),
        );
        assert_eq!(
            waiting,
            SweepDecision::WaitGrace {
                until: moment(grace)
            }
        );
        let sweeping = sweep_decision(
            &candidate(Vec::new()),
            GcEpoch(3),
            &PinSet::new(),
            moment(grace),
        );
        assert!(matches!(sweeping, SweepDecision::DeleteUnderFence { .. }));
    }
}
