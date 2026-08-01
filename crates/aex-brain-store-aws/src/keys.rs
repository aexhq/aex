//! The Brain's key namespace inside `session-authority`.
//!
//! This module is a **published cross-stream artifact**. The regional-stores peer shares
//! the table, so the ownership rule has to be one sentence with no exceptions:
//!
//! - every Brain-owned **session-level** item uses the `BRAIN#` sort-key prefix under the
//!   session partition;
//! - every **per-agent** item lives in its own `SESSION#<sid>#AGENT#<aid>` partition.
//!
//! No other prefix is claimed, and nothing here forks an item shape another crate owns.
//!
//! Sort keys are zero-padded to a fixed width so lexicographic order is numeric order.
//! Without the padding `J#10` sorts before `J#9`, and a paged journal read would silently
//! return records out of sequence — which the fold would then reject as a gap, turning a
//! formatting mistake into an unexplained outage.

use aex_brain_domain::ids::{
    AgentId, AgentKey, EffectId, FanoutIntentId, JoinId, JournalSeq, SessionId,
};

/// The sort-key prefix every Brain-owned session-level item carries.
pub const BRAIN_PREFIX: &str = "BRAIN#";

/// Width of a zero-padded sequence in a sort key.
pub const SEQ_WIDTH: usize = 20;

/// Width of a zero-padded ordinal in a sort key.
pub const ORDINAL_WIDTH: usize = 10;

/// Width of a zero-padded priority in a sort key.
pub const PRIORITY_WIDTH: usize = 3;

/// Width of a zero-padded shard index in a sort key.
pub const SHARD_WIDTH: usize = 3;

/// The partition every per-agent item lives in.
#[must_use]
pub fn agent_partition(key: &AgentKey) -> String {
    format!(
        "SESSION#{}#AGENT#{}",
        key.session.0.as_hyphenated(),
        key.agent.0.as_hyphenated()
    )
}

/// The partition every session-level item lives in.
#[must_use]
pub fn session_partition(session: SessionId) -> String {
    format!("SESSION#{}", session.0.as_hyphenated())
}

/// The agent control item's sort key.
#[must_use]
pub fn control_sort_key() -> String {
    "CONTROL".to_owned()
}

/// One journal record's sort key.
#[must_use]
pub fn journal_sort_key(seq: JournalSeq) -> String {
    format!("J#{:0>width$}", seq.get(), width = SEQ_WIDTH)
}

/// One durable effect's sort key.
#[must_use]
pub fn effect_sort_key(effect: EffectId) -> String {
    format!("E#{}", effect.to_hex())
}

/// One child index entry's sort key under its parent.
#[must_use]
pub fn child_index_sort_key(ordinal: u32, child: AgentId) -> String {
    format!(
        "C#{:0>width$}#{}",
        ordinal,
        child.0.as_hyphenated(),
        width = ORDINAL_WIDTH
    )
}

/// One queued child's sort key in the session-level scheduler index.
#[must_use]
pub fn queued_index_sort_key(priority: u8, enqueued_at_millis: i64, child: AgentId) -> String {
    // The enqueue instant is clamped at zero: a negative timestamp would sort before every
    // legitimate entry and jump the queue, and a clock that produced one is already wrong.
    let enqueued = enqueued_at_millis.max(0);
    format!(
        "{BRAIN_PREFIX}Q#{:0>pw$}#{:0>sw$}#{}",
        priority,
        enqueued,
        child.0.as_hyphenated(),
        pw = PRIORITY_WIDTH,
        sw = SEQ_WIDTH
    )
}

/// One join group's sort key under the waiting parent.
#[must_use]
pub fn join_sort_key(join: JoinId) -> String {
    format!("JOIN#{}", join.0.as_hyphenated())
}

/// One join shard counter's sort key.
#[must_use]
pub fn join_shard_sort_key(join: JoinId, shard: u16) -> String {
    format!(
        "JOIN#{}#S#{:0>width$}",
        join.0.as_hyphenated(),
        shard,
        width = SHARD_WIDTH
    )
}

/// One mailbox entry's sort key.
#[must_use]
pub fn mailbox_sort_key(seq: u64) -> String {
    format!("MBOX#{seq:0>SEQ_WIDTH$}")
}

/// One preview event's sort key.
#[must_use]
pub fn preview_sort_key(event_seq: u64) -> String {
    format!("P#{event_seq:0>SEQ_WIDTH$}")
}

/// The session-level Brain budget item's sort key.
#[must_use]
pub fn session_budget_sort_key() -> String {
    format!("{BRAIN_PREFIX}BUDGET")
}

/// One paged fanout intent's sort key under the spawning parent.
#[must_use]
pub fn fanout_intent_sort_key(intent: FanoutIntentId) -> String {
    format!("{BRAIN_PREFIX}FANOUT#{}", intent.0.as_hyphenated())
}

/// Which shard a join member's terminal increments.
///
/// Sharding the counter is what removes the write hot spot: a measured 1 000-leaf join on a
/// single counter produced 885 first-pass conflicts against 191 on 64 shards. The counter
/// remains a projection, never completion truth.
#[must_use]
pub fn join_shard_for(child: AgentId, shards: u16) -> u16 {
    if shards <= 1 {
        return 0;
    }
    let digest = blake3::hash(child.0.as_bytes());
    let bytes = digest.as_bytes();
    let value = u16::from_be_bytes([bytes[0], bytes[1]]);
    value % shards
}

#[cfg(test)]
mod tests {
    use super::{
        BRAIN_PREFIX, agent_partition, child_index_sort_key, control_sort_key, effect_sort_key,
        fanout_intent_sort_key, join_shard_for, join_shard_sort_key, join_sort_key,
        journal_sort_key, mailbox_sort_key, preview_sort_key, queued_index_sort_key,
        session_budget_sort_key, session_partition,
    };
    use aex_brain_domain::ids::{
        AgentId, AgentKey, EffectId, FanoutIntentId, JoinId, JournalSeq, SessionId,
    };
    use uuid::Uuid;

    fn key() -> AgentKey {
        AgentKey::new(SessionId(Uuid::from_u128(1)), AgentId(Uuid::from_u128(2)))
    }

    #[test]
    fn sequence_sort_keys_order_numerically() {
        let mut keys: Vec<String> = [0_u64, 1, 9, 10, 99, 100, 1_000, u64::MAX]
            .into_iter()
            .map(|seq| journal_sort_key(JournalSeq(seq)))
            .collect();
        let sorted = {
            let mut copy = keys.clone();
            copy.sort();
            copy
        };
        assert_eq!(keys, sorted, "lexicographic order must be numeric order");
        keys.dedup();
        assert_eq!(keys.len(), 8, "every sequence has a distinct key");
    }

    #[test]
    fn per_agent_items_live_in_their_own_partition() {
        let partition = agent_partition(&key());
        assert!(partition.starts_with("SESSION#"));
        assert!(partition.contains("#AGENT#"));
        for sort_key in [
            control_sort_key(),
            journal_sort_key(JournalSeq(4)),
            effect_sort_key(EffectId([3; 16])),
            child_index_sort_key(0, AgentId(Uuid::from_u128(5))),
            join_sort_key(JoinId(Uuid::from_u128(6))),
            join_shard_sort_key(JoinId(Uuid::from_u128(6)), 2),
            mailbox_sort_key(1),
            preview_sort_key(1_024),
        ] {
            assert!(
                !sort_key.starts_with(BRAIN_PREFIX),
                "a per-agent item must not claim the session-level prefix: {sort_key}"
            );
        }
    }

    #[test]
    fn every_session_level_item_claims_the_brain_prefix_and_nothing_else() {
        let partition = session_partition(SessionId(Uuid::from_u128(1)));
        assert!(!partition.contains("AGENT"));
        for sort_key in [
            session_budget_sort_key(),
            queued_index_sort_key(1, 1_767_225_600_000, AgentId(Uuid::from_u128(7))),
            fanout_intent_sort_key(FanoutIntentId(Uuid::from_u128(8))),
        ] {
            assert!(
                sort_key.starts_with(BRAIN_PREFIX),
                "a session-level item must claim exactly one prefix: {sort_key}"
            );
        }
    }

    #[test]
    fn the_queue_index_orders_by_priority_then_arrival() {
        let child = AgentId(Uuid::from_u128(9));
        let urgent = queued_index_sort_key(0, 2_000, child);
        let ordinary_early = queued_index_sort_key(9, 1_000, child);
        let ordinary_late = queued_index_sort_key(9, 2_000, child);
        assert!(urgent < ordinary_early, "priority outranks arrival");
        assert!(ordinary_early < ordinary_late, "arrival breaks the tie");
    }

    #[test]
    fn a_negative_enqueue_instant_cannot_jump_the_queue() {
        let child = AgentId(Uuid::from_u128(9));
        let skewed = queued_index_sort_key(5, -1, child);
        let epoch = queued_index_sort_key(5, 0, child);
        assert_eq!(skewed, epoch, "a bad clock must not buy priority");
    }

    #[test]
    fn join_shards_spread_and_stay_in_range() {
        for shards in [1_u16, 2, 8, 64] {
            let mut counts = vec![0_usize; usize::from(shards)];
            for seed in 0..512_u128 {
                let shard = join_shard_for(AgentId(Uuid::from_u128(seed)), shards);
                assert!(shard < shards, "{shard} out of {shards}");
                counts[usize::from(shard)] += 1;
            }
            // Not a uniformity proof, just a check that the hash is actually consulted:
            // a constant shard would defeat the whole point of sharding the counter.
            if shards > 1 {
                assert!(
                    counts.iter().filter(|count| **count > 0).count() > 1,
                    "{shards} shards but only one was ever used"
                );
            }
        }
    }

    #[test]
    fn join_shard_selection_is_deterministic() {
        let child = AgentId(Uuid::from_u128(11));
        assert_eq!(join_shard_for(child, 64), join_shard_for(child, 64));
    }
}
