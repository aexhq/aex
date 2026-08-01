//! Gate outcomes and the one failure message a capacity miss produces.
//!
//! The predicate belongs to the owning stream - only it knows what
//! `rss_peak <= 0.85 * task_memory_limit` means for its artifact. What belongs
//! here is the contract around the predicate: a gate is named, it is a budget or
//! a diagnostic, a miss is reported with the observed value **and** the source
//! of the threshold, and a blocking miss fails the campaign rather than being
//! filed as a note.

use serde::{Deserialize, Serialize};

use crate::workload::{Gate, GateKind};

/// Whether the gate's assertion held.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GateVerdict {
    /// The assertion held.
    Met,
    /// The assertion did not hold.
    Unmet,
}

/// One gate's result.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GateOutcome {
    /// The gate id.
    pub id: String,
    /// Budget or diagnostic.
    pub kind: GateKind,
    /// Whether a miss blocks.
    pub blocking: bool,
    /// Whether the assertion held.
    pub verdict: GateVerdict,
    /// What was observed, in the owner's own units. This is the text the
    /// failure message carries, so it must be specific enough to act on.
    pub observed: String,
    /// Where the threshold comes from.
    pub source: String,
}

impl GateOutcome {
    /// Records `gate`'s result.
    #[must_use]
    pub fn record(gate: &Gate, verdict: GateVerdict, observed: impl Into<String>) -> Self {
        Self {
            id: gate.id.clone(),
            kind: gate.kind,
            blocking: gate.blocking,
            verdict,
            observed: observed.into(),
            source: gate.source.clone(),
        }
    }

    /// Whether this outcome fails the campaign.
    #[must_use]
    pub fn is_blocking_failure(&self) -> bool {
        self.blocking && self.verdict == GateVerdict::Unmet && self.kind == GateKind::Budget
    }

    /// The failure line, in the one declared form.
    #[must_use]
    pub fn failure_message(&self) -> String {
        format!(
            "[perf-gate-unmet] gate {}: {} (source: {})",
            self.id, self.observed, self.source
        )
    }
}

/// Every gate outcome of one campaign.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct GateReport {
    /// The outcomes, in declaration order.
    pub outcomes: Vec<GateOutcome>,
}

impl GateReport {
    /// An empty report.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds an outcome.
    pub fn push(&mut self, outcome: GateOutcome) {
        self.outcomes.push(outcome);
    }

    /// Every gate in `declared` that produced no outcome.
    ///
    /// A declared gate with no outcome is not a pass: the campaign either never
    /// measured it or lost the measurement, and both are failures.
    #[must_use]
    pub fn unreported<'a>(&self, declared: &'a [Gate]) -> Vec<&'a Gate> {
        declared
            .iter()
            .filter(|gate| !self.outcomes.iter().any(|outcome| outcome.id == gate.id))
            .collect()
    }

    /// The failure lines, one per blocking miss.
    #[must_use]
    pub fn failures(&self) -> Vec<String> {
        self.outcomes
            .iter()
            .filter(|outcome| outcome.is_blocking_failure())
            .map(GateOutcome::failure_message)
            .collect()
    }

    /// Whether the campaign passed: every declared gate reported, and no
    /// blocking budget missed.
    #[must_use]
    pub fn passed(&self, declared: &[Gate]) -> bool {
        self.unreported(declared).is_empty() && self.failures().is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::{GateOutcome, GateReport, GateVerdict};
    use crate::workload::{Gate, GateKind};

    fn gate(id: &str, kind: GateKind, blocking: bool) -> Gate {
        Gate {
            id: id.to_owned(),
            kind,
            blocking,
            assert: "rss_peak <= 0.85 * task_memory_limit".to_owned(),
            source: "PERF-02".to_owned(),
        }
    }

    #[test]
    fn a_missed_blocking_budget_produces_the_declared_message() {
        let gate = gate("LOAD-200-SAFE", GateKind::Budget, true);
        let outcome = GateOutcome::record(
            &gate,
            GateVerdict::Unmet,
            "rss_peak 1.94 GiB exceeds 0.85 x 2 GiB = 1.70 GiB",
        );
        assert!(outcome.is_blocking_failure());
        assert_eq!(
            outcome.failure_message(),
            "[perf-gate-unmet] gate LOAD-200-SAFE: rss_peak 1.94 GiB exceeds 0.85 x 2 GiB = 1.70 GiB (source: PERF-02)"
        );
    }

    #[test]
    fn a_missed_diagnostic_is_recorded_and_never_blocks() {
        let gate = gate("PERF-BASELINE-CODECS", GateKind::Diagnostic, false);
        let outcome = GateOutcome::record(&gate, GateVerdict::Unmet, "1 MiB JCS 14.2 ms");
        assert!(!outcome.is_blocking_failure());
        let mut report = GateReport::new();
        report.push(outcome);
        assert!(report.failures().is_empty());
        assert!(report.passed(std::slice::from_ref(&gate)));
    }

    #[test]
    fn a_declared_gate_with_no_outcome_is_not_a_pass() {
        let declared = vec![
            gate("LOAD-100-COMPLETE", GateKind::Budget, true),
            gate("LOAD-100-BUDGETS", GateKind::Budget, true),
        ];
        let mut report = GateReport::new();
        report.push(GateOutcome::record(
            &declared[0],
            GateVerdict::Met,
            "500/500",
        ));
        assert_eq!(report.unreported(&declared).len(), 1);
        assert!(!report.passed(&declared));
        assert!(
            report.failures().is_empty(),
            "an unreported gate is not a miss, it is a gap"
        );
    }

    #[test]
    fn a_fully_reported_campaign_with_no_miss_passes() {
        let declared = vec![gate("LOAD-100-COMPLETE", GateKind::Budget, true)];
        let mut report = GateReport::new();
        report.push(GateOutcome::record(
            &declared[0],
            GateVerdict::Met,
            "500/500",
        ));
        assert!(report.passed(&declared));
    }
}
