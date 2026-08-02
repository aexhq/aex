//! The ordering tuple every observation read is sorted by.
//!
//! `(time-or-accepted, signalRank, observationId, revision)`. Every component is
//! immutable once written, which is what makes the order stable across pages and
//! across a snapshot boundary: a page boundary can never mean something
//! different on the next request.

use std::cmp::Ordering;

use aex_wire::ids::ObservationId;
use aex_wire::types::Timestamp;

use crate::signal::Signal;

/// Which component the walk is keyed on.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum OrderBy {
    /// The observation's event time — the default, served by the time indexes.
    #[default]
    Time,
    /// The accepted position — the strongly consistent base-table order.
    Accepted,
}

impl OrderBy {
    /// Every value, in declared order.
    pub const ALL: &'static [OrderBy] = &[OrderBy::Time, OrderBy::Accepted];

    /// The wire spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Time => "time",
            Self::Accepted => "accepted",
        }
    }
}

/// Which direction the walk runs.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum Direction {
    /// Oldest first.
    #[default]
    Ascending,
    /// Newest first.
    Descending,
}

impl Direction {
    /// Every value, in declared order.
    pub const ALL: &'static [Direction] = &[Direction::Ascending, Direction::Descending];

    /// The wire spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Ascending => "ascending",
            Self::Descending => "descending",
        }
    }

    /// Compares two tuples in this direction.
    #[must_use]
    pub fn compare(self, left: &OrderTuple, right: &OrderTuple) -> Ordering {
        match self {
            Self::Ascending => left.cmp(right),
            Self::Descending => right.cmp(left),
        }
    }

    /// The direction reversed.
    #[must_use]
    pub const fn reversed(self) -> Self {
        match self {
            Self::Ascending => Self::Descending,
            Self::Descending => Self::Ascending,
        }
    }
}

impl From<aex_wire::models::ObservationOrder> for Direction {
    fn from(value: aex_wire::models::ObservationOrder) -> Self {
        match value {
            aex_wire::models::ObservationOrder::Ascending => Self::Ascending,
            aex_wire::models::ObservationOrder::Descending => Self::Descending,
        }
    }
}

/// The immutable ordering tuple.
///
/// `Ord` is derived, and the field order *is* the tuple order: primary position,
/// then signal rank, then observation id, then revision.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct OrderTuple {
    /// The primary component: either the event time or the accepted time,
    /// depending on the walk's [`OrderBy`].
    pub primary: Timestamp,
    /// The signal rank, so ties across signals are deterministic.
    pub signal_rank: u8,
    /// The observation identity.
    pub observation_id: ObservationId,
    /// The revision, so a later revision of one observation follows its
    /// predecessor rather than replacing it.
    pub revision: u64,
}

impl OrderTuple {
    /// Builds a tuple.
    #[must_use]
    pub const fn new(
        primary: Timestamp,
        signal: Signal,
        observation_id: ObservationId,
        revision: u64,
    ) -> Self {
        Self {
            primary,
            signal_rank: signal.rank(),
            observation_id,
            revision,
        }
    }

    /// The signal this tuple's rank names.
    ///
    /// # Panics
    ///
    /// Never: `signal_rank` is only ever set from [`Signal::rank`].
    #[must_use]
    pub fn signal(&self) -> Signal {
        Signal::ALL
            .iter()
            .copied()
            .find(|signal| signal.rank() == self.signal_rank)
            .expect("an OrderTuple rank always names a signal")
    }
}

/// Renders the canonical physical sort key for an ordering tuple.
///
/// Every observation index uses this exact spelling, so provider order and the
/// public merge order are the same order. Fixed-width numeric fields preserve
/// lexical ordering, and the `obs_` `UUIDv7` spelling preserves identifier order.
#[must_use]
pub fn order_sort_key(tuple: OrderTuple) -> String {
    format!(
        "{}#{:03}#{}#{:020}",
        tuple.primary.to_wire(),
        tuple.signal_rank,
        tuple.observation_id,
        tuple.revision
    )
}

/// The position a walk resumes from inside one segment.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SegmentPosition {
    /// Which signal the segment carries.
    pub signal: Signal,
    /// Which shard of the bucket.
    pub shard: u8,
    /// The last sort key delivered from this shard.
    pub last_sk: Box<str>,
}

#[cfg(test)]
mod tests {
    use super::{Direction, OrderBy, OrderTuple, order_sort_key};
    use crate::signal::Signal;
    use aex_wire::ids::ObservationId;
    use aex_wire::ids::{PrefixedId, Uuid7};
    use aex_wire::types::Timestamp;

    fn id(byte: u8) -> ObservationId {
        ObservationId::from_uuid7(Uuid7::compose(1, [byte; 10]))
    }

    fn instant(millis: i64) -> Timestamp {
        Timestamp::from_unix_millis(millis).expect("representable")
    }

    #[test]
    fn the_tuple_reports_the_signal_its_rank_names() {
        for signal in Signal::ALL {
            let tuple = OrderTuple::new(instant(0), *signal, id(1), 0);
            assert_eq!(tuple.signal(), *signal);
        }
    }

    #[test]
    fn reversing_a_direction_twice_is_the_identity() {
        for direction in Direction::ALL {
            assert_eq!(direction.reversed().reversed(), *direction);
        }
    }

    #[test]
    fn the_wire_spellings_are_stable() {
        assert_eq!(OrderBy::Time.as_str(), "time");
        assert_eq!(OrderBy::Accepted.as_str(), "accepted");
        assert_eq!(Direction::Ascending.as_str(), "ascending");
        assert_eq!(Direction::Descending.as_str(), "descending");
        assert_eq!(OrderBy::default(), OrderBy::Time);
        assert_eq!(Direction::default(), Direction::Ascending);
    }

    #[test]
    fn physical_sort_keys_have_the_same_order_as_merge_tuples() {
        let tuples = [
            OrderTuple::new(instant(1), Signal::Events, id(1), 1),
            OrderTuple::new(instant(1), Signal::Logs, id(1), 1),
            OrderTuple::new(instant(1), Signal::Logs, id(2), 1),
            OrderTuple::new(instant(1), Signal::Logs, id(2), 2),
            OrderTuple::new(instant(2), Signal::Events, id(1), 1),
        ];
        for pair in tuples.windows(2) {
            assert!(pair[0] < pair[1]);
            assert!(order_sort_key(pair[0]) < order_sort_key(pair[1]));
        }
    }
}
