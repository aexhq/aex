//! Condition and update expressions, built as data.
//!
//! Every transaction the Brain performs is described here as a value before any AWS type is
//! constructed. That is what makes the shapes testable without a network call, and it is
//! why the precondition sets below can be asserted against the plan's table rather than
//! read out of a request builder at review time.

use aex_brain_domain::commit::FenceGuardRef;
use aex_brain_domain::ids::{EffectId, Timestamp};

/// One `DynamoDB` action inside a transaction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Action {
    /// Which table.
    pub table: Table,
    /// Partition key.
    pub partition: String,
    /// Sort key.
    pub sort: String,
    /// What the action does.
    pub kind: ActionKind,
    /// Every precondition, in a stable order.
    pub conditions: Vec<Condition>,
}

/// Which of the two participating tables an action touches.
///
/// Exactly two. Adding a third would make the Brain's decision a distributed transaction
/// across another owner's authority, which is precisely what the item-ownership rule
/// exists to prevent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Table {
    /// Journals, control, effects, children, joins, budgets, mailboxes and previews.
    SessionAuthority,
    /// Wake and due items only, written through the work adapter's builders.
    RegionalWork,
}

/// What an action does to an item.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ActionKind {
    /// Create an item that must not already exist.
    Put,
    /// Modify an item in place.
    Update,
    /// Remove an item.
    Delete,
    /// Assert a condition without writing. Used to check a limit an action does not own.
    ConditionCheck,
}

/// One precondition on an action.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Condition {
    /// The item must not exist. This is what makes a redelivered decision idempotent: the
    /// second attempt fails the journal put rather than appending a duplicate record.
    NotExists,
    /// The item must exist.
    Exists,
    /// An attribute must equal a value.
    Equals {
        /// Which attribute.
        attribute: &'static str,
        /// The expected value, rendered.
        value: String,
    },
    /// An attribute must be strictly less than a value.
    LessThan {
        /// Which attribute.
        attribute: &'static str,
        /// The bound, rendered.
        value: String,
    },
    /// An attribute must not equal a value.
    NotEquals {
        /// Which attribute.
        attribute: &'static str,
        /// The excluded value, rendered.
        value: String,
    },
    /// The sum of two attributes plus a delta must stay within a third.
    ///
    /// This is how the scheduler's claim asserts the session active limit without reading
    /// it first: the check and the increment are the same transaction, so no window exists
    /// in which two claimants both believe there is room.
    SumWithin {
        /// The accumulating attribute.
        used: &'static str,
        /// How much this action adds.
        delta: u64,
        /// The limiting attribute.
        limit: &'static str,
    },
}

/// The full precondition set every activation write carries.
///
/// All five, always. Each rejects a different way of being wrong — a stale plan, a lost
/// agent, an expired lease, a cancelled run, a moved journal — and dropping any one of them
/// lets that particular stale writer publish.
#[must_use]
pub fn activation_preconditions(guard: &FenceGuardRef) -> Vec<Condition> {
    vec![
        Condition::Equals {
            attribute: "revision",
            value: guard.revision.0.to_string(),
        },
        Condition::Equals {
            attribute: "fence",
            value: guard.fence.0.to_string(),
        },
        Condition::Equals {
            attribute: "lease_owner",
            value: guard.owner.0.as_hyphenated().to_string(),
        },
        Condition::Equals {
            attribute: "cancel_epoch",
            value: guard.cancel_epoch.0.to_string(),
        },
        Condition::Equals {
            attribute: "journal_tail",
            value: guard
                .tail
                .map_or_else(|| "null".to_owned(), |tail| tail.get().to_string()),
        },
    ]
}

/// The condition a claim is admitted under.
///
/// A claimant may take an expired lease only once the grace has also elapsed. Clock skew
/// therefore changes *when* a steal happens, never *whether* a stale owner can still write:
/// the fence decides that, and it decides it without reference to any clock.
#[must_use]
pub fn claim_conditions(
    now: Timestamp,
    steal_grace: core::time::Duration,
    self_owner: Option<&str>,
) -> Vec<Condition> {
    let grace_millis = i64::try_from(steal_grace.as_millis()).unwrap_or(i64::MAX);
    let stealable_before = now.millis().saturating_sub(grace_millis);
    let mut conditions = vec![Condition::LessThan {
        attribute: "lease_expires_at",
        value: stealable_before.to_string(),
    }];
    if let Some(owner) = self_owner {
        // Re-claiming your own agent does not need to wait out your own lease.
        conditions.push(Condition::Equals {
            attribute: "lease_owner",
            value: owner.to_owned(),
        });
    }
    conditions
}

/// The condition `mark_dispatch_started` is admitted under.
///
/// Both halves matter: the effect must still be `Prepared`, so one pre-send write
/// authorizes one attempt, and the agent's fence must still be ours, so a fenced-out owner
/// cannot mint a ticket for work it is no longer allowed to do.
#[must_use]
pub fn dispatch_started_conditions(effect: EffectId, fence: u64) -> Vec<Condition> {
    vec![
        Condition::Equals {
            attribute: "state",
            value: "prepared".to_owned(),
        },
        Condition::Equals {
            attribute: "effect_id",
            value: effect.to_hex(),
        },
        Condition::Equals {
            attribute: "agent_fence",
            value: fence.to_string(),
        },
    ]
}

/// The condition `mark_response_started` is admitted under.
#[must_use]
pub fn response_started_conditions() -> Vec<Condition> {
    vec![Condition::Equals {
        attribute: "state",
        value: "dispatch_started".to_owned(),
    }]
}

/// The conditions the scheduler's child claim is admitted under.
#[must_use]
pub fn claim_child_conditions(cancel_epoch: u64, observed_fence: u64) -> Vec<Condition> {
    vec![
        Condition::SumWithin {
            used: "active_used",
            delta: 1,
            limit: "active_limit",
        },
        Condition::Equals {
            attribute: "cancel_epoch",
            value: cancel_epoch.to_string(),
        },
        Condition::Equals {
            attribute: "state",
            value: "queued".to_owned(),
        },
        Condition::Equals {
            attribute: "fence",
            value: observed_fence.to_string(),
        },
    ]
}

/// The condition a fenced stop is admitted under.
///
/// A stop carries the fence the caller *observed*. A stale one is rejected rather than
/// applied, because a stop aimed at a previous incarnation of the child would otherwise
/// cancel work the caller has never seen.
#[must_use]
pub fn stop_conditions(observed_fence: u64) -> Vec<Condition> {
    vec![Condition::Equals {
        attribute: "fence",
        value: observed_fence.to_string(),
    }]
}

/// Cross-stream seam for the `regional-work` item shape.
///
/// `TODO(cross-stream)`: `aex-work-dynamodb` publishes no expression builders. Its
/// modules are `claim`, `codec`, `keys` and `store`, and the item shape is reachable only
/// through the `store::WorkAuthority` trait, which performs its own call.
/// The Brain writes wake and due items **inside its own transaction**, so it needs the
/// expressions as data rather than a store that performs its own call. Owning the trait
/// here rather than forking the item shape means the swap is one impl, not a rewrite.
pub trait WorkExpressions: Send + Sync {
    /// The action that creates one wake item.
    fn wake_put(&self, wake: &WakeItem) -> Action;

    /// The action that removes one wake item.
    fn wake_delete(&self, wake: &WakeItem) -> Action;

    /// The key condition for one due-shard query.
    fn due_shard_query(&self, shard: u16, due_before: Timestamp) -> Vec<Condition>;
}

/// The wake payload the Brain publishes.
///
/// A published cross-stream artifact: `regional-stream`, the work adapter and
/// `session-operation-worker` all read it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WakeItem {
    /// Which agent to wake, rendered as its partition key.
    pub agent_partition: String,
    /// The key that collapses duplicate receipts before admission.
    pub dedup_key: String,
    /// A stable reason tag.
    pub reason: String,
    /// When the wake becomes due, for the reasons that have a due time.
    pub due: Option<Timestamp>,
    /// Scheduling priority; lower is sooner.
    pub priority: u8,
    /// The tenant the wake is fair-shared under.
    pub tenant: String,
    /// The due shard it is written to.
    pub shard: u16,
}

#[cfg(test)]
mod tests {
    use super::{
        Condition, activation_preconditions, claim_child_conditions, claim_conditions,
        dispatch_started_conditions, response_started_conditions, stop_conditions,
    };
    use aex_brain_domain::commit::FenceGuardRef;
    use aex_brain_domain::ids::{
        AgentId, AgentKey, AgentRevision, CancelEpoch, EffectId, Fence, JournalSeq, OwnerToken,
        SessionId, Timestamp,
    };
    use uuid::Uuid;

    fn guard(tail: Option<JournalSeq>) -> FenceGuardRef {
        FenceGuardRef {
            key: AgentKey::new(SessionId(Uuid::from_u128(1)), AgentId(Uuid::from_u128(2))),
            owner: OwnerToken(Uuid::from_u128(3)),
            fence: Fence(4),
            revision: AgentRevision(5),
            tail,
            cancel_epoch: CancelEpoch(6),
        }
    }

    fn names(conditions: &[Condition]) -> Vec<&'static str> {
        conditions
            .iter()
            .filter_map(|condition| match condition {
                Condition::Equals { attribute, .. }
                | Condition::LessThan { attribute, .. }
                | Condition::NotEquals { attribute, .. } => Some(*attribute),
                Condition::SumWithin { used, .. } => Some(*used),
                Condition::NotExists | Condition::Exists => None,
            })
            .collect()
    }

    /// All five preconditions, always. Each rejects a different stale writer, so a missing
    /// one is a hole rather than an optimization.
    #[test]
    fn every_activation_write_carries_the_whole_precondition_set() {
        let conditions = activation_preconditions(&guard(Some(JournalSeq(9))));
        assert_eq!(
            names(&conditions),
            vec![
                "revision",
                "fence",
                "lease_owner",
                "cancel_epoch",
                "journal_tail"
            ]
        );
        assert!(conditions.contains(&Condition::Equals {
            attribute: "journal_tail",
            value: "9".to_owned()
        }));
    }

    /// An agent with no journal yet conditions on the *absence* of a tail, not on zero.
    /// Conditioning on zero would let a commit that appended sequence zero be replayed.
    #[test]
    fn an_empty_journal_conditions_on_absence_not_on_zero() {
        let conditions = activation_preconditions(&guard(None));
        assert!(conditions.contains(&Condition::Equals {
            attribute: "journal_tail",
            value: "null".to_owned()
        }));
    }

    #[test]
    fn a_steal_waits_out_the_lease_and_the_grace() {
        let conditions =
            claim_conditions(Timestamp(100_000), core::time::Duration::from_secs(5), None);
        assert!(conditions.contains(&Condition::LessThan {
            attribute: "lease_expires_at",
            value: "95000".to_owned()
        }));
        assert!(
            !conditions.iter().any(|condition| matches!(
                condition,
                Condition::Equals {
                    attribute: "status",
                    ..
                } | Condition::NotEquals {
                    attribute: "status",
                    ..
                } | Condition::LessThan {
                    attribute: "status",
                    ..
                }
            )),
            "a terminal agent can still be fenced while its source wake retires"
        );
    }

    #[test]
    fn reclaiming_your_own_agent_does_not_wait_out_your_own_lease() {
        let conditions = claim_conditions(
            Timestamp(100_000),
            core::time::Duration::from_secs(5),
            Some("owner-1"),
        );
        assert!(conditions.contains(&Condition::Equals {
            attribute: "lease_owner",
            value: "owner-1".to_owned()
        }));
    }

    #[test]
    fn dispatch_started_requires_both_the_effect_state_and_the_agent_fence() {
        let conditions = dispatch_started_conditions(EffectId([7; 16]), 4);
        assert_eq!(
            names(&conditions),
            vec!["state", "effect_id", "agent_fence"]
        );
        assert!(conditions.contains(&Condition::Equals {
            attribute: "state",
            value: "prepared".to_owned()
        }));
    }

    #[test]
    fn a_response_may_only_start_after_a_dispatch_did() {
        assert_eq!(
            response_started_conditions(),
            vec![Condition::Equals {
                attribute: "state",
                value: "dispatch_started".to_owned()
            }]
        );
    }

    /// The active limit is checked and incremented in the same transaction, so two
    /// claimants can never both conclude there is room.
    #[test]
    fn the_child_claim_checks_the_limit_it_is_about_to_consume() {
        let conditions = claim_child_conditions(6, 2);
        assert!(conditions.contains(&Condition::SumWithin {
            used: "active_used",
            delta: 1,
            limit: "active_limit"
        }));
        assert!(conditions.contains(&Condition::Equals {
            attribute: "state",
            value: "queued".to_owned()
        }));
        assert!(conditions.contains(&Condition::Equals {
            attribute: "cancel_epoch",
            value: "6".to_owned()
        }));
    }

    #[test]
    fn a_stop_is_fenced_to_the_incarnation_the_caller_observed() {
        assert_eq!(
            stop_conditions(11),
            vec![Condition::Equals {
                attribute: "fence",
                value: "11".to_owned()
            }]
        );
    }
}
