//! The per-`(scope, signal, bucket)` segment directory.
//!
//! This is the structure that replaces the removed projection's sparse index and
//! it is load-bearing (decision O-06). It records the bucket's shard count and
//! its min/max positions, so a reader
//!
//! - skips empty hours for free rather than probing them,
//! - fans out over the *exact* shard count instead of guessing one, and
//! - can have that shard count raised safely, because a raise applies to the
//!   next bucket only and a reader never has to guess how many partitions to
//!   merge.
//!
//! Without it, a query over a sparse scope would either over-fan or under-read,
//! which is precisely the failure the column store used to hide.

use aex_observation_domain::keys::BucketHour;
use aex_observation_domain::signal::Signal;
use aex_wire::types::Timestamp;

/// One `SEG#`/`SEGT#` row.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Segment {
    /// Which signal the bucket holds.
    pub signal: Signal,
    /// The bucket.
    pub bucket: BucketHour,
    /// How many partitions the bucket is spread over.
    ///
    /// Fixed for the bucket's life; a raise applies to the next bucket only.
    pub shards: u8,
    /// The lowest accepted ordinal in the bucket.
    pub min_accepted_seq: u64,
    /// The highest accepted ordinal in the bucket.
    pub max_accepted_seq: u64,
    /// How many observations the bucket holds.
    pub count: u64,
    /// How many canonical bytes the bucket holds.
    pub logical_bytes: u64,
    /// The earliest event time in the bucket.
    pub min_time: Timestamp,
    /// The latest event time in the bucket.
    pub max_time: Timestamp,
}

impl Segment {
    /// The first segment of a bucket.
    #[must_use]
    pub const fn opened(
        signal: Signal,
        bucket: BucketHour,
        shards: u8,
        accepted_seq: u64,
        time: Timestamp,
    ) -> Self {
        Self {
            signal,
            bucket,
            shards,
            min_accepted_seq: accepted_seq,
            max_accepted_seq: accepted_seq,
            count: 0,
            logical_bytes: 0,
            min_time: time,
            max_time: time,
        }
    }

    /// Folds one committed range into the segment.
    ///
    /// `shards` is deliberately not a parameter: a bucket's fan-out cannot
    /// change under a reader that is already walking it.
    pub const fn advance(
        &mut self,
        accepted_lo: u64,
        accepted_hi: u64,
        count: u64,
        logical_bytes: u64,
        min_time: Timestamp,
        max_time: Timestamp,
    ) {
        if accepted_lo < self.min_accepted_seq {
            self.min_accepted_seq = accepted_lo;
        }
        if accepted_hi > self.max_accepted_seq {
            self.max_accepted_seq = accepted_hi;
        }
        self.count += count;
        self.logical_bytes += logical_bytes;
        if min_time.unix_millis() < self.min_time.unix_millis() {
            self.min_time = min_time;
        }
        if max_time.unix_millis() > self.max_time.unix_millis() {
            self.max_time = max_time;
        }
    }

    /// Whether any observation in the bucket can fall inside the window.
    #[must_use]
    pub fn intersects_time(&self, from: Timestamp, to: Timestamp) -> bool {
        self.count > 0 && self.min_time < to && from <= self.max_time
    }
}

/// Every segment of one scope, ordered by bucket.
#[derive(Clone, Debug, Default)]
pub struct SegmentDirectory {
    segments: std::collections::BTreeMap<(Signal, BucketHour), Segment>,
}

impl SegmentDirectory {
    /// An empty directory.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Records one segment.
    pub fn insert(&mut self, segment: Segment) {
        self.segments
            .insert((segment.signal, segment.bucket), segment);
    }

    /// How many segments the directory holds.
    #[must_use]
    pub fn len(&self) -> usize {
        self.segments.len()
    }

    /// Whether the directory holds nothing.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.segments.is_empty()
    }

    /// The non-empty buckets a walk must open, in bucket order.
    ///
    /// An hour with no data costs one directory read for the whole scope rather
    /// than one probe per shard, which is the whole point of the directory.
    #[must_use]
    pub fn plan(&self, signal: Signal, from: Timestamp, to: Timestamp) -> Vec<Segment> {
        self.segments
            .values()
            .filter(|segment| segment.signal == signal && segment.intersects_time(from, to))
            .copied()
            .collect()
    }

    /// How many `Query` calls a walk over the plan issues.
    ///
    /// Exactly the sum of the planned buckets' shard counts: exact fan-out, not
    /// a guess.
    #[must_use]
    pub fn fan_out(&self, signal: Signal, from: Timestamp, to: Timestamp) -> usize {
        self.plan(signal, from, to)
            .iter()
            .map(|segment| usize::from(segment.shards))
            .sum()
    }
}

#[cfg(test)]
mod tests {
    use super::{Segment, SegmentDirectory};
    use aex_observation_domain::keys::BucketHour;
    use aex_observation_domain::signal::Signal;
    use aex_wire::types::Timestamp;

    fn bucket(text: &str) -> BucketHour {
        BucketHour::parse(text).expect("fixture parses")
    }

    fn instant(text: &str) -> Timestamp {
        Timestamp::parse(text).expect("fixture parses")
    }

    fn filled(hour: &str, from: &str, to: &str, shards: u8, count: u64) -> Segment {
        let mut segment = Segment::opened(Signal::Logs, bucket(hour), shards, 0, instant(from));
        segment.advance(0, count, count, count * 16, instant(from), instant(to));
        segment
    }

    #[test]
    fn an_empty_hour_is_skipped_rather_than_probed() {
        let mut directory = SegmentDirectory::new();
        directory.insert(filled(
            "2026-08-01T09",
            "2026-08-01T09:00:00.000Z",
            "2026-08-01T09:59:59.999Z",
            4,
            10,
        ));
        directory.insert(filled(
            "2026-08-01T11",
            "2026-08-01T11:00:00.000Z",
            "2026-08-01T11:30:00.000Z",
            2,
            5,
        ));
        assert_eq!(directory.len(), 2);
        assert!(!directory.is_empty());

        let plan = directory.plan(
            Signal::Logs,
            instant("2026-08-01T09:00:00.000Z"),
            instant("2026-08-01T12:00:00.000Z"),
        );
        assert_eq!(plan.len(), 2, "the empty 10:00 hour is not in the plan");
        assert_eq!(
            directory.fan_out(
                Signal::Logs,
                instant("2026-08-01T09:00:00.000Z"),
                instant("2026-08-01T12:00:00.000Z")
            ),
            6,
            "fan-out is the exact sum of the planned buckets' shard counts"
        );
    }

    #[test]
    fn a_segment_with_no_observations_never_enters_a_plan() {
        let mut directory = SegmentDirectory::new();
        directory.insert(Segment::opened(
            Signal::Logs,
            bucket("2026-08-01T09"),
            8,
            0,
            instant("2026-08-01T09:00:00.000Z"),
        ));
        assert!(
            directory
                .plan(
                    Signal::Logs,
                    instant("2026-08-01T00:00:00.000Z"),
                    instant("2026-08-02T00:00:00.000Z"),
                )
                .is_empty()
        );
    }

    #[test]
    fn a_signal_filter_keeps_the_walk_to_its_own_segments() {
        let mut directory = SegmentDirectory::new();
        directory.insert(filled(
            "2026-08-01T09",
            "2026-08-01T09:00:00.000Z",
            "2026-08-01T09:10:00.000Z",
            1,
            3,
        ));
        assert!(
            directory
                .plan(
                    Signal::Metrics,
                    instant("2026-08-01T00:00:00.000Z"),
                    instant("2026-08-02T00:00:00.000Z"),
                )
                .is_empty()
        );
    }

    #[test]
    fn min_and_max_are_maintained_across_advances() {
        let mut segment = Segment::opened(
            Signal::Logs,
            bucket("2026-08-01T09"),
            4,
            10,
            instant("2026-08-01T09:30:00.000Z"),
        );
        segment.advance(
            5,
            20,
            16,
            256,
            instant("2026-08-01T09:00:00.000Z"),
            instant("2026-08-01T09:59:00.000Z"),
        );
        assert_eq!(segment.min_accepted_seq, 5);
        assert_eq!(segment.max_accepted_seq, 20);
        assert_eq!(segment.count, 16);
        assert_eq!(segment.logical_bytes, 256);
        assert_eq!(segment.min_time, instant("2026-08-01T09:00:00.000Z"));
        assert_eq!(segment.max_time, instant("2026-08-01T09:59:00.000Z"));
        assert_eq!(segment.shards, 4, "a bucket's fan-out never changes");
    }
}
