//! The recorder and the mandatory metric set.
//!
//! Every load owner reports at least [`Metric::MANDATORY`]. That is not
//! bureaucracy: a capacity claim that reports throughput without `open_fds`,
//! `tokio_tasks` and `cost_micro_usd_per_completed_turn` cannot be compared with
//! the next campaign, and a leak or a cost regression hides between two green
//! runs.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// One reported measurement.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Metric {
    /// Completed units of work per second.
    CompletedPerS,
    /// Time an offer waited before dispatch.
    QueueDelayMs,
    /// Time from dispatch to the first streamed token.
    TimeToFirstTokenMs,
    /// Time from offer to terminal.
    EndToEndMs,
    /// Time spent waiting on a model provider.
    ProviderWaitMs,
    /// CPU consumed by the system under test.
    LocalCpuMs,
    /// Peak resident set size.
    RssPeakBytes,
    /// Open file descriptors.
    OpenFds,
    /// Live Tokio tasks.
    TokioTasks,
    /// Open sockets.
    Sockets,
    /// Journal bytes written.
    JournalBytes,
    /// Checkpoint bytes written.
    CheckpointBytes,
    /// Fraction of dispatches that hit a warm affinity target.
    AffinityHitRate,
    /// Seconds between a scale trigger and capacity arriving.
    ScaleEventS,
    /// Seconds from a fault to restored progress.
    RecoveryS,
    /// Effects applied more than once.
    DuplicateEffects,
    /// Observed gaps in an ordered series.
    Gaps,
    /// Cost of one completed turn.
    CostMicroUsdPerCompletedTurn,
}

impl Metric {
    /// The metric set every load descriptor reports.
    pub const MANDATORY: [Self; 18] = [
        Self::CompletedPerS,
        Self::QueueDelayMs,
        Self::TimeToFirstTokenMs,
        Self::EndToEndMs,
        Self::ProviderWaitMs,
        Self::LocalCpuMs,
        Self::RssPeakBytes,
        Self::OpenFds,
        Self::TokioTasks,
        Self::Sockets,
        Self::JournalBytes,
        Self::CheckpointBytes,
        Self::AffinityHitRate,
        Self::ScaleEventS,
        Self::RecoveryS,
        Self::DuplicateEffects,
        Self::Gaps,
        Self::CostMicroUsdPerCompletedTurn,
    ];

    /// The metric's name in the descriptor and the receipt.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::CompletedPerS => "completed_per_s",
            Self::QueueDelayMs => "queue_delay_ms",
            Self::TimeToFirstTokenMs => "time_to_first_token_ms",
            Self::EndToEndMs => "end_to_end_ms",
            Self::ProviderWaitMs => "provider_wait_ms",
            Self::LocalCpuMs => "local_cpu_ms",
            Self::RssPeakBytes => "rss_peak_bytes",
            Self::OpenFds => "open_fds",
            Self::TokioTasks => "tokio_tasks",
            Self::Sockets => "sockets",
            Self::JournalBytes => "journal_bytes",
            Self::CheckpointBytes => "checkpoint_bytes",
            Self::AffinityHitRate => "affinity_hit_rate",
            Self::ScaleEventS => "scale_event_s",
            Self::RecoveryS => "recovery_s",
            Self::DuplicateEffects => "duplicate_effects",
            Self::Gaps => "gaps",
            Self::CostMicroUsdPerCompletedTurn => "cost_micro_usd_per_completed_turn",
        }
    }

    /// The metric named `text`, if it is one.
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        Self::MANDATORY
            .into_iter()
            .find(|metric| metric.as_str() == text)
    }
}

impl std::fmt::Display for Metric {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// The distribution of one metric over a campaign.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Summary {
    /// How many samples were recorded.
    pub count: usize,
    /// The smallest sample.
    pub min: f64,
    /// The median.
    pub p50: f64,
    /// The 95th percentile.
    pub p95: f64,
    /// The 99th percentile.
    pub p99: f64,
    /// The largest sample.
    pub max: f64,
    /// The arithmetic mean.
    pub mean: f64,
}

/// Collects samples per metric.
///
/// Samples are kept rather than streamed into a sketch: a load campaign is
/// bounded, and an exact percentile removes an argument about whether the
/// sketch or the system caused a budget miss.
#[derive(Debug, Default)]
pub struct Recorder {
    samples: BTreeMap<Metric, Vec<f64>>,
}

impl Recorder {
    /// An empty recorder.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Records one sample.
    pub fn record(&mut self, metric: Metric, value: f64) {
        self.samples.entry(metric).or_default().push(value);
    }

    /// Which metrics have at least one sample.
    #[must_use]
    pub fn recorded(&self) -> Vec<Metric> {
        self.samples
            .iter()
            .filter(|(_, values)| !values.is_empty())
            .map(|(metric, _)| *metric)
            .collect()
    }

    /// Which mandatory metrics have no sample.
    ///
    /// A campaign that leaves one unreported is not comparable with any other
    /// campaign, so this is a failure of the run, not a note in the report.
    #[must_use]
    pub fn missing_mandatory(&self) -> Vec<Metric> {
        Metric::MANDATORY
            .into_iter()
            .filter(|metric| self.samples.get(metric).is_none_or(Vec::is_empty))
            .collect()
    }

    /// The distribution of `metric`, or `None` when nothing was recorded.
    #[must_use]
    pub fn summary(&self, metric: Metric) -> Option<Summary> {
        let values = self.samples.get(&metric)?;
        if values.is_empty() {
            return None;
        }
        let mut sorted = values.clone();
        sorted.sort_by(f64::total_cmp);
        #[allow(clippy::cast_precision_loss)]
        let count = sorted.len() as f64;
        Some(Summary {
            count: sorted.len(),
            min: sorted[0],
            p50: percentile(&sorted, 0.50),
            p95: percentile(&sorted, 0.95),
            p99: percentile(&sorted, 0.99),
            max: sorted[sorted.len() - 1],
            mean: sorted.iter().sum::<f64>() / count,
        })
    }

    /// Every recorded distribution, keyed by metric name.
    #[must_use]
    pub fn report(&self) -> BTreeMap<&'static str, Summary> {
        self.recorded()
            .into_iter()
            .filter_map(|metric| {
                self.summary(metric)
                    .map(|summary| (metric.as_str(), summary))
            })
            .collect()
    }
}

/// The nearest-rank percentile of an already sorted, non-empty slice.
fn percentile(sorted: &[f64], quantile: f64) -> f64 {
    debug_assert!(!sorted.is_empty(), "percentile of an empty sample");
    #[allow(clippy::cast_precision_loss)]
    let position = (quantile * sorted.len() as f64).ceil();
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let rank = (position as usize).max(1);
    sorted[rank.min(sorted.len()) - 1]
}

#[cfg(test)]
mod tests {
    use super::{Metric, Recorder};

    #[test]
    fn the_mandatory_set_is_the_declared_eighteen_and_round_trips_by_name() {
        assert_eq!(Metric::MANDATORY.len(), 18);
        for metric in Metric::MANDATORY {
            assert_eq!(Metric::parse(metric.as_str()), Some(metric));
        }
        assert_eq!(Metric::parse("throughput"), None);
    }

    #[test]
    fn an_empty_recorder_reports_every_mandatory_metric_as_missing() {
        let recorder = Recorder::new();
        assert_eq!(recorder.missing_mandatory().len(), 18);
        assert!(recorder.report().is_empty());
    }

    #[test]
    fn percentiles_are_ordered_and_exact_at_the_extremes() {
        let mut recorder = Recorder::new();
        for value in 1..=100 {
            recorder.record(Metric::EndToEndMs, f64::from(value));
        }
        let summary = recorder.summary(Metric::EndToEndMs).expect("samples exist");
        assert_eq!(summary.count, 100);
        assert!((summary.min - 1.0).abs() < f64::EPSILON);
        assert!((summary.max - 100.0).abs() < f64::EPSILON);
        assert!((summary.p50 - 50.0).abs() < f64::EPSILON);
        assert!((summary.p95 - 95.0).abs() < f64::EPSILON);
        assert!((summary.p99 - 99.0).abs() < f64::EPSILON);
        assert!(
            summary.p50 <= summary.p95 && summary.p95 <= summary.p99 && summary.p99 <= summary.max
        );
    }

    #[test]
    fn a_single_sample_is_its_own_every_percentile() {
        let mut recorder = Recorder::new();
        recorder.record(Metric::Gaps, 0.0);
        let summary = recorder.summary(Metric::Gaps).expect("one sample");
        assert!((summary.p99 - 0.0).abs() < f64::EPSILON);
        assert_eq!(summary.count, 1);
    }

    #[test]
    fn a_recorded_metric_leaves_the_rest_missing() {
        let mut recorder = Recorder::new();
        recorder.record(Metric::OpenFds, 12.0);
        assert_eq!(recorder.recorded(), vec![Metric::OpenFds]);
        assert!(!recorder.missing_mandatory().contains(&Metric::OpenFds));
        assert_eq!(recorder.missing_mandatory().len(), 17);
    }
}
