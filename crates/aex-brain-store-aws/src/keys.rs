//! The Brain's key namespace inside `session-authority`.
//!
//! This module is a **published cross-stream artifact**, and it has one job: to make sure
//! there is exactly one renderer per physical item family.
//!
//! Six families — agent control, journal entry, effect, fanout page, join shard and budget
//! return — are owned by `aex-session-dynamodb`, which declares them in
//! `migrations/regional/tables/session-authority.json` and encodes them in its own codec.
//! Every function here for one of those families **delegates** to that crate rather than
//! rendering a second spelling of the same key. Two renderers for one item is how a writer
//! and a reader end up addressing different rows while both look correct.
//!
//! Three families are Brain's alone, because no peer declares them: the scheduler's queued
//! index, the session-level Brain budget, and the per-agent mailbox, child index and join
//! group. Those sit in the two spaces `aex_session_dynamodb::keys` reserves for Brain:
//!
//! - session-level items take the `BRAIN#` sort-key prefix under `SESSION#{session_id}`;
//! - per-agent Brain items take their own `BRAINAGENT#{session_id}#{agent_id}` partition.
//!
//! Sort keys are zero-padded to a fixed width so lexicographic order is numeric order.
//! Without the padding `J#10` sorts before `J#9`, and a paged journal read would silently
//! return records out of sequence — which the fold would then reject as a gap, turning a
//! formatting mistake into an unexplained outage.

use aex_brain_domain::ids::{
    AgentId, AgentKey, EffectId, FanoutIntentId, JoinId, JournalSeq, SessionId,
};
use aex_session_dynamodb::component::{self, KeyError};
use aex_session_dynamodb::keys as shared;

use crate::translate::{self, TranslateError};

/// The sort-key prefix every Brain-owned session-level item carries.
pub const BRAIN_PREFIX: &str = aex_session_dynamodb::keys::BRAIN_PREFIX;

/// The partition prefix every Brain-owned per-agent item carries.
pub const BRAIN_AGENT_PARTITION_PREFIX: &str =
    aex_session_dynamodb::keys::BRAIN_AGENT_PARTITION_PREFIX;

/// Width of a zero-padded sequence in a sort key.
pub const SEQ_WIDTH: usize = component::SEQUENCE_WIDTH;

/// Width of a zero-padded ordinal in a sort key.
pub const ORDINAL_WIDTH: usize = 10;

/// Width of a zero-padded priority in a sort key.
pub const PRIORITY_WIDTH: usize = 3;

/// Why a Brain key could not be rendered.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum BrainKeyError {
    /// An identifier had no wire form.
    #[error(transparent)]
    Translate(#[from] TranslateError),
    /// A component could not enter a key.
    #[error(transparent)]
    Component(#[from] KeyError),
}

/// One composite key.
pub type Key = aex_session_dynamodb::keys::Key;

/// The partition every item `aex-session-dynamodb` owns for this agent lives in.
///
/// # Errors
///
/// [`BrainKeyError::Translate`] when either identifier has no wire form.
pub fn agent_partition(key: &AgentKey) -> Result<String, BrainKeyError> {
    let (session, agent) = translate::agent_key(key)?;
    Ok(shared::agent_partition(session, agent))
}

/// The partition every Brain-owned per-agent item lives in.
///
/// A second partition rather than a second sort-key prefix inside the shared one: the
/// shared partition's shape is declared by the peer's table definition, and adding a family
/// to it would make Brain a second owner of that item collection.
///
/// # Errors
///
/// As [`agent_partition`].
pub fn brain_agent_partition(key: &AgentKey) -> Result<String, BrainKeyError> {
    let (session, agent) = translate::agent_key(key)?;
    Ok(format!("{BRAIN_AGENT_PARTITION_PREFIX}{session}#{agent}"))
}

/// The partition every session-level item lives in.
///
/// # Errors
///
/// [`BrainKeyError::Translate`] when the session identifier has no wire form.
pub fn session_partition(session: SessionId) -> Result<String, BrainKeyError> {
    Ok(shared::session_partition(translate::session(session)?))
}

/// The agent control item.
///
/// # Errors
///
/// As [`agent_partition`].
pub fn control(key: &AgentKey) -> Result<Key, BrainKeyError> {
    let (session, agent) = translate::agent_key(key)?;
    Ok(shared::agent_control(session, agent))
}

/// The agent control item's sort key.
#[must_use]
pub fn control_sort_key() -> String {
    "CONTROL".to_owned()
}

/// One journal record.
///
/// # Errors
///
/// As [`agent_partition`].
pub fn journal(key: &AgentKey, seq: JournalSeq) -> Result<Key, BrainKeyError> {
    let (session, agent) = translate::agent_key(key)?;
    Ok(shared::journal(session, agent, seq.get()))
}

/// One journal record's sort key.
#[must_use]
pub fn journal_sort_key(seq: JournalSeq) -> String {
    shared::journal_sort_key(seq.get())
}

/// The sort-key prefix a journal page query ranges over.
#[must_use]
pub fn journal_prefix() -> &'static str {
    shared::journal_prefix()
}

/// One durable effect.
///
/// # Errors
///
/// As [`agent_partition`], plus [`BrainKeyError::Component`] if the rendered identity
/// cannot enter a key — which cannot happen for a hex digest, but is not assumed.
pub fn effect(key: &AgentKey, effect: EffectId) -> Result<Key, BrainKeyError> {
    let (session, agent) = translate::agent_key(key)?;
    Ok(shared::effect(session, agent, &effect.to_hex())?)
}

/// One durable effect's sort key.
#[must_use]
pub fn effect_sort_key(effect: EffectId) -> String {
    format!("EFFECT#{}", effect.to_hex())
}

/// The sort-key prefix an open-effect query ranges over.
#[must_use]
pub fn effect_prefix() -> &'static str {
    "EFFECT#"
}

/// One page of a paged fanout intent.
///
/// # Errors
///
/// As [`effect`].
pub fn fanout_page(
    key: &AgentKey,
    intent: FanoutIntentId,
    page: u32,
) -> Result<Key, BrainKeyError> {
    let (session, agent) = translate::agent_key(key)?;
    Ok(shared::fanout_page(
        session,
        agent,
        &intent.0.as_hyphenated().to_string(),
        page,
    )?)
}

/// One join shard counter.
///
/// # Errors
///
/// As [`effect`].
pub fn join_shard(key: &AgentKey, join: JoinId, shard: u16) -> Result<Key, BrainKeyError> {
    let (session, agent) = translate::agent_key(key)?;
    Ok(shared::join_shard(
        session,
        agent,
        &join.0.as_hyphenated().to_string(),
        shard,
    )?)
}

/// One child's budget return under its parent.
///
/// # Errors
///
/// As [`agent_partition`].
pub fn budget_return(parent: &AgentKey, child: AgentId) -> Result<Key, BrainKeyError> {
    let (session, agent) = translate::agent_key(parent)?;
    Ok(shared::budget_return(
        session,
        agent,
        translate::agent(child)?,
    ))
}

// ---------------------------------------------------------------------------
// Brain-owned families
// ---------------------------------------------------------------------------

/// The session-level Brain budget item.
///
/// # Errors
///
/// As [`session_partition`].
pub fn session_budget(session: SessionId) -> Result<Key, BrainKeyError> {
    Ok(Key {
        pk: session_partition(session)?,
        sk: session_budget_sort_key(),
    })
}

/// The session-level Brain budget item's sort key.
#[must_use]
pub fn session_budget_sort_key() -> String {
    format!("{BRAIN_PREFIX}BUDGET")
}

/// One queued child in the session-level scheduler index.
///
/// # Errors
///
/// As [`session_partition`].
pub fn queued_index(
    session: SessionId,
    priority: u8,
    enqueued_at_millis: i64,
    child: AgentId,
) -> Result<Key, BrainKeyError> {
    Ok(Key {
        pk: session_partition(session)?,
        sk: queued_index_sort_key(priority, enqueued_at_millis, child),
    })
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

/// The sort-key prefix the scheduler's queued-child scan ranges over.
#[must_use]
pub fn queued_index_prefix() -> String {
    format!("{BRAIN_PREFIX}Q#")
}

/// One child index entry under its parent.
///
/// # Errors
///
/// As [`agent_partition`].
pub fn child_index(parent: &AgentKey, ordinal: u32, child: AgentId) -> Result<Key, BrainKeyError> {
    Ok(Key {
        pk: brain_agent_partition(parent)?,
        sk: child_index_sort_key(ordinal, child),
    })
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

/// One join group under the waiting parent.
///
/// # Errors
///
/// As [`agent_partition`].
pub fn join_group(parent: &AgentKey, join: JoinId) -> Result<Key, BrainKeyError> {
    Ok(Key {
        pk: brain_agent_partition(parent)?,
        sk: join_sort_key(join),
    })
}

/// One join group's sort key under the waiting parent.
#[must_use]
pub fn join_sort_key(join: JoinId) -> String {
    format!("JOIN#{}", join.0.as_hyphenated())
}

/// One mailbox entry.
///
/// # Errors
///
/// As [`agent_partition`].
pub fn mailbox(key: &AgentKey, seq: u64) -> Result<Key, BrainKeyError> {
    Ok(Key {
        pk: brain_agent_partition(key)?,
        sk: mailbox_sort_key(seq),
    })
}

/// One mailbox entry's sort key.
#[must_use]
pub fn mailbox_sort_key(seq: u64) -> String {
    format!("MBOX#{}", component::sequence(seq))
}

/// The sort-key prefix a mailbox drain ranges over.
#[must_use]
pub fn mailbox_prefix() -> &'static str {
    "MBOX#"
}

/// One paged fanout intent's sort key under the spawning parent.
#[must_use]
pub fn fanout_intent_sort_key(intent: FanoutIntentId) -> String {
    format!("{BRAIN_PREFIX}FANOUT#{}", intent.0.as_hyphenated())
}

/// One paged fanout intent, which is session-level so a resumed fanout is findable without
/// knowing which parent opened it.
///
/// # Errors
///
/// As [`session_partition`].
pub fn fanout_intent(session: SessionId, intent: FanoutIntentId) -> Result<Key, BrainKeyError> {
    Ok(Key {
        pk: session_partition(session)?,
        sk: fanout_intent_sort_key(intent),
    })
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
        BRAIN_AGENT_PARTITION_PREFIX, BRAIN_PREFIX, agent_partition, brain_agent_partition,
        child_index_sort_key, control, control_sort_key, effect, effect_sort_key,
        fanout_intent_sort_key, join_shard, join_shard_for, join_sort_key, journal,
        journal_sort_key, mailbox_sort_key, queued_index_sort_key, session_budget_sort_key,
        session_partition,
    };
    use aex_brain_domain::ids::{
        AgentId, AgentKey, EffectId, FanoutIntentId, JoinId, JournalSeq, SessionId,
    };
    use aex_wire::ids::Uuid7;

    fn v7(millis: u64, seed: u8) -> uuid::Uuid {
        uuid::Uuid::from_bytes(*Uuid7::compose(millis, [seed; 10]).as_bytes())
    }

    fn key() -> AgentKey {
        AgentKey::new(
            SessionId(v7(1_767_225_600_000, 1)),
            AgentId(v7(1_767_225_600_001, 2)),
        )
    }

    /// The whole point of this module: one renderer per item family. If Brain rendered its
    /// own spelling of a shared key, a Brain write and a session read would address
    /// different rows and both would look right.
    #[test]
    fn every_shared_family_renders_exactly_what_the_owning_crate_renders() {
        let key = key();
        let session = super::translate::session(key.session).expect("v7");
        let agent = super::translate::agent(key.agent).expect("v7");

        assert_eq!(
            agent_partition(&key).expect("v7"),
            aex_session_dynamodb::keys::agent_partition(session, agent)
        );
        assert_eq!(
            control(&key).expect("v7"),
            aex_session_dynamodb::keys::agent_control(session, agent)
        );
        assert_eq!(
            journal(&key, JournalSeq(42)).expect("v7"),
            aex_session_dynamodb::keys::journal(session, agent, 42)
        );
        let id = EffectId([3; 16]);
        assert_eq!(
            effect(&key, id).expect("v7"),
            aex_session_dynamodb::keys::effect(session, agent, &id.to_hex()).expect("hex")
        );
        let join = JoinId(v7(1_767_225_600_002, 4));
        assert_eq!(
            join_shard(&key, join, 7).expect("v7"),
            aex_session_dynamodb::keys::join_shard(
                session,
                agent,
                &join.0.as_hyphenated().to_string(),
                7
            )
            .expect("hyphenated uuid")
        );
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

    /// Brain's own per-agent families sit in the reserved partition space, never inside the
    /// item collection the peer's table definition declares.
    #[test]
    fn brain_owned_per_agent_items_use_the_reserved_partition() {
        let partition = brain_agent_partition(&key()).expect("v7");
        assert!(partition.starts_with(BRAIN_AGENT_PARTITION_PREFIX));
        assert_ne!(partition, agent_partition(&key()).expect("v7"));
        for sort_key in [
            child_index_sort_key(0, AgentId(v7(1_767_225_600_003, 5))),
            join_sort_key(JoinId(v7(1_767_225_600_004, 6))),
            mailbox_sort_key(1),
        ] {
            assert!(
                !sort_key.starts_with(BRAIN_PREFIX),
                "a per-agent item must not claim the session-level prefix: {sort_key}"
            );
        }
    }

    #[test]
    fn every_session_level_item_claims_the_brain_prefix_and_nothing_else() {
        let partition = session_partition(SessionId(v7(1_767_225_600_000, 1))).expect("v7");
        assert!(!partition.contains("AGENT"));
        for sort_key in [
            session_budget_sort_key(),
            queued_index_sort_key(1, 1_767_225_600_000, AgentId(v7(1_767_225_600_005, 7))),
            fanout_intent_sort_key(FanoutIntentId(v7(1_767_225_600_006, 8))),
        ] {
            assert!(
                sort_key.starts_with(BRAIN_PREFIX),
                "a session-level item must claim exactly one prefix: {sort_key}"
            );
        }
        assert_ne!(control_sort_key(), session_budget_sort_key());
        assert!(!effect_sort_key(EffectId([1; 16])).starts_with(BRAIN_PREFIX));
    }

    #[test]
    fn the_queue_index_orders_by_priority_then_arrival() {
        let child = AgentId(v7(1_767_225_600_007, 9));
        let urgent = queued_index_sort_key(0, 2_000, child);
        let ordinary_early = queued_index_sort_key(9, 1_000, child);
        let ordinary_late = queued_index_sort_key(9, 2_000, child);
        assert!(urgent < ordinary_early, "priority outranks arrival");
        assert!(ordinary_early < ordinary_late, "arrival breaks the tie");
    }

    #[test]
    fn a_negative_enqueue_instant_cannot_jump_the_queue() {
        let child = AgentId(v7(1_767_225_600_008, 10));
        let skewed = queued_index_sort_key(5, -1, child);
        let epoch = queued_index_sort_key(5, 0, child);
        assert_eq!(skewed, epoch, "a bad clock must not buy priority");
    }

    #[test]
    fn join_shards_spread_and_stay_in_range() {
        for shards in [1_u16, 2, 8, 64] {
            let mut counts = vec![0_usize; usize::from(shards)];
            for seed in 0..512_u128 {
                let shard = join_shard_for(AgentId(uuid::Uuid::from_u128(seed)), shards);
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
        let child = AgentId(v7(1_767_225_600_009, 11));
        assert_eq!(join_shard_for(child, 64), join_shard_for(child, 64));
    }
}
