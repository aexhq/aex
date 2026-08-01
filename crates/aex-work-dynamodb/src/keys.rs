//! The `regional-work` key templates, closed vocabularies and due-index
//! addressing.
//!
//! There is no literal single due partition anywhere here. Priority is folded
//! into the due time as a lead rather than carried as a separate key segment,
//! which gives priority ordering and bounded ageing from one sort key: a
//! high-priority item is served first, and an old low-priority item is
//! guaranteed to overtake a newer high-priority one.

use aex_session_dynamodb::component::{Component, KeyError, due_shard, shard4};
use aex_wire::types::Timestamp;

/// One composite key.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Key {
    /// The partition key.
    pub pk: String,
    /// The sort key.
    pub sk: String,
}

/// How many shards the due index spreads over (`work.due_shards`).
pub const DUE_SHARDS: u64 = 64;

/// The lead each priority band takes off its due time, in seconds.
///
/// Band zero is highest. A band-four item due now sorts as if it were due 30
/// minutes ago, so it eventually overtakes any newer higher-priority item and
/// no band can starve.
pub const PRIORITY_LEAD_SECONDS: [i64; 5] = [0, 5, 30, 300, 1800];

/// The closed `kind` vocabulary. A value outside it never reaches a row.
pub const KINDS: &[&str] = &[
    "agent.wake",
    "operation.step",
    "content.gc_mark",
    "content.gc_sweep",
    "content.staged_orphan",
    "registry.upload_expiry",
    "runtime.evaluate",
    "usage.storage.delta",
    "usage.compute.closure",
    "usage.transfer.authorized",
    "secret.lineage_sweep",
];

/// The closed `state` vocabulary.
pub const STATES: &[&str] = &["pending", "claimed", "done", "poisoned"];

/// Every `itemType` this table may hold.
pub const ITEM_TYPES: &[&str] = &["work", "work_dedupe", "work_cursor"];

/// The due index name.
pub const DUE_INDEX: &str = "gsi_due";

/// The due index partition key attribute.
pub const DUE_PK: &str = "dueShardPk";

/// The due index sort key attribute.
pub const DUE_SK: &str = "dueShardSk";

/// The largest typed payload a work row may carry.
///
/// The payload holds cursors, identifiers and epochs. It never holds a prompt, a
/// secret, a body or a URL, and this ceiling plus the per-kind key check is what
/// makes the `NEW_IMAGE` stream safe rather than merely intended.
pub const MAX_PAYLOAD_BYTES: usize = 4 * 1024;

/// `WORK#{work_id}` / `STATE`.
///
/// # Errors
///
/// [`KeyError`] when the work identity could not enter a key.
pub fn work(work_id: &str) -> Result<Key, KeyError> {
    let work_id = Component::parse(work_id)?;
    Ok(Key {
        pk: format!("WORK#{work_id}"),
        sk: "STATE".to_owned(),
    })
}

/// `WDEDUPE#{dedupe_key_sha256_hex}` / `CLAIM`.
///
/// The claim makes "one logical wake outstanding at a time" a durable invariant
/// rather than a queue property, so it survives a queue that redelivers, drops
/// or reorders.
///
/// # Errors
///
/// [`KeyError`] when the digest could not enter a key.
pub fn dedupe(dedupe_key_sha256_hex: &str) -> Result<Key, KeyError> {
    let digest = Component::parse(dedupe_key_sha256_hex)?;
    Ok(Key {
        pk: format!("WDEDUPE#{digest}"),
        sk: "CLAIM".to_owned(),
    })
}

/// `WCURSOR#{shard:04}` / `STATE`.
#[must_use]
pub fn cursor(shard: u16) -> Key {
    Key {
        pk: format!("WCURSOR#{}", shard4(shard)),
        sk: "STATE".to_owned(),
    }
}

/// The due index partition for `work_id`.
#[must_use]
pub fn due_partition(work_id: &str) -> String {
    format!("DUE#{}", shard4(shard_of(work_id)))
}

/// The due index partition for a shard the reconciler is sweeping.
#[must_use]
pub fn due_partition_for_shard(shard: u16) -> String {
    format!("DUE#{}", shard4(shard))
}

/// Which shard `work_id` belongs to.
///
/// # Panics
///
/// Never: [`DUE_SHARDS`] is far below `u16::MAX`.
#[must_use]
pub fn shard_of(work_id: &str) -> u16 {
    u16::try_from(due_shard(work_id, DUE_SHARDS)).expect("64 shards fit in a u16")
}

/// The due index sort key.
///
/// # Errors
///
/// [`KeyError`] when the work identity could not enter a key.
pub fn due_sort(effective_due_at: Timestamp, work_id: &str) -> Result<String, KeyError> {
    let work_id = Component::parse(work_id)?;
    Ok(format!("{}#{work_id}", effective_due_at.to_wire()))
}

/// Applies the priority lead to a due time.
///
/// # Errors
///
/// [`KeyError::Empty`] when the priority band is outside
/// [`PRIORITY_LEAD_SECONDS`], which is a planning bug rather than a data
/// condition and is therefore never clamped.
pub fn effective_due_at(due_at: Timestamp, priority: u8) -> Result<Timestamp, KeyError> {
    let lead = PRIORITY_LEAD_SECONDS
        .get(usize::from(priority))
        .ok_or(KeyError::Empty)?;
    Timestamp::from_unix_millis(due_at.unix_millis() - lead * 1_000).map_err(|_| KeyError::Empty)
}

#[cfg(test)]
mod tests {
    use aex_wire::types::Timestamp;

    use super::{
        DUE_SHARDS, PRIORITY_LEAD_SECONDS, cursor, dedupe, due_partition, due_sort,
        effective_due_at, shard_of, work,
    };

    fn stamp(millis: i64) -> Timestamp {
        Timestamp::from_unix_millis(millis).expect("in range")
    }

    #[test]
    fn no_due_partition_is_ever_a_literal_single_partition() {
        let mut seen = std::collections::BTreeSet::new();
        for index in 0..2_000 {
            seen.insert(due_partition(&format!("wrk_{index:06}")));
        }
        assert!(
            seen.len() > 32,
            "the due index collapsed onto {} partitions",
            seen.len()
        );
        assert!(seen.len() <= usize::try_from(DUE_SHARDS).expect("64 fits"));
    }

    #[test]
    fn a_shard_is_a_pure_function_of_the_work_identity() {
        assert_eq!(shard_of("wrk_1"), shard_of("wrk_1"));
        assert!(u64::from(shard_of("wrk_1")) < DUE_SHARDS);
    }

    #[test]
    fn priority_is_a_lead_on_the_due_time_and_never_a_separate_key_segment() {
        let due = stamp(1_800_000);
        let urgent = effective_due_at(due, 0).expect("band zero");
        let background = effective_due_at(due, 4).expect("band four");
        assert_eq!(urgent, due);
        assert_eq!(
            background.unix_millis(),
            due.unix_millis() - PRIORITY_LEAD_SECONDS[4] * 1_000
        );
        assert!(
            background < urgent,
            "a background item due at the same instant must sort ahead only after ageing"
        );
    }

    #[test]
    fn an_old_low_priority_item_overtakes_a_newer_high_priority_one() {
        let old_low = effective_due_at(stamp(0), 4).expect("band four");
        let new_high = effective_due_at(stamp(60_000), 0).expect("band zero");
        assert!(
            due_sort(old_low, "wrk_a").expect("a key")
                < due_sort(new_high, "wrk_b").expect("a key"),
            "bounded ageing must eventually win"
        );
    }

    #[test]
    fn a_priority_band_outside_the_table_is_refused_rather_than_clamped() {
        assert!(effective_due_at(stamp(0), 5).is_err());
        assert!(effective_due_at(stamp(0), 200).is_err());
    }

    #[test]
    fn the_due_sort_key_orders_by_time_then_by_identity() {
        let early = due_sort(stamp(1_000), "wrk_z").expect("a key");
        let late = due_sort(stamp(2_000), "wrk_a").expect("a key");
        assert!(early < late);
    }

    #[test]
    fn a_work_identity_carrying_the_separator_can_never_reach_a_key() {
        assert!(work("wrk#evil").is_err());
        assert!(dedupe("aa#bb").is_err());
        assert_eq!(work("wrk_1").expect("a key").pk, "WORK#wrk_1");
        assert_eq!(cursor(7).pk, "WCURSOR#0007");
    }
}
