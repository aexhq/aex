//! Deterministic observation and usage fixtures.
//!
//! Batches carry an explicit sequence and record count so a duplicate, a gap and
//! an out-of-order delivery can each be constructed exactly, rather than
//! provoked by timing.

use time::OffsetDateTime;
use uuid::Uuid;

use crate::clock::TestClock;
use crate::ids::IdFactory;
use crate::prefix::{PrefixError, RunPrefix};

/// The four launch meters, in registry order.
pub const METERS: [&str; 4] = [
    "compute.millicpu_ms.v1",
    "data_transfer.egress_byte.v1",
    "memory.byte_ms.v1",
    "storage.byte_min.v1",
];

/// One admitted observation batch and its receipt shape.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObservationBatchFixture {
    /// Batch identifier.
    pub id: Uuid,
    /// Series the batch belongs to.
    pub series_id: Uuid,
    /// Position of the batch within its series.
    pub sequence: u64,
    /// How many records the batch carries.
    pub record_count: u32,
    /// Content-addressed digest of the admitted bytes.
    pub digest: String,
    /// Admission instant read from the injected clock.
    pub admitted_at: OffsetDateTime,
}

/// One immutable usage fact.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UsageFactFixture {
    /// Fact identifier.
    pub id: Uuid,
    /// One of the four launch meters.
    pub meter: String,
    /// Measured quantity in the meter's own unit.
    pub quantity: u64,
    /// Position in the category's accepted sequence.
    pub sequence: u64,
    /// Record instant read from the injected clock.
    pub recorded_at: OffsetDateTime,
}

/// Builds observation and usage fixtures from one clock, seed and run prefix.
#[derive(Debug, Clone)]
pub struct ObservationFixtures {
    clock: TestClock,
    ids: IdFactory,
    prefix: RunPrefix,
    issued: u64,
}

impl ObservationFixtures {
    /// A fixture factory bound to `prefix` and `seed`.
    #[must_use]
    pub fn new(prefix: RunPrefix, seed: u64) -> Self {
        Self {
            clock: TestClock::new(),
            ids: IdFactory::new(seed),
            prefix,
            issued: 0,
        }
    }

    /// The injected clock, so a test can advance it between fixtures.
    #[must_use]
    pub const fn clock(&self) -> &TestClock {
        &self.clock
    }

    /// Mutable access to the injected clock.
    pub const fn clock_mut(&mut self) -> &mut TestClock {
        &mut self.clock
    }

    /// The run prefix every synthetic name is built from.
    #[must_use]
    pub const fn prefix(&self) -> &RunPrefix {
        &self.prefix
    }

    /// A fresh series identifier for this run.
    ///
    /// # Errors
    ///
    /// Returns [`PrefixError`] when the run prefix rejects the generated name.
    pub fn series_name(&mut self) -> Result<String, PrefixError> {
        let ordinal = self.issued;
        self.issued = self.issued.saturating_add(1);
        self.prefix.resource(&format!("series-{ordinal:04}"))
    }

    /// The batch at `sequence` in `series_id`, carrying `record_count` records.
    ///
    /// Passing an explicit sequence is what makes duplicate, gap and disorder
    /// cases constructible rather than incidental.
    #[must_use]
    pub fn batch(
        &mut self,
        series_id: Uuid,
        sequence: u64,
        record_count: u32,
    ) -> ObservationBatchFixture {
        let id = self.ids.next_id();
        ObservationBatchFixture {
            id,
            series_id,
            sequence,
            record_count,
            digest: format!("blake3:{}", id.simple()),
            admitted_at: self.clock.tick(),
        }
    }

    /// The next usage fact for `meter_index`, taken from [`METERS`].
    #[must_use]
    pub fn usage_fact(&mut self, meter_index: usize, quantity: u64) -> UsageFactFixture {
        let sequence = self.issued.saturating_add(1);
        self.issued = sequence;
        let meter = METERS
            .get(meter_index % METERS.len())
            .copied()
            .unwrap_or(METERS[0]);
        UsageFactFixture {
            id: self.ids.next_id(),
            meter: meter.to_owned(),
            quantity,
            sequence,
            recorded_at: self.clock.tick(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{METERS, ObservationFixtures};
    use crate::prefix::RunPrefix;
    use uuid::Uuid;

    fn factory() -> ObservationFixtures {
        let prefix = RunPrefix::deterministic("observation", "0001").expect("a valid label");
        ObservationFixtures::new(prefix, 3)
    }

    #[test]
    fn the_same_seed_and_prefix_produce_identical_fixtures() {
        let series = Uuid::from_u128(21);
        let mut left = factory();
        let mut right = factory();
        for sequence in 0..8 {
            assert_eq!(
                left.batch(series, sequence, 10),
                right.batch(series, sequence, 10)
            );
            assert_eq!(left.usage_fact(1, 4_096), right.usage_fact(1, 4_096));
        }
    }

    #[test]
    fn a_duplicate_sequence_is_directly_constructible() {
        let series = Uuid::from_u128(1);
        let mut fixtures = factory();
        let first = fixtures.batch(series, 7, 3);
        let duplicate = fixtures.batch(series, 7, 3);
        assert_eq!(first.sequence, duplicate.sequence);
        assert_ne!(
            first.id, duplicate.id,
            "a duplicate sequence still gets its own identity"
        );
    }

    #[test]
    fn batch_digests_are_derived_from_the_batch_identity() {
        let mut fixtures = factory();
        let batch = fixtures.batch(Uuid::from_u128(2), 0, 1);
        assert_eq!(batch.digest, format!("blake3:{}", batch.id.simple()));
    }

    #[test]
    fn usage_facts_cover_the_four_launch_meters() {
        let mut fixtures = factory();
        let mut meters: Vec<String> = (0..METERS.len())
            .map(|index| fixtures.usage_fact(index, 1).meter)
            .collect();
        meters.sort_unstable();
        assert_eq!(meters, METERS.to_vec());
    }

    #[test]
    fn every_series_name_lives_under_the_run_prefix() {
        let mut fixtures = factory();
        let name = fixtures.series_name().expect("a fixture name is accepted");
        assert!(RunPrefix::is_synthetic(&name), "{name}");
        assert!(name.starts_with(fixtures.prefix().as_str()));
    }
}
