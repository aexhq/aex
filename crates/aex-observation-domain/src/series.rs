//! Metric series identity and the workspace cardinality fence.
//!
//! A metric series is identified by the exact input set the implementation
//! being replaced already used — `{workspaceId, name, kind, unit, temporality,
//! monotonic, attributes}` — kept because it is a working oracle and changing
//! it would silently re-partition every existing claim.
//!
//! [`SeriesClaims`] is the sequential model the store's conditional `ADD` over
//! the 256 `SERIESCT#` counter shards plus `attribute_not_exists` per `SERIES#`
//! claim must reproduce. A batch that would cross the ceiling is refused
//! **whole**: a partially-claimed batch would leave a workspace paying for
//! cardinality it never got to use.

use sha2::{Digest as _, Sha256};

/// Why a claim was refused.
#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
pub enum SeriesError {
    /// The batch would take the workspace past its series ceiling.
    #[error(
        "claiming {attempted} new series would exceed the {ceiling}-series ceiling ({claimed} already claimed)"
    )]
    CardinalityExceeded {
        /// The effective workspace ceiling.
        ceiling: u64,
        /// How many series were already claimed.
        claimed: u64,
        /// How many *new* series this batch introduces.
        attempted: u64,
    },
}

/// The identity of one metric series.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct SeriesHash([u8; 32]);

impl SeriesHash {
    /// Wraps a raw digest.
    #[must_use]
    pub const fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    /// The raw digest.
    #[must_use]
    pub const fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    /// The lowercase-hex rendering used in the `SERIES#` key.
    #[must_use]
    pub fn to_hex(self) -> String {
        use std::fmt::Write as _;
        let mut out = String::with_capacity(64);
        for byte in self.0 {
            let _ = write!(out, "{byte:02x}");
        }
        out
    }

    /// The first sixteen hex characters, used as the `gsi_metric` sort-key
    /// tie-break where the full digest would waste index bytes.
    #[must_use]
    pub fn to_hex16(self) -> String {
        use std::fmt::Write as _;
        let mut out = String::with_capacity(16);
        for byte in &self.0[..8] {
            let _ = write!(out, "{byte:02x}");
        }
        out
    }

    /// Computes the series identity over the pinned input set.
    ///
    /// Every field is length-prefixed, so no concatenation of two fields can
    /// collide with another pair, and attributes are sorted by key before
    /// hashing, so attribute order is not part of identity.
    #[must_use]
    pub fn compute(
        workspace_id: &str,
        name: &str,
        kind: &str,
        unit: &str,
        temporality: &str,
        monotonic: bool,
        attributes: &[(String, String)],
    ) -> Self {
        let mut hasher = Sha256::new();
        for field in [workspace_id, name, kind, unit, temporality] {
            hasher.update(u64::try_from(field.len()).unwrap_or(u64::MAX).to_be_bytes());
            hasher.update(field.as_bytes());
        }
        hasher.update([u8::from(monotonic)]);
        let mut sorted: Vec<&(String, String)> = attributes.iter().collect();
        sorted.sort_by(|left, right| left.0.cmp(&right.0).then_with(|| left.1.cmp(&right.1)));
        hasher.update(
            u64::try_from(sorted.len())
                .unwrap_or(u64::MAX)
                .to_be_bytes(),
        );
        for (key, value) in sorted {
            hasher.update(u64::try_from(key.len()).unwrap_or(u64::MAX).to_be_bytes());
            hasher.update(key.as_bytes());
            hasher.update(u64::try_from(value.len()).unwrap_or(u64::MAX).to_be_bytes());
            hasher.update(value.as_bytes());
        }
        Self(hasher.finalize().into())
    }
}

/// What one batch's claim actually took.
///
/// Carried by the admission receipt so an abort releases exactly what the
/// preparation reserved, and never more.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ClaimOutcome {
    /// How many series this batch introduced that no prior batch had claimed.
    pub newly_claimed: u64,
    /// Exactly which series those were.
    pub series: Vec<SeriesHash>,
}

/// The workspace's claimed-series set under its cardinality ceiling.
#[derive(Clone, Debug)]
pub struct SeriesClaims {
    ceiling: u64,
    claimed: std::collections::BTreeSet<SeriesHash>,
}

impl SeriesClaims {
    /// A workspace with nothing claimed yet.
    #[must_use]
    pub fn new(ceiling: u64) -> Self {
        Self {
            ceiling,
            claimed: std::collections::BTreeSet::new(),
        }
    }

    /// The effective ceiling.
    #[must_use]
    pub const fn ceiling(&self) -> u64 {
        self.ceiling
    }

    /// How many series are claimed.
    #[must_use]
    pub fn claimed(&self) -> u64 {
        self.claimed.len() as u64
    }

    /// Whether one series is already claimed.
    #[must_use]
    pub fn is_claimed(&self, series: SeriesHash) -> bool {
        self.claimed.contains(&series)
    }

    /// Claims every series a batch introduces, or none of them.
    ///
    /// Duplicates inside one batch count once, and a series another batch
    /// already claimed costs nothing.
    ///
    /// # Errors
    ///
    /// Returns [`SeriesError::CardinalityExceeded`] when the batch's novel
    /// series would take the workspace past its ceiling. Nothing is claimed in
    /// that case.
    pub fn claim_all(&mut self, series: &[SeriesHash]) -> Result<ClaimOutcome, SeriesError> {
        let novel: std::collections::BTreeSet<SeriesHash> = series
            .iter()
            .copied()
            .filter(|hash| !self.claimed.contains(hash))
            .collect();
        let attempted = novel.len() as u64;
        let claimed = self.claimed();
        if claimed.saturating_add(attempted) > self.ceiling {
            return Err(SeriesError::CardinalityExceeded {
                ceiling: self.ceiling,
                claimed,
                attempted,
            });
        }
        let taken: Vec<SeriesHash> = novel.into_iter().collect();
        for hash in &taken {
            self.claimed.insert(*hash);
        }
        Ok(ClaimOutcome {
            newly_claimed: attempted,
            series: taken,
        })
    }

    /// Releases exactly what one claim took.
    ///
    /// Idempotent: releasing an outcome twice leaves the count where the first
    /// release left it rather than driving it negative.
    pub fn release(&mut self, outcome: &ClaimOutcome) {
        for hash in &outcome.series {
            self.claimed.remove(hash);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{SeriesClaims, SeriesHash};

    #[test]
    fn attribute_order_is_not_part_of_series_identity() {
        let forward = SeriesHash::compute(
            "ws_1",
            "m",
            "sum",
            "1",
            "delta",
            true,
            &[
                ("a".to_owned(), "1".to_owned()),
                ("b".to_owned(), "2".to_owned()),
            ],
        );
        let reverse = SeriesHash::compute(
            "ws_1",
            "m",
            "sum",
            "1",
            "delta",
            true,
            &[
                ("b".to_owned(), "2".to_owned()),
                ("a".to_owned(), "1".to_owned()),
            ],
        );
        assert_eq!(forward, reverse);
    }

    #[test]
    fn a_duplicate_inside_one_batch_is_claimed_once() {
        let mut claims = SeriesClaims::new(4);
        let hash = SeriesHash::from_bytes([1; 32]);
        let outcome = claims.claim_all(&[hash, hash, hash]).expect("claims");
        assert_eq!(outcome.newly_claimed, 1);
        assert_eq!(claims.claimed(), 1);
        assert_eq!(claims.ceiling(), 4);
    }

    #[test]
    fn the_hex_renderings_are_the_expected_widths() {
        let hash = SeriesHash::from_bytes([0xab; 32]);
        assert_eq!(hash.to_hex().len(), 64);
        assert_eq!(hash.to_hex16().len(), 16);
        assert_eq!(hash.as_bytes()[0], 0xab);
    }
}
