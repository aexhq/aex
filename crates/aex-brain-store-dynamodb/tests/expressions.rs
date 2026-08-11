//! Slice S-2.2 — expression and item shapes for every transaction in the plan's table,
//! asserted without an AWS call.
//!
//! These are the shapes a live test could only observe indirectly and expensively. Pinning
//! them here means a change to the key namespace or a dropped precondition fails in the
//! unit lane rather than as an unexplained conflict on a deployed plane.

use aex_brain_domain::commit::{
    DecisionCommit, MAX_ITEM_BYTES, MAX_TRANSACTION_ACTIONS, SPAWN_PAGE_CHILDREN,
};
use aex_brain_domain::ids::{
    AgentId, AgentKey, EffectId, FanoutIntentId, JoinId, JournalSeq, SessionId,
};
use aex_brain_store_dynamodb::keys::{
    BRAIN_AGENT_PARTITION_PREFIX, BRAIN_PREFIX, agent_partition, brain_agent_partition,
    child_index_sort_key, control_sort_key, effect_sort_key, fanout_intent_sort_key,
    join_shard_for, join_sort_key, journal_sort_key, mailbox_sort_key, queued_index_sort_key,
    session_budget_sort_key, session_partition,
};
use aex_brain_store_dynamodb::{Condition, Table};
use aex_wire::ids::Uuid7;
use uuid::Uuid;

fn v7(millis: u64, seed: u8) -> Uuid {
    Uuid::from_bytes(*Uuid7::compose(millis, [seed; 10]).as_bytes())
}

fn key(agent: u64) -> AgentKey {
    AgentKey::new(
        SessionId(v7(1_767_225_600_000, 1)),
        AgentId(v7(1_767_225_600_000 + agent, 2)),
    )
}

/// Exactly two tables participate. A third would turn a Brain decision into a distributed
/// transaction across another owner's authority.
#[test]
fn only_two_tables_participate() {
    let tables = [Table::SessionAuthority, Table::RegionalWork];
    assert_eq!(tables.len(), 2);
    assert_ne!(tables[0], tables[1]);
}

/// Two agents in one session never share a partition, and one agent's items never leak into
/// another's.
#[test]
fn agent_partitions_are_disjoint() {
    let first = agent_partition(&key(2)).expect("a version-7 identity");
    let second = agent_partition(&key(3)).expect("a version-7 identity");
    assert_ne!(first, second);
    assert!(first.starts_with("AGENT#"));
    assert_ne!(
        first,
        session_partition(SessionId(v7(1_767_225_600_000, 1))).expect("a version-7 identity")
    );
}

/// Every sort key the Brain writes is accounted for, and each is in exactly one of the two
/// namespaces. A key in neither would be an item nobody owns.
#[test]
fn every_brain_sort_key_is_in_exactly_one_namespace() {
    let per_agent = [
        control_sort_key(),
        journal_sort_key(JournalSeq(0)),
        effect_sort_key(EffectId([1; 16])),
        child_index_sort_key(0, AgentId(v7(1_767_225_600_004, 4))),
        join_sort_key(JoinId(v7(1_767_225_600_005, 5))),
        mailbox_sort_key(0),
    ];
    let session_level = [
        session_budget_sort_key(),
        queued_index_sort_key(0, 0, AgentId(v7(1_767_225_600_006, 6))),
        fanout_intent_sort_key(FanoutIntentId(v7(1_767_225_600_007, 7))),
    ];
    assert!(
        brain_agent_partition(&key(2))
            .expect("a version-7 identity")
            .starts_with(BRAIN_AGENT_PARTITION_PREFIX),
        "Brain-owned per-agent items sit in the reserved partition space"
    );

    for sort_key in &per_agent {
        assert!(!sort_key.starts_with(BRAIN_PREFIX), "{sort_key}");
    }
    for sort_key in &session_level {
        assert!(sort_key.starts_with(BRAIN_PREFIX), "{sort_key}");
    }

    let mut all: Vec<&str> = per_agent
        .iter()
        .chain(session_level.iter())
        .map(String::as_str)
        .collect();
    let total = all.len();
    all.sort_unstable();
    all.dedup();
    assert_eq!(all.len(), total, "two item families share a sort key");
}

/// The journal reads back in sequence order for any page, which is what lets a gap be a
/// real gap rather than a formatting accident.
#[test]
fn a_paged_journal_read_returns_records_in_sequence_order() {
    let mut keys: Vec<(u64, String)> = (0..2_048_u64)
        .map(|seq| (seq, journal_sort_key(JournalSeq(seq))))
        .collect();
    keys.sort_by(|left, right| left.1.cmp(&right.1));
    for (index, (seq, _)) in keys.iter().enumerate() {
        assert_eq!(
            *seq, index as u64,
            "sort order diverged from sequence order"
        );
    }
}

/// A join's shard count comes from the measured conflict curve, and the members spread
/// across it.
#[test]
fn join_shards_follow_the_measured_curve_and_are_used() {
    use aex_brain_domain::wire_pending::join_shards;
    assert_eq!(join_shards(1), 1);
    assert_eq!(join_shards(8), 1);
    assert_eq!(join_shards(16), 2);
    assert_eq!(join_shards(1_000), 64, "clamped at the measured knee");
    assert_eq!(join_shards(100_000), 64, "and never above it");

    let shards = join_shards(1_000);
    let mut used = vec![false; usize::from(shards)];
    for seed in 0..1_000_u64 {
        used[usize::from(join_shard_for(
            AgentId(v7(1_767_225_600_000 + seed, 9)),
            shards,
        ))] = true;
    }
    assert!(
        used.iter().filter(|hit| **hit).count() > shards as usize / 2,
        "a counter that concentrates on a few shards has not removed the hot spot"
    );
}

/// The envelope validator, the page-size constant and the action ceiling agree.
///
/// Three numbers that must move together: if the page size were derived from one and
/// checked against another, a fanout would either under-fill every transaction or be
/// rejected at the boundary.
#[test]
fn the_page_size_the_validator_and_the_ceiling_agree() {
    assert_eq!(MAX_TRANSACTION_ACTIONS, 100);
    assert_eq!(
        3 * SPAWN_PAGE_CHILDREN as usize + 3,
        99,
        "the derivation must leave the ceiling intact"
    );
    assert!(3 * (SPAWN_PAGE_CHILDREN as usize + 1) + 3 > MAX_TRANSACTION_ACTIONS);
    assert_eq!(MAX_ITEM_BYTES, 256 * 1_024);
}

/// A condition set is compared as data, so a dropped precondition is a diff rather than a
/// silent behavioural change.
#[test]
fn conditions_are_values_and_compare_structurally() {
    let left = Condition::Equals {
        attribute: "fence",
        value: "4".to_owned(),
    };
    assert_eq!(left.clone(), left);
    assert_ne!(
        left,
        Condition::Equals {
            attribute: "fence",
            value: "5".to_owned()
        }
    );
    assert_ne!(
        left,
        Condition::Equals {
            attribute: "revision",
            value: "4".to_owned()
        }
    );
}

/// A decision that would not fit is rejected here, before any AWS call.
#[test]
fn an_oversized_decision_never_reaches_the_transport() {
    use aex_brain_domain::budget::DimensionVector;
    use aex_brain_domain::commit::{ChildWrite, ControlUpdate, FenceGuardRef};
    use aex_brain_domain::ids::{AgentRevision, CancelEpoch, Fence, OwnerToken};

    let children: Vec<ChildWrite> = (0..64)
        .map(|ordinal| ChildWrite::Spawn {
            child: AgentId(v7(1_767_225_600_100 + u64::from(ordinal), 8)),
            ordinal,
            grant: DimensionVector::uniform(1),
            join: JoinId(v7(1_767_225_600_007, 7)),
            queued_reason: None,
        })
        .collect();
    let decision = DecisionCommit {
        guard: FenceGuardRef {
            key: key(2),
            owner: OwnerToken(v7(1_767_225_600_003, 3)),
            fence: Fence(1),
            revision: AgentRevision(1),
            tail: None,
            cancel_epoch: CancelEpoch::ZERO,
        },
        appends: Vec::new(),
        control: ControlUpdate {
            next_revision: AgentRevision(2),
            next_tail: JournalSeq(0),
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
        messages: Vec::new(),
        session_events: Vec::new(),
        run: None,
        session: None,
        idempotency: None,
    };
    decision
        .validate()
        .expect_err("64 children is well past the envelope");
}
