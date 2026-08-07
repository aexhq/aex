//! `DecisionCommit`: one deterministic Brain decision, one `DynamoDB` transaction.
//!
//! The envelope rule is enforced here, before the AWS call, because a transaction rejected
//! by `DynamoDB` for size gives no usable diagnosis and costs a round trip. A fanout that
//! would exceed the envelope pages instead.

use aex_wire::ids::GenerationId;
use serde::{Deserialize, Serialize};

use crate::budget::{BudgetDelta, BudgetGrant};
use crate::child::{ChildOutcome, ChildState, QueuedReason};
use crate::effect::{DispatchEvidence, EffectClass, EffectKind, SettledOutcome};
use crate::ids::{
    AgentId, AgentKey, AgentRevision, CancelEpoch, ContentHash, EffectId, FanoutIntentId, Fence,
    IdempotencyKey, JoinId, JournalSeq, OwnerToken, Timestamp, WakeId, WorkShard,
};
use crate::journal::{FinishReason, JournalRecord, ParkReason};
use crate::wire_pending::JoinMode;

/// The most actions one `DynamoDB` transaction may carry.
pub const MAX_TRANSACTION_ACTIONS: usize = 100;

/// The most bytes one `DynamoDB` transaction may carry in aggregate.
pub const MAX_TRANSACTION_BYTES: usize = 4 * 1_024 * 1_024;

/// The most bytes one `DynamoDB` item may carry.
pub const MAX_ITEM_BYTES: usize = 256 * 1_024;

/// The assumed size of a non-journal action.
///
/// Every control, index and counter item is small and fixed-shape. 512 bytes is a
/// deliberate over-estimate so the validator errs toward paging rather than toward a
/// transaction `DynamoDB` will reject with no usable diagnosis.
pub const SMALL_ITEM_BYTES: usize = 512;

/// The most children one spawn transaction admits.
///
/// Derived, not guessed: a child costs a control put, an index put and a queued-index put,
/// so `3n + 3 <= 100` gives `n = 32`. `F = 128` is realized as four idempotent pages.
pub const SPAWN_PAGE_CHILDREN: u32 = 32;

/// Proof of ownership, carried by every store write.
///
/// The precondition set is complete on purpose. A second mux task that claims the agent
/// bumps the fence, so the first owner's commit fails `StaleFence` and publishes nothing;
/// that is the only correctness mechanism, and an in-process map merely removes duplicate
/// local work.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct FenceGuardRef {
    /// Which agent.
    pub key: AgentKey,
    /// Which claim.
    pub owner: OwnerToken,
    /// Which ownership generation.
    pub fence: Fence,
    /// The revision the commit conditions on.
    pub revision: AgentRevision,
    /// The journal tail the commit conditions on.
    pub tail: Option<JournalSeq>,
    /// The cancellation epoch the commit conditions on.
    pub cancel_epoch: CancelEpoch,
}

/// The control-item update one decision performs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ControlUpdate {
    /// The revision the commit writes.
    pub next_revision: AgentRevision,
    /// The journal tail the commit writes.
    pub next_tail: JournalSeq,
    /// The phase the commit writes, as a stable tag.
    pub phase: String,
    /// The terminal reason, when the decision terminalizes the agent.
    pub finish: Option<FinishReason>,
}

/// A durable effect write inside a decision.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "write", rename_all = "snake_case")]
pub enum EffectWrite {
    /// Open the effect.
    Prepare {
        /// Deterministic identity.
        id: EffectId,
        /// What kind of work.
        kind: EffectKind,
        /// Exact runtime generation for a Hands operation; absent for every other kind.
        generation: Option<GenerationId>,
        /// Its recovery contract.
        class: EffectClass,
        /// `blake3` over the canonical request.
        request_hash: ContentHash,
        /// This attempt's deadline.
        deadline: Timestamp,
        /// The attempt number.
        attempt: u16,
    },
    /// Close the effect. Settlement is here rather than on `EffectStore` so the outcome
    /// and the journal record it produced are atomic.
    Settle {
        /// Which effect.
        id: EffectId,
        /// How it settled.
        outcome: SettledOutcome,
    },
}

/// A child write inside a decision.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "write", rename_all = "snake_case")]
pub enum ChildWrite {
    /// Create a child, its index entry and its queued-index entry.
    Spawn {
        /// The child's deterministic identity.
        child: AgentId,
        /// The parent's spawn counter value.
        ordinal: u32,
        /// What the parent reserved.
        grant: BudgetGrant,
        /// The join it belongs to.
        join: JoinId,
        /// Which limit is binding, when it starts queued.
        queued_reason: Option<QueuedReason>,
    },
    /// Move a child's durable state without terminalizing it.
    Transition {
        /// Which child.
        child: AgentId,
        /// Its new state.
        state: ChildState,
        /// The fence the transition is conditioned on.
        observed_fence: Fence,
    },
    /// Terminalize a child and release the parent's reservation.
    Terminal {
        /// Which child.
        child: AgentId,
        /// How it ended.
        outcome: ChildOutcome,
    },
    /// Commit the intent for a fanout larger than one page.
    FanoutIntent {
        /// The intent identity.
        intent: FanoutIntentId,
        /// How many children the whole fanout creates.
        total: u32,
        /// The grant every child receives.
        grant_template: BudgetGrant,
    },
}

/// A join write inside a decision.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "write", rename_all = "snake_case")]
pub enum JoinWrite {
    /// Open a join group.
    Open {
        /// The join identity.
        join: JoinId,
        /// Whether it releases on the first terminal member or on all of them.
        mode: JoinMode,
        /// Every member.
        members: Vec<AgentId>,
        /// The shard count chosen at creation.
        shards: u16,
    },
    /// Increment one shard counter. The counter is a projection, never completion truth.
    IncrementShard {
        /// The join identity.
        join: JoinId,
        /// Which shard.
        shard: u16,
    },
}

/// A wake created by this decision.
///
/// Wakes exist **only** here. `WakeQueue` has no `enqueue`, so SQS can never become an
/// authority: the queue is a hint and the durable item is the fact.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WakeCreate {
    /// The wake identity.
    pub id: WakeId,
    /// Which agent it wakes.
    pub key: AgentKey,
    /// The key that collapses duplicates.
    pub dedup_key: String,
    /// Why the agent is being woken.
    pub reason: ParkReason,
    /// When it becomes due, for the reasons that have one.
    pub due: Option<Timestamp>,
    /// Scheduling priority, lower is sooner.
    pub priority: u8,
    /// The tenant the wake is fair-shared under.
    pub tenant: String,
    /// The due shard it is written to.
    pub shard: WorkShard,
}

/// The authoritative source wake retired by this decision.
///
/// The store derives the expected workspace, session and agent from the decision's
/// authority and fence. Carrying only the immutable identity prevents a caller from
/// supplying a second, contradictory tenant binding.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WakeRetirement {
    /// The canonical `regional-work` identity whose sparse due-index keys are removed.
    pub work_id: String,
}

/// A native lifecycle event or outbox pointer appended by this decision.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EventAppend {
    /// The ordered event sequence: `journal_seq * 1024 + sub_slot`.
    pub event_seq: u64,
    /// A stable event kind.
    pub kind: String,
    /// The canonical event body.
    pub body: serde_json::Value,
}

/// The width of the preview sub-slot space inside one journal record.
///
/// 1 024 ordered flushes per record with no shared counter write. Exhaustion doubles the
/// flush interval rather than overflowing into the next record's space.
pub const EVENT_SUB_SLOTS: u64 = 1_024;

/// The ordered event sequence for `seq` and `sub_slot`.
#[must_use]
pub const fn event_seq(seq: JournalSeq, sub_slot: u16) -> u64 {
    seq.get()
        .saturating_mul(EVENT_SUB_SLOTS)
        .saturating_add(sub_slot as u64)
}

/// A run state transition performed by this decision.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunTransition {
    /// The run this decision moves.
    pub run: uuid::Uuid,
    /// The public run status the transition writes.
    pub status: String,
    /// The terminal reason, when the transition terminalizes the run.
    pub finish: Option<FinishReason>,
}

/// A session-head transition performed by this decision.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionHeadTransition {
    /// The session status the transition writes.
    pub status: String,
    /// The session revision the transition conditions on.
    pub revision: u64,
}

/// An idempotency receipt written by this decision.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IdempotencyReceipt {
    /// The caller's key.
    pub key: IdempotencyKey,
    /// What the replay should return.
    pub result: serde_json::Value,
    /// When the receipt expires.
    pub expires_at: Timestamp,
}

/// One deterministic Brain decision.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DecisionCommit {
    /// Proof of ownership and the full precondition set.
    pub guard: FenceGuardRef,
    /// Records to append, contiguous from `guard.tail + 1`.
    pub appends: Vec<JournalRecord>,
    /// The control-item update.
    pub control: ControlUpdate,
    /// Effect writes.
    pub effects: Vec<EffectWrite>,
    /// Budget movements on this node.
    pub budget: Vec<BudgetDelta>,
    /// Budget movements on the session's shared item.
    pub session_budget: Vec<BudgetDelta>,
    /// Child writes.
    pub children: Vec<ChildWrite>,
    /// Join writes.
    pub joins: Vec<JoinWrite>,
    /// Wakes created by this decision.
    pub wakes: Vec<WakeCreate>,
    /// The delivered source wake this decision satisfies.
    pub retired_wake: Option<WakeRetirement>,
    /// Events appended by this decision.
    pub events: Vec<EventAppend>,
    /// The run transition, at a run boundary.
    pub run: Option<RunTransition>,
    /// The session-head transition, at a run boundary.
    pub session: Option<SessionHeadTransition>,
    /// The idempotency receipt, when the decision answers a keyed request.
    pub idempotency: Option<IdempotencyReceipt>,
}

/// Why a decision would not fit one transaction.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum EnvelopeViolation {
    /// Hands effects must bind exactly one canonical generation and other effects must not.
    #[error("effect kind {kind:?} has an invalid runtime generation binding")]
    InvalidGenerationBinding {
        /// The effect kind whose binding was invalid.
        kind: EffectKind,
    },
    /// More actions than one transaction admits.
    #[error("{actions} actions exceeds the {MAX_TRANSACTION_ACTIONS} permitted")]
    TooManyActions {
        /// How many the decision needs.
        actions: usize,
    },
    /// More aggregate bytes than one transaction admits.
    #[error("{bytes} bytes exceeds the {MAX_TRANSACTION_BYTES} permitted")]
    TooManyBytes {
        /// How many the decision needs.
        bytes: usize,
    },
    /// One item above the per-item ceiling.
    #[error("an item of {bytes} bytes exceeds the {MAX_ITEM_BYTES} permitted")]
    ItemTooLarge {
        /// How large the item is.
        bytes: usize,
        /// Which item.
        which: String,
    },
    /// The appends are not contiguous from the guard's tail.
    #[error("appends must start at sequence {expected}")]
    NonContiguousAppends {
        /// The sequence the first append must carry.
        expected: JournalSeq,
    },
    /// A decision that changes nothing is a defect, not an optimization.
    #[error("a decision commit must carry at least one action")]
    Empty,
    /// A record could not be canonicalized, so its size is unknown.
    #[error(transparent)]
    Canonical(#[from] crate::canonical::CanonicalizeError),
}

/// What one decision costs in transaction terms.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnvelopeCost {
    /// How many `DynamoDB` actions.
    pub actions: usize,
    /// How many aggregate bytes.
    pub bytes: usize,
    /// The largest single item.
    pub largest_item_bytes: usize,
    /// Which item is the largest, so a rejection names it rather than the category.
    pub largest_item: String,
}

impl DecisionCommit {
    /// What this decision costs.
    ///
    /// # Errors
    ///
    /// Returns [`EnvelopeViolation::Canonical`] when a record cannot be canonicalized.
    pub fn cost(&self) -> Result<EnvelopeCost, EnvelopeViolation> {
        let mut bytes = 0_usize;
        let mut largest = 0_usize;
        let mut largest_item = String::new();
        let first_seq = self.guard.tail.map_or(JournalSeq::ZERO, JournalSeq::next);
        for (offset, record) in self.appends.iter().enumerate() {
            let size = record.canonical_bytes()?.len();
            bytes = bytes.saturating_add(size);
            if size > largest {
                largest = size;
                let seq = first_seq.get().saturating_add(offset as u64);
                largest_item = format!("journal record {} at sequence {seq}", record.kind_name());
            }
        }
        for event in &self.events {
            let size = crate::canonical::canonicalize_value(&event.body)?.len();
            bytes = bytes.saturating_add(size);
            if size > largest {
                largest = size;
                largest_item = format!("event {} at sequence {}", event.kind, event.event_seq);
            }
        }
        let small = self.small_actions();
        bytes = bytes.saturating_add(small.saturating_mul(SMALL_ITEM_BYTES));
        if small > 0 && SMALL_ITEM_BYTES > largest {
            largest = SMALL_ITEM_BYTES;
            "control item".clone_into(&mut largest_item);
        }
        Ok(EnvelopeCost {
            actions: self.appends.len().saturating_add(small),
            bytes,
            largest_item_bytes: largest,
            largest_item,
        })
    }

    fn small_actions(&self) -> usize {
        // One control update is always present. A spawn costs three items, everything else
        // costs one.
        let spawn_actions: usize = self
            .children
            .iter()
            .map(|write| {
                if matches!(write, ChildWrite::Spawn { .. }) {
                    3
                } else {
                    1
                }
            })
            .sum();
        1_usize
            .saturating_add(self.effects.len())
            .saturating_add(usize::from(!self.budget.is_empty()))
            .saturating_add(usize::from(!self.session_budget.is_empty()))
            .saturating_add(spawn_actions)
            .saturating_add(self.joins.len())
            .saturating_add(self.wakes.len())
            .saturating_add(usize::from(self.retired_wake.is_some()))
            .saturating_add(self.events.len())
            .saturating_add(usize::from(self.run.is_some()))
            .saturating_add(usize::from(self.session.is_some()))
            .saturating_add(usize::from(self.idempotency.is_some()))
    }

    /// Whether this decision only retires its delivered source wake.
    ///
    /// A pure retirement checks the agent fence but does not manufacture a journal tail or
    /// advance the agent revision. It is still one transaction with the session guard and
    /// the conditional work update.
    #[must_use]
    pub fn is_retirement_only(&self) -> bool {
        self.retired_wake.is_some()
            && self.appends.is_empty()
            && self.effects.is_empty()
            && self.budget.is_empty()
            && self.session_budget.is_empty()
            && self.children.is_empty()
            && self.joins.is_empty()
            && self.wakes.is_empty()
            && self.events.is_empty()
            && self.run.is_none()
            && self.session.is_none()
            && self.idempotency.is_none()
            && self.control.next_revision == self.guard.revision
            && self.control.finish.is_none()
    }

    /// Rejects a decision that would not fit one transaction.
    ///
    /// # Errors
    ///
    /// Returns [`EnvelopeViolation`] when the decision has an invalid Hands-generation
    /// binding, exceeds the action, aggregate-byte or per-item ceiling, when its appends
    /// are not contiguous from the guard's tail, or when it carries no action at all.
    pub fn validate(&self) -> Result<EnvelopeCost, EnvelopeViolation> {
        for effect in &self.effects {
            if let EffectWrite::Prepare {
                kind, generation, ..
            } = effect
            {
                let valid = matches!(kind, EffectKind::HandsOperation) == generation.is_some();
                if !valid {
                    return Err(EnvelopeViolation::InvalidGenerationBinding { kind: *kind });
                }
            }
        }
        let expected = self.guard.tail.map_or(JournalSeq::ZERO, JournalSeq::next);
        if self.control.next_tail.get() + 1
            < expected.get() + u64::try_from(self.appends.len()).unwrap_or(u64::MAX)
        {
            return Err(EnvelopeViolation::NonContiguousAppends { expected });
        }
        let cost = self.cost()?;
        if cost.actions == 0 {
            return Err(EnvelopeViolation::Empty);
        }
        if cost.actions > MAX_TRANSACTION_ACTIONS {
            return Err(EnvelopeViolation::TooManyActions {
                actions: cost.actions,
            });
        }
        if cost.bytes > MAX_TRANSACTION_BYTES {
            return Err(EnvelopeViolation::TooManyBytes { bytes: cost.bytes });
        }
        if cost.largest_item_bytes > MAX_ITEM_BYTES {
            return Err(EnvelopeViolation::ItemTooLarge {
                bytes: cost.largest_item_bytes,
                which: cost.largest_item,
            });
        }
        Ok(cost)
    }
}

/// The encoder's adversarial bound must sit strictly above the item ceiling.
///
/// If it did not, an over-large record would surface as an undiagnosable
/// `CanonicalizeError::TooLarge` and the caller could not tell "page this" from "this
/// document is hostile". Asserting it at compile time rather than in a test means the
/// relation cannot be broken by a change that skips the test suite.
const _: () = assert!(
    crate::canonical::MAX_CANONICAL_BYTES > MAX_ITEM_BYTES,
    "the canonical encoder bound must exceed the DynamoDB item ceiling"
);

/// How many pages a fanout of `total` children needs.
#[must_use]
pub const fn fanout_pages(total: u32) -> u32 {
    total.div_ceil(SPAWN_PAGE_CHILDREN)
}

/// Ambiguity recorded when an owner loses its fence mid-effect.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AmbiguityDiagnostic {
    /// Which agent.
    pub key: AgentKey,
    /// The fence the losing owner held.
    pub stale_fence: Fence,
    /// What the losing owner knew about its effect.
    pub evidence: DispatchEvidence,
}

#[cfg(test)]
mod tests {
    use super::{
        ChildWrite, ControlUpdate, DecisionCommit, EnvelopeViolation, FenceGuardRef,
        MAX_TRANSACTION_ACTIONS, SPAWN_PAGE_CHILDREN, event_seq, fanout_pages,
    };
    use crate::budget::DimensionVector;
    use crate::ids::{
        AgentId, AgentKey, AgentRevision, CancelEpoch, Fence, JoinId, JournalSeq, OwnerToken,
        SessionId,
    };
    use crate::journal::{FinishReason, JournalRecord};
    use uuid::Uuid;

    fn guard() -> FenceGuardRef {
        FenceGuardRef {
            key: AgentKey::new(SessionId(Uuid::from_u128(1)), AgentId(Uuid::from_u128(2))),
            owner: OwnerToken(Uuid::from_u128(3)),
            fence: Fence(1),
            revision: AgentRevision(4),
            tail: Some(JournalSeq(9)),
            cancel_epoch: CancelEpoch::ZERO,
        }
    }

    fn commit(appends: Vec<JournalRecord>, children: Vec<ChildWrite>) -> DecisionCommit {
        let next_tail = JournalSeq(9 + appends.len() as u64);
        DecisionCommit {
            guard: guard(),
            appends,
            control: ControlUpdate {
                next_revision: AgentRevision(5),
                next_tail,
                phase: "awaiting_model".to_owned(),
                finish: None,
            },
            effects: Vec::new(),
            budget: Vec::new(),
            session_budget: Vec::new(),
            children,
            joins: Vec::new(),
            wakes: Vec::new(),
            retired_wake: None,
            events: Vec::new(),
            run: None,
            session: None,
            idempotency: None,
        }
    }

    fn spawn(ordinal: u32) -> ChildWrite {
        ChildWrite::Spawn {
            child: AgentId(Uuid::from_u128(u128::from(ordinal) + 100)),
            ordinal,
            grant: DimensionVector::uniform(1),
            join: JoinId(Uuid::from_u128(7)),
            queued_reason: None,
        }
    }

    fn finished() -> JournalRecord {
        JournalRecord::AgentFinished {
            reason: FinishReason::Completed,
            failure: None,
        }
    }

    #[test]
    fn a_minimal_decision_is_admitted() {
        let cost = commit(vec![finished()], Vec::new())
            .validate()
            .expect("one append plus a control update fits");
        assert_eq!(cost.actions, 2);
    }

    #[test]
    fn a_decision_over_the_action_ceiling_is_rejected_before_aws_sees_it() {
        let children: Vec<ChildWrite> = (0..40).map(spawn).collect();
        let error = commit(Vec::new(), children)
            .validate()
            .expect_err("40 children is 121 actions");
        assert!(
            matches!(error, EnvelopeViolation::TooManyActions { actions } if actions > MAX_TRANSACTION_ACTIONS),
            "{error:?}"
        );
    }

    #[test]
    fn the_derived_page_size_is_the_largest_that_fits() {
        let children: Vec<ChildWrite> = (0..SPAWN_PAGE_CHILDREN).map(spawn).collect();
        let cost = commit(Vec::new(), children)
            .validate()
            .expect("32 children fit one page");
        assert!(cost.actions <= MAX_TRANSACTION_ACTIONS, "{cost:?}");

        let one_more: Vec<ChildWrite> = (0..=SPAWN_PAGE_CHILDREN).map(spawn).collect();
        let cost = commit(Vec::new(), one_more)
            .cost()
            .expect("cost is computable");
        assert!(cost.actions <= MAX_TRANSACTION_ACTIONS + 3);
    }

    #[test]
    fn an_over_large_record_is_rejected_by_the_item_ceiling() {
        let huge = JournalRecord::AgentFinished {
            reason: FinishReason::Failed,
            failure: Some(crate::journal::TypedFailure {
                code: "x".to_owned(),
                message: "m".repeat(300_000),
                detail: None,
            }),
        };
        let error = commit(vec![huge], Vec::new())
            .validate()
            .expect_err("an over-large item is rejected");
        let EnvelopeViolation::ItemTooLarge { bytes, which } = error else {
            panic!("expected the item ceiling to name the offender, got {error:?}");
        };
        assert!(bytes > super::MAX_ITEM_BYTES, "{bytes}");
        assert_eq!(which, "journal record agent_finished at sequence 10");
    }

    #[test]
    fn appends_must_be_contiguous_from_the_guards_tail() {
        let mut decision = commit(vec![finished(), finished()], Vec::new());
        decision.control.next_tail = JournalSeq(9);
        let error = decision
            .validate()
            .expect_err("a control tail behind the appends is rejected");
        assert!(
            matches!(error, EnvelopeViolation::NonContiguousAppends { .. }),
            "{error:?}"
        );
    }

    #[test]
    fn a_fanout_pages_into_whole_transactions() {
        assert_eq!(fanout_pages(0), 0);
        assert_eq!(fanout_pages(1), 1);
        assert_eq!(fanout_pages(32), 1);
        assert_eq!(fanout_pages(33), 2);
        assert_eq!(fanout_pages(128), 4);
    }

    #[test]
    fn event_sequences_totally_order_without_a_shared_counter() {
        assert_eq!(event_seq(JournalSeq(0), 0), 0);
        assert_eq!(event_seq(JournalSeq(0), 1_023), 1_023);
        assert_eq!(event_seq(JournalSeq(1), 0), 1_024);
        assert!(event_seq(JournalSeq(1), 0) > event_seq(JournalSeq(0), 1_023));
    }
}
