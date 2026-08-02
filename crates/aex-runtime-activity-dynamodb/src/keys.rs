//! The `runtime-activity` key templates, closed vocabularies and due index.
//!
//! The due index is a **dedicated** index rather than a `regional-work` item per
//! evaluation, and that is the whole design: the 180-second true-idle timer
//! would otherwise cost one work-item rewrite per generation per evaluation,
//! whereas the index gives the reaper a bounded ordered scan at zero write cost
//! between evaluations.

use aex_runtime_control::generation::GenerationState;
use aex_session_dynamodb::component::{Component, KeyError, due_shard, shard2};
use aex_wire::ids::{GenerationId, SessionId};
use aex_wire::types::Timestamp;

/// One composite key.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Key {
    /// The partition key.
    pub pk: String,
    /// The sort key.
    pub sk: String,
}

/// Every `itemType` this table may hold, as declared in
/// `migrations/regional/tables/runtime-activity.json`.
pub const ITEM_TYPES: &[&str] = &[
    "hands_generation",
    "lifecycle_intent",
    "lifecycle_receipt",
    "idle_probe",
    "current_generation",
    "usage_outbox",
];

/// Every generation state, as the domain spells them.
pub const STATES: &[&str] = &[
    "requested",
    "launching",
    "running",
    "suspending",
    "suspended",
    "resuming",
    "lifetime_draining",
    "terminating",
    "terminated",
    "lost",
    "unknown",
];

/// Every lifecycle action.
pub const ACTIONS: &[&str] = &["launch", "suspend", "resume", "terminate", "snapshot"];

/// Every lifecycle intent state.
pub const INTENT_STATES: &[&str] = &[
    "prepared",
    "dispatched",
    "settled",
    "unknown",
    "quarantined",
];

/// Every lifecycle receipt outcome.
pub const OUTCOMES: &[&str] = &["succeeded", "failed", "unknown"];

/// How many shards the evaluation due index spreads over
/// (`runtime.due_shards`).
pub const DUE_SHARDS: u64 = 16;

/// The due index name.
pub const DUE_INDEX: &str = "gsi_runtime_due";

/// The due index partition key attribute.
pub const DUE_PK: &str = "rtDuePk";

/// The due index sort key attribute.
pub const DUE_SK: &str = "rtDueSk";

/// Exactly the attributes `gsi_runtime_due` projects.
pub const DUE_PROJECTION: &[&str] = &[
    "sessionId",
    "workspaceId",
    "generationId",
    "state",
    "fence",
    "revision",
    "idleSince",
    "providerLifetimeExpiresAt",
    "keepaliveLeaseUntil",
];

/// How long an idle probe is retained before TTL reclaims it.
pub const PROBE_TTL_SECONDS: i64 = 7 * 24 * 60 * 60;

/// The partition every row of one generation shares.
#[must_use]
pub fn generation_partition(_session: SessionId, generation: GenerationId) -> String {
    generation_partition_for_id(generation)
}

/// The globally addressable partition for one generation.
#[must_use]
pub fn generation_partition_for_id(generation: GenerationId) -> String {
    format!("GEN#{generation}")
}

/// The generation head.
#[must_use]
pub fn head(session: SessionId, generation: GenerationId) -> Key {
    Key {
        pk: generation_partition(session, generation),
        sk: "HEAD".to_owned(),
    }
}

/// A generation head addressed without first knowing its session.
#[must_use]
pub fn head_for_generation(generation: GenerationId) -> Key {
    Key {
        pk: generation_partition_for_id(generation),
        sk: "HEAD".to_owned(),
    }
}

/// One lifecycle intent.
///
/// # Errors
///
/// [`KeyError`] when the intent identity could not enter a key.
pub fn intent(
    session: SessionId,
    generation: GenerationId,
    intent_id: &str,
) -> Result<Key, KeyError> {
    let intent = Component::parse(intent_id)?;
    Ok(Key {
        pk: generation_partition(session, generation),
        sk: format!("INTENT#{intent}"),
    })
}

/// One immutable lifecycle receipt.
///
/// # Errors
///
/// As [`intent`].
pub fn receipt(
    session: SessionId,
    generation: GenerationId,
    intent_id: &str,
) -> Result<Key, KeyError> {
    let intent = Component::parse(intent_id)?;
    Ok(Key {
        pk: generation_partition(session, generation),
        sk: format!("RECEIPT#{intent}"),
    })
}

/// One pending usage draft, keyed by deterministic authority identity.
#[must_use]
pub fn usage_outbox(generation: GenerationId, fact_id: &str) -> Key {
    Key {
        pk: generation_partition_for_id(generation),
        sk: format!("USAGE#{fact_id}"),
    }
}

/// One true-idle probe.
#[must_use]
pub fn probe(session: SessionId, generation: GenerationId, observed_at: Timestamp) -> Key {
    Key {
        pk: generation_partition(session, generation),
        sk: format!("PROBE#{}", observed_at.to_wire()),
    }
}

/// The pointer a Brain activation reads to resolve "the exact generation".
#[must_use]
pub fn current(session: SessionId) -> Key {
    Key {
        pk: format!("SESSIONGEN#{session}"),
        sk: "CURRENT".to_owned(),
    }
}

/// Which shard a generation evaluates in.
///
/// # Panics
///
/// Never: [`DUE_SHARDS`] is far below `u8::MAX`.
#[must_use]
pub fn shard_of(generation: GenerationId) -> u8 {
    u8::try_from(due_shard(&generation.to_string(), DUE_SHARDS)).expect("16 shards fit in a u8")
}

/// The due index partition for one shard.
#[must_use]
pub fn due_partition_for_shard(shard: u8) -> String {
    format!("RTDUE#{}", shard2(shard))
}

/// The due index partition a generation belongs to.
#[must_use]
pub fn due_partition(generation: GenerationId) -> String {
    due_partition_for_shard(shard_of(generation))
}

/// The due index sort key.
#[must_use]
pub fn due_sort(next_evaluate_at: Timestamp, generation: GenerationId) -> String {
    format!("{}#{generation}", next_evaluate_at.to_wire())
}

/// The stable wire spelling of a generation state.
#[must_use]
pub const fn state_str(state: GenerationState) -> &'static str {
    match state {
        GenerationState::Requested => "requested",
        GenerationState::Launching => "launching",
        GenerationState::Running => "running",
        GenerationState::Suspending => "suspending",
        GenerationState::Suspended => "suspended",
        GenerationState::Resuming => "resuming",
        GenerationState::LifetimeDraining => "lifetime_draining",
        GenerationState::Terminating => "terminating",
        GenerationState::Terminated => "terminated",
        GenerationState::Lost => "lost",
        GenerationState::Unknown => "unknown",
    }
}

/// Resolves a stored generation state. There is no alias table.
#[must_use]
pub fn state_of(text: &str) -> Option<GenerationState> {
    GenerationState::ALL
        .into_iter()
        .find(|state| state_str(*state) == text)
}

/// Whether a state still carries the due index attributes.
///
/// A terminal transition removes them, so the reaper's scan holds only
/// generations that can still change.
#[must_use]
pub const fn is_evaluable(state: GenerationState) -> bool {
    !state.is_terminal()
}

#[cfg(test)]
mod tests {
    use aex_runtime_control::generation::GenerationState;
    use aex_wire::ids::{GenerationId, PrefixedId, SessionId, Uuid7};
    use aex_wire::types::Timestamp;

    use super::{
        DUE_SHARDS, STATES, current, due_partition, due_sort, head, is_evaluable, probe, shard_of,
        state_of, state_str,
    };

    fn session() -> SessionId {
        SessionId::from_uuid7(Uuid7::compose(1_754_051_696_789, [3; 10]))
    }

    fn generation(byte: u8) -> GenerationId {
        GenerationId::from_uuid7(Uuid7::compose(1_754_051_696_789, [byte; 10]))
    }

    #[test]
    fn every_state_has_exactly_one_stable_spelling_and_parses_back() {
        let mut seen = std::collections::BTreeSet::new();
        for state in GenerationState::ALL {
            let text = state_str(state);
            assert!(seen.insert(text), "`{text}` is used twice");
            assert_eq!(state_of(text), Some(state));
            assert!(STATES.contains(&text));
        }
        assert_eq!(seen.len(), STATES.len());
    }

    #[test]
    fn a_terminal_state_leaves_the_due_index() {
        assert!(!is_evaluable(GenerationState::Terminated));
        assert!(!is_evaluable(GenerationState::Lost));
        assert!(is_evaluable(GenerationState::Running));
        assert!(
            is_evaluable(GenerationState::Unknown),
            "an unsettled generation is exactly what the reaper must keep looking at"
        );
    }

    #[test]
    fn the_due_index_is_sharded_and_never_a_literal_single_partition() {
        let mut seen = std::collections::BTreeSet::new();
        for byte in 0..=255u8 {
            seen.insert(due_partition(generation(byte)));
        }
        assert!(
            seen.len() > 8,
            "the index collapsed onto {} shards",
            seen.len()
        );
        assert!(seen.len() <= usize::try_from(DUE_SHARDS).expect("16 fits"));
        assert!(u64::from(shard_of(generation(1))) < DUE_SHARDS);
    }

    #[test]
    fn the_due_sort_key_orders_by_evaluation_time() {
        let earlier = due_sort(
            Timestamp::from_unix_millis(1_000).expect("in range"),
            generation(9),
        );
        let later = due_sort(
            Timestamp::from_unix_millis(2_000).expect("in range"),
            generation(1),
        );
        assert!(earlier < later);
    }

    #[test]
    fn every_row_of_one_generation_shares_a_partition_and_the_pointer_does_not() {
        let head = head(session(), generation(1));
        let probe = probe(
            session(),
            generation(1),
            Timestamp::from_unix_millis(0).expect("epoch"),
        );
        assert_eq!(head.pk, probe.pk);
        assert_ne!(
            head.pk,
            current(session()).pk,
            "a Brain activation reads the pointer with one point read and must not \
             land in the generation's own partition"
        );
    }
}
