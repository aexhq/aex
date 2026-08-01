//! Deterministic identifier generation.
//!
//! Fixtures never call a random generator. Two runs of the same test with the
//! same seed produce the same identifiers, so a golden assertion and a failure
//! report both stay stable.

use uuid::{Builder, Uuid};

use crate::clock::EPOCH_MILLIS;

/// A deterministic, time-ordered identifier source.
///
/// Identifiers are `UUID` version 7 values whose timestamp advances by one
/// millisecond per identifier from [`EPOCH_MILLIS`], so they sort in issue order
/// exactly as production identifiers do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IdFactory {
    state: u64,
    issued: u64,
}

impl IdFactory {
    /// An identifier source for `seed`.
    #[must_use]
    pub const fn new(seed: u64) -> Self {
        Self {
            state: seed,
            issued: 0,
        }
    }

    /// The next identifier.
    pub fn next_id(&mut self) -> Uuid {
        let millis = EPOCH_MILLIS.saturating_add(self.issued);
        self.issued = self.issued.saturating_add(1);
        let [a0, a1, a2, a3, a4, a5, a6, a7] = self.mix().to_be_bytes();
        let [b0, b1, _, _, _, _, _, _] = self.mix().to_be_bytes();
        Builder::from_unix_timestamp_millis(millis, &[a0, a1, a2, a3, a4, a5, a6, a7, b0, b1])
            .into_uuid()
    }

    /// How many identifiers this source has issued.
    #[must_use]
    pub const fn issued(&self) -> u64 {
        self.issued
    }

    fn mix(&mut self) -> u64 {
        self.state = self.state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut mixed = self.state;
        mixed = (mixed ^ (mixed >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        mixed = (mixed ^ (mixed >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        mixed ^ (mixed >> 31)
    }
}

#[cfg(test)]
mod tests {
    use super::IdFactory;

    #[test]
    fn the_same_seed_produces_the_same_sequence() {
        let mut left = IdFactory::new(42);
        let mut right = IdFactory::new(42);
        for _ in 0..128 {
            assert_eq!(left.next_id(), right.next_id());
        }
    }

    #[test]
    fn different_seeds_diverge_immediately() {
        assert_ne!(IdFactory::new(1).next_id(), IdFactory::new(2).next_id());
    }

    #[test]
    fn identifiers_are_version_seven_and_unique() {
        let mut factory = IdFactory::new(7);
        let mut seen = Vec::with_capacity(256);
        for _ in 0..256 {
            let id = factory.next_id();
            assert_eq!(id.get_version_num(), 7, "{id}");
            seen.push(id);
        }
        let mut sorted = seen.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), seen.len(), "identifiers must be unique");
        assert_eq!(sorted, seen, "identifiers must be issued in sort order");
        assert_eq!(factory.issued(), 256);
    }
}
