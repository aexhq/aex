//! Bounded, streaming metric aggregation.
//!
//! This is the one capability the removed column store genuinely provided
//! better, and §4.8 says so plainly. Aggregation here is a single streaming pass
//! over `gsi_metric` capped at `metric.aggregate_scan`: no rollups, no
//! materialized views, no pre-aggregated tables, because each of those is a
//! projection with a materializer, write amplification, a rebuild duty and a
//! deletion duty — exactly what A11-OBSERVATION removed.
//!
//! Naming the cap and its remedy is better than a query that silently takes
//! forty seconds or lies.

use std::collections::BTreeMap;

use aex_observation_domain::limits;

use crate::ast::QueryError;

/// What an aggregation computes.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum Calculation {
    /// How many points contributed.
    Count,
    /// Sum of the values.
    Sum,
    /// Smallest value.
    Min,
    /// Largest value.
    Max,
    /// Arithmetic mean.
    Mean,
    /// An approximate quantile, from a pinned-compression t-digest.
    Quantile,
    /// Reset-aware increase of a cumulative monotonic series.
    Increase,
    /// Reset-aware per-second rate of a cumulative monotonic series.
    Rate,
}

impl Calculation {
    /// Every calculation, in declared order.
    pub const ALL: &'static [Calculation] = &[
        Calculation::Count,
        Calculation::Sum,
        Calculation::Min,
        Calculation::Max,
        Calculation::Mean,
        Calculation::Quantile,
        Calculation::Increase,
        Calculation::Rate,
    ];

    /// The wire spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Count => "count",
            Self::Sum => "sum",
            Self::Min => "min",
            Self::Max => "max",
            Self::Mean => "mean",
            Self::Quantile => "quantile",
            Self::Increase => "increase",
            Self::Rate => "rate",
        }
    }

    /// Whether the calculation is meaningful for the instrument.
    ///
    /// `increase` and `rate` are only defined over a cumulative monotonic
    /// series; asking for them over a gauge is a request the data cannot answer,
    /// and it is refused **before any read** rather than answered with a number
    /// that means nothing.
    #[must_use]
    pub const fn is_valid_for(self, kind: InstrumentKind) -> bool {
        match self {
            Self::Increase | Self::Rate => matches!(kind, InstrumentKind::CumulativeMonotonic),
            Self::Count | Self::Min | Self::Max | Self::Mean | Self::Quantile => true,
            // Summing a cumulative counter across series is meaningless.
            Self::Sum => !matches!(kind, InstrumentKind::CumulativeMonotonic),
        }
    }
}

/// The instrument shape an aggregation is asked over.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum InstrumentKind {
    /// A gauge, or any non-monotonic instantaneous value.
    Gauge,
    /// A delta-temporality sum.
    DeltaSum,
    /// A cumulative monotonic counter.
    CumulativeMonotonic,
    /// A histogram or summary, aggregated over its scalar surface.
    Distribution,
}

/// A bounded t-digest with a pinned compression, so a quantile is reproducible.
///
/// The compression is [`limits::METRIC_TDIGEST_COMPRESSION`] and is part of the
/// answer's identity: the contract already declares the quantile approximate,
/// and pinning the compression is what stops "approximate" meaning "different
/// every time".
#[derive(Clone, Debug, Default)]
pub struct TDigest {
    /// `(mean, weight)` centroids.
    ///
    /// Weights are what make a merge distribution-preserving: two centroids
    /// collapse into their weighted mean carrying the combined weight, so a
    /// quantile is read off cumulative weight rather than off centroid count.
    centroids: Vec<(f64, u64)>,
    total: u64,
}

impl TDigest {
    /// An empty digest.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// The bounded centroid capacity, derived from the pinned compression.
    #[must_use]
    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "the pinned compression is a small positive constant"
    )]
    pub const fn capacity() -> usize {
        (limits::METRIC_TDIGEST_COMPRESSION as usize) * 20
    }

    /// Adds one value.
    pub fn add(&mut self, value: f64) {
        if !value.is_finite() {
            return;
        }
        self.centroids.push((value, 1));
        self.total += 1;
        if self.centroids.len() > Self::capacity() {
            self.compress();
        }
    }

    /// How many centroids the digest holds.
    #[must_use]
    pub fn len(&self) -> usize {
        self.centroids.len()
    }

    /// How many values contributed.
    #[must_use]
    pub const fn weight(&self) -> u64 {
        self.total
    }

    /// Whether nothing contributed.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.centroids.is_empty()
    }

    #[allow(
        clippy::cast_precision_loss,
        reason = "a centroid weight is a bounded count"
    )]
    fn compress(&mut self) {
        self.centroids
            .sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
        let merged: Vec<(f64, u64)> = self
            .centroids
            .chunks(2)
            .map(|pair| {
                let weight: u64 = pair.iter().map(|(_, w)| *w).sum();
                let mean =
                    pair.iter().map(|(mean, w)| mean * (*w as f64)).sum::<f64>() / (weight as f64);
                (mean, weight)
            })
            .collect();
        self.centroids = merged;
    }

    /// The approximate quantile, or `None` when nothing contributed.
    #[must_use]
    #[allow(
        clippy::cast_precision_loss,
        reason = "a cumulative weight compared against a quantile"
    )]
    pub fn quantile(&self, q: f64) -> Option<f64> {
        if self.centroids.is_empty() || !(0.0..=1.0).contains(&q) {
            return None;
        }
        let mut sorted = self.centroids.clone();
        sorted.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
        let target = (self.total as f64) * q;
        let mut cumulative = 0f64;
        for (mean, weight) in &sorted {
            cumulative += *weight as f64;
            if cumulative >= target {
                return Some(*mean);
            }
        }
        sorted.last().map(|(mean, _)| *mean)
    }
}

/// One series' running aggregate.
#[derive(Clone, Debug, Default)]
struct SeriesState {
    count: u64,
    sum: f64,
    min: Option<f64>,
    max: Option<f64>,
    first_time_ms: Option<i64>,
    last_time_ms: Option<i64>,
    last_value: Option<f64>,
    increase: f64,
    digest: TDigest,
}

/// A bounded streaming aggregation over raw metric points.
#[derive(Clone, Debug)]
pub struct Aggregation {
    kind: InstrumentKind,
    scan_budget: u64,
    scanned: u64,
    groups: BTreeMap<Vec<String>, SeriesState>,
    max_rows: u32,
}

impl Aggregation {
    /// Starts an aggregation.
    ///
    /// # Errors
    ///
    /// Returns [`QueryError::InvalidMetricAggregation`] for an invalid
    /// instrument/calculation pair, a group-field or calculation count above its
    /// ceiling — **before any read happens**.
    pub fn start(
        kind: InstrumentKind,
        calculations: &[Calculation],
        group_fields: usize,
    ) -> Result<Self, QueryError> {
        if calculations.is_empty() {
            return Err(QueryError::InvalidMetricAggregation {
                reason: "at least one calculation is required",
            });
        }
        if calculations.len() > limits::METRIC_MAX_CALCULATIONS {
            return Err(QueryError::InvalidMetricAggregation {
                reason: "too many calculations",
            });
        }
        if group_fields > limits::METRIC_MAX_GROUP_FIELDS {
            return Err(QueryError::InvalidMetricAggregation {
                reason: "too many group fields",
            });
        }
        for calculation in calculations {
            if !calculation.is_valid_for(kind) {
                return Err(QueryError::InvalidMetricAggregation {
                    reason: "the calculation is not defined for this instrument",
                });
            }
        }
        Ok(Self {
            kind,
            scan_budget: limits::METRIC_AGGREGATE_SCAN,
            scanned: 0,
            groups: BTreeMap::new(),
            max_rows: limits::METRIC_MAX_ROWS,
        })
    }

    /// How many raw points the pass consumed.
    #[must_use]
    pub const fn scanned(&self) -> u64 {
        self.scanned
    }

    /// How many groups the pass produced.
    #[must_use]
    pub fn rows(&self) -> usize {
        self.groups.len()
    }

    /// Whether the scan budget is spent.
    #[must_use]
    pub const fn budget_exhausted(&self) -> bool {
        self.scanned >= self.scan_budget
    }

    /// Feeds one raw point.
    ///
    /// # Errors
    ///
    /// Returns [`QueryError::Bound`] when the scan budget or the row ceiling is
    /// reached. The caller ends the page with a cursor; nothing is silently
    /// dropped.
    pub fn feed(&mut self, group: Vec<String>, time_ms: i64, value: f64) -> Result<(), QueryError> {
        if self.budget_exhausted() {
            return Err(QueryError::Bound {
                what: "metric.aggregate_scan",
                observed: usize::try_from(self.scanned).unwrap_or(usize::MAX),
                limit: usize::try_from(self.scan_budget).unwrap_or(usize::MAX),
            });
        }
        if !self.groups.contains_key(&group) && self.groups.len() >= self.max_rows as usize {
            return Err(QueryError::Bound {
                what: "aggregation rows",
                observed: self.groups.len() + 1,
                limit: self.max_rows as usize,
            });
        }
        self.scanned += 1;
        let state = self.groups.entry(group).or_default();
        state.count += 1;
        state.sum += value;
        state.min = Some(state.min.map_or(value, |current| current.min(value)));
        state.max = Some(state.max.map_or(value, |current| current.max(value)));
        state.digest.add(value);
        if state.first_time_ms.is_none() {
            state.first_time_ms = Some(time_ms);
        }
        state.last_time_ms = Some(time_ms);

        if self.kind == InstrumentKind::CumulativeMonotonic {
            // Reset-aware: a decrease means the counter restarted, and the whole
            // post-reset value is the increase.
            match state.last_value {
                Some(previous) if value >= previous => state.increase += value - previous,
                Some(_) => state.increase += value,
                None => {}
            }
            state.last_value = Some(value);
        }
        Ok(())
    }

    /// Computes one calculation per group.
    #[must_use]
    pub fn finish(&self, calculation: Calculation, quantile: f64) -> Vec<(Vec<String>, f64, u64)> {
        self.groups
            .iter()
            .map(|(group, state)| {
                #[allow(
                    clippy::cast_precision_loss,
                    reason = "an aggregate value is a double by contract"
                )]
                let value = match calculation {
                    Calculation::Count => state.count as f64,
                    Calculation::Sum => state.sum,
                    Calculation::Min => state.min.unwrap_or(0.0),
                    Calculation::Max => state.max.unwrap_or(0.0),
                    Calculation::Mean if state.count > 0 => state.sum / state.count as f64,
                    Calculation::Mean => 0.0,
                    Calculation::Quantile => state.digest.quantile(quantile).unwrap_or(0.0),
                    Calculation::Increase => state.increase,
                    Calculation::Rate => {
                        let span = state
                            .last_time_ms
                            .zip(state.first_time_ms)
                            .map(|(last, first)| last - first)
                            .unwrap_or_default();
                        if span > 0 {
                            state.increase / (span as f64 / 1_000.0)
                        } else {
                            0.0
                        }
                    }
                };
                (group.clone(), value, state.count)
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::{Aggregation, Calculation, InstrumentKind, TDigest};
    use crate::ast::QueryError;
    use aex_observation_domain::limits;

    fn key(name: &str) -> Vec<String> {
        vec![name.to_owned()]
    }

    #[test]
    fn an_invalid_instrument_calculation_pair_is_refused_before_any_read() {
        assert!(matches!(
            Aggregation::start(InstrumentKind::Gauge, &[Calculation::Rate], 0),
            Err(QueryError::InvalidMetricAggregation { .. })
        ));
        assert!(matches!(
            Aggregation::start(InstrumentKind::CumulativeMonotonic, &[Calculation::Sum], 0),
            Err(QueryError::InvalidMetricAggregation { .. })
        ));
        Aggregation::start(
            InstrumentKind::CumulativeMonotonic,
            &[Calculation::Rate, Calculation::Increase],
            2,
        )
        .expect("a cumulative monotonic counter supports rate and increase");
    }

    #[test]
    fn the_group_and_calculation_ceilings_are_exact() {
        let many: Vec<Calculation> =
            std::iter::repeat_n(Calculation::Count, limits::METRIC_MAX_CALCULATIONS + 1).collect();
        assert!(Aggregation::start(InstrumentKind::Gauge, &many, 0).is_err());
        assert!(
            Aggregation::start(
                InstrumentKind::Gauge,
                &[Calculation::Count],
                limits::METRIC_MAX_GROUP_FIELDS
            )
            .is_ok()
        );
        assert!(
            Aggregation::start(
                InstrumentKind::Gauge,
                &[Calculation::Count],
                limits::METRIC_MAX_GROUP_FIELDS + 1
            )
            .is_err()
        );
        assert!(Aggregation::start(InstrumentKind::Gauge, &[], 0).is_err());
    }

    #[test]
    fn a_cumulative_reset_adds_the_post_reset_value() {
        let mut aggregation = Aggregation::start(
            InstrumentKind::CumulativeMonotonic,
            &[Calculation::Increase],
            1,
        )
        .expect("starts");
        for (time, value) in [(0i64, 10.0), (1_000, 30.0), (2_000, 5.0), (3_000, 9.0)] {
            aggregation.feed(key("a"), time, value).expect("feeds");
        }
        // 10 -> 30 is +20; 30 -> 5 is a reset contributing 5; 5 -> 9 is +4.
        let rows = aggregation.finish(Calculation::Increase, 0.0);
        assert_eq!(rows.len(), 1);
        assert!((rows[0].1 - 29.0).abs() < f64::EPSILON, "{rows:?}");
        assert_eq!(rows[0].2, 4);
    }

    #[test]
    fn a_rate_divides_the_increase_by_the_measured_span() {
        let mut aggregation =
            Aggregation::start(InstrumentKind::CumulativeMonotonic, &[Calculation::Rate], 1)
                .expect("starts");
        aggregation.feed(key("a"), 0, 0.0).expect("feeds");
        aggregation.feed(key("a"), 2_000, 10.0).expect("feeds");
        let rows = aggregation.finish(Calculation::Rate, 0.0);
        assert!((rows[0].1 - 5.0).abs() < f64::EPSILON, "{rows:?}");
    }

    #[test]
    fn delta_sums_are_direct_addition_and_the_scalar_aggregates_agree() {
        let mut aggregation = Aggregation::start(
            InstrumentKind::DeltaSum,
            &[
                Calculation::Sum,
                Calculation::Count,
                Calculation::Min,
                Calculation::Max,
                Calculation::Mean,
            ],
            1,
        )
        .expect("starts");
        for value in [1.0, 2.0, 3.0, 4.0] {
            aggregation.feed(key("a"), 0, value).expect("feeds");
        }
        let close = |calculation: Calculation, expected: f64| {
            let observed = aggregation.finish(calculation, 0.0)[0].1;
            assert!(
                (observed - expected).abs() < f64::EPSILON,
                "{} was {observed}, expected {expected}",
                calculation.as_str()
            );
        };
        close(Calculation::Sum, 10.0);
        close(Calculation::Count, 4.0);
        close(Calculation::Min, 1.0);
        close(Calculation::Max, 4.0);
        close(Calculation::Mean, 2.5);
        assert_eq!(aggregation.scanned(), 4);
        assert_eq!(aggregation.rows(), 1);
    }

    #[test]
    fn the_row_ceiling_ends_the_pass_rather_than_dropping_a_group() {
        let mut aggregation =
            Aggregation::start(InstrumentKind::Gauge, &[Calculation::Count], 1).expect("starts");
        // Drive straight at the ceiling with distinct groups.
        for index in 0..limits::METRIC_MAX_ROWS {
            aggregation
                .feed(key(&index.to_string()), 0, 1.0)
                .expect("feeds");
        }
        let error = aggregation
            .feed(key("one-too-many"), 0, 1.0)
            .expect_err("refused");
        assert!(matches!(
            error,
            QueryError::Bound {
                what: "aggregation rows",
                ..
            }
        ));
    }

    #[test]
    fn a_quantile_is_reproducible_and_bounded() {
        let mut digest = TDigest::new();
        assert!(digest.is_empty());
        assert_eq!(digest.quantile(0.5), None);
        for value in 1..=10_000 {
            digest.add(f64::from(value));
        }
        assert!(
            digest.len() <= TDigest::capacity(),
            "the digest stays bounded regardless of how many points the scan consumed"
        );
        assert_eq!(digest.weight(), 10_000);
        let first = digest.quantile(0.5).expect("a median");
        let second = digest.quantile(0.5).expect("a median");
        assert!(
            (first - second).abs() < f64::EPSILON,
            "the same digest must answer identically"
        );
        assert!(digest.quantile(1.5).is_none());
        assert!(
            first > 4_500.0 && first < 5_500.0,
            "a weighted digest keeps the median near the true one; got {first}"
        );
    }

    #[test]
    fn the_calculation_vocabulary_is_closed() {
        assert_eq!(Calculation::ALL.len(), 8);
        assert_eq!(Calculation::Quantile.as_str(), "quantile");
        assert!(Calculation::Quantile.is_valid_for(InstrumentKind::Distribution));
    }
}
