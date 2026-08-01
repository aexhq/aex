//! The workload descriptor and its validation.
//!
//! `PERF-01`: a capacity claim is only meaningful next to the shape that
//! produced it. The descriptor pins the tier, the arrival process, the mix, the
//! duration, the spend budget, the TTL, at least one gate and the full
//! mandatory metric set. `kind = "slo"` is rejected here, at parse time, so a
//! customer percentile promise cannot become a prelaunch release gate by
//! accident.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::arrival::{Arrival, Mix};
use crate::recorder::Metric;

/// Which phase the workspace is in.
///
/// Live evidence cannot be earned before deployment (`OD-07`), so a descriptor
/// with an unset spend budget is a recorded gap during the source rewrite and a
/// blocking failure once a release candidate exists.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    /// Source is being written; nothing is deployed.
    SourceRewrite,
    /// A release candidate exists and live evidence is earnable.
    Candidate,
}

/// What a gate is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GateKind {
    /// An engineering threshold on a pinned shape, with a named source. Blocks.
    Budget,
    /// A measurement that is recorded and compared, never blocking.
    Diagnostic,
}

/// One assertion the campaign must satisfy.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Gate {
    /// The gate id, referenced by `release/policy/workload-registry.toml`.
    pub id: String,
    /// Budget or diagnostic.
    pub kind: GateKind,
    /// Whether a miss blocks.
    pub blocking: bool,
    /// The assertion, in the owning stream's own terms.
    pub assert: String,
    /// Where the threshold comes from. A budget with no source is a guess.
    pub source: String,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
struct RawArrival {
    mode: Arrival,
    #[serde(default)]
    concurrency: Option<u32>,
    #[serde(default)]
    rate_per_s: Option<f64>,
    mix: Mix,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
struct RawReport {
    metrics: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
struct RawGate {
    id: String,
    kind: String,
    blocking: bool,
    assert: String,
    source: String,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
struct RawDescriptor {
    id: String,
    owner: String,
    target: String,
    tier: String,
    duration: String,
    #[serde(default)]
    budget_micro_usd: Option<u64>,
    ttl: String,
    arrival: RawArrival,
    #[serde(default)]
    gate: Vec<RawGate>,
    report: RawReport,
}

/// A validated workload descriptor.
#[derive(Debug, Clone, PartialEq)]
pub struct WorkloadDescriptor {
    /// The workload id; the file is `tests/load/workloads/<owner>/<id>.toml`.
    pub id: String,
    /// The owning stream.
    pub owner: String,
    /// The live package whose `load` target executes it.
    pub target: String,
    /// The tier profile it references.
    pub tier: String,
    /// How long the campaign runs, in milliseconds.
    pub duration_ms: u64,
    /// The soft spend ceiling, when one is declared.
    pub budget_micro_usd: Option<u64>,
    /// How long the campaign's resources may live, in milliseconds.
    pub ttl_ms: u64,
    /// The arrival mode.
    pub arrival: Arrival,
    /// Agents in flight, for closed arrival.
    pub concurrency: Option<u32>,
    /// Offers per second, for open arrival.
    pub rate_per_s: Option<f64>,
    /// The request-shape mix.
    pub mix: Mix,
    /// The gates the campaign must satisfy.
    pub gates: Vec<Gate>,
    /// The metrics the campaign reports.
    pub metrics: Vec<Metric>,
}

/// Why a descriptor is not usable.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum WorkloadError {
    /// The document is not a workload descriptor.
    #[error("workload `{path}` does not parse: {detail}")]
    Parse {
        /// The descriptor path.
        path: String,
        /// The parse failure.
        detail: String,
    },
    /// A gate declared `kind = "slo"`.
    #[error(
        "[perf-workload-undeclared] workload `{workload}` gate `{gate}` declares kind `slo`; a prelaunch gate is a budget or a diagnostic, never a customer SLO"
    )]
    SloGate {
        /// The declaring workload.
        workload: String,
        /// The offending gate.
        gate: String,
    },
    /// A gate declared an unknown kind.
    #[error(
        "workload `{workload}` gate `{gate}` declares kind `{kind}`; permitted kinds: budget, diagnostic"
    )]
    UnknownGateKind {
        /// The declaring workload.
        workload: String,
        /// The offending gate.
        gate: String,
        /// The value that was declared.
        kind: String,
    },
    /// The descriptor declared no gate at all.
    #[error(
        "workload `{workload}` declares no gate; a campaign with no assertion measures nothing"
    )]
    NoGate {
        /// The declaring workload.
        workload: String,
    },
    /// A blocking budget gate carries no threshold source.
    #[error(
        "workload `{workload}` gate `{gate}` is a blocking budget with no source; a threshold with no source is a guess"
    )]
    UnsourcedBudget {
        /// The declaring workload.
        workload: String,
        /// The offending gate.
        gate: String,
    },
    /// The report omits mandatory metrics.
    #[error("workload `{workload}` omits mandatory report metric(s): {missing}")]
    MissingMetrics {
        /// The declaring workload.
        workload: String,
        /// The metrics that were omitted, comma separated.
        missing: String,
    },
    /// The report names a metric that is not in the closed set.
    #[error("workload `{workload}` reports unknown metric `{metric}`; the metric set is closed")]
    UnknownMetric {
        /// The declaring workload.
        workload: String,
        /// The unknown name.
        metric: String,
    },
    /// The mix weights do not sum to one.
    #[error("workload `{workload}` declares a mix whose weights do not sum to 1.0")]
    MixNotNormalized {
        /// The declaring workload.
        workload: String,
    },
    /// The arrival mode is missing the parameter it needs.
    #[error("workload `{workload}` declares {mode} arrival without `{field}`")]
    ArrivalIncomplete {
        /// The declaring workload.
        workload: String,
        /// The declared mode.
        mode: &'static str,
        /// The field it needs.
        field: &'static str,
    },
    /// A duration string could not be read.
    #[error(
        "workload `{workload}` declares `{field} = \"{value}\"`, which is not a duration such as `30m`, `4h` or `90s`"
    )]
    BadDuration {
        /// The declaring workload.
        workload: String,
        /// Which field.
        field: &'static str,
        /// The value that failed.
        value: String,
    },
    /// The tier profile does not exist.
    #[error(
        "workload `{workload}` references tier `{tier}`, which has no profile in tests/load/profiles"
    )]
    UnknownTier {
        /// The declaring workload.
        workload: String,
        /// The tier that was referenced.
        tier: String,
    },
    /// A candidate-phase descriptor left its spend budget unset.
    #[error(
        "workload `{workload}` declares no budget_micro_usd; a live campaign with no spend ceiling cannot be stopped"
    )]
    BudgetUnset {
        /// The declaring workload.
        workload: String,
    },
}

impl WorkloadDescriptor {
    /// Parses and validates a descriptor.
    ///
    /// # Errors
    ///
    /// Returns [`WorkloadError`] for every rule in this module's documentation.
    pub fn parse(path: &str, text: &str) -> Result<Self, WorkloadError> {
        let raw: RawDescriptor = toml::from_str(text).map_err(|error| WorkloadError::Parse {
            path: path.to_owned(),
            detail: error.to_string(),
        })?;
        let workload = raw.id.clone();
        let gates = parse_gates(&workload, raw.gate)?;
        let metrics = parse_metrics(&workload, &raw.report.metrics)?;
        check_arrival(&workload, &raw.arrival)?;

        let duration_ms =
            parse_duration(&raw.duration).ok_or_else(|| WorkloadError::BadDuration {
                workload: workload.clone(),
                field: "duration",
                value: raw.duration.clone(),
            })?;
        let ttl_ms = parse_duration(&raw.ttl).ok_or_else(|| WorkloadError::BadDuration {
            workload: workload.clone(),
            field: "ttl",
            value: raw.ttl.clone(),
        })?;

        Ok(Self {
            id: raw.id,
            owner: raw.owner,
            target: raw.target,
            tier: raw.tier,
            duration_ms,
            budget_micro_usd: raw.budget_micro_usd,
            ttl_ms,
            arrival: raw.arrival.mode,
            concurrency: raw.arrival.concurrency,
            rate_per_s: raw.arrival.rate_per_s,
            mix: raw.arrival.mix,
            gates,
            metrics,
        })
    }

    /// The checks that depend on the rest of the tree and on the phase.
    ///
    /// # Errors
    ///
    /// Returns [`WorkloadError::UnknownTier`] when the referenced tier profile
    /// is absent, and [`WorkloadError::BudgetUnset`] when a candidate-phase
    /// descriptor has no spend ceiling.
    pub fn verify(&self, known_tiers: &[String], phase: Phase) -> Result<(), WorkloadError> {
        if !known_tiers.iter().any(|tier| tier == &self.tier) {
            return Err(WorkloadError::UnknownTier {
                workload: self.id.clone(),
                tier: self.tier.clone(),
            });
        }
        if phase == Phase::Candidate && self.budget_micro_usd.is_none() {
            return Err(WorkloadError::BudgetUnset {
                workload: self.id.clone(),
            });
        }
        Ok(())
    }

    /// The blocking gates, which are the ones a campaign can fail on.
    #[must_use]
    pub fn blocking_gates(&self) -> Vec<&Gate> {
        self.gates.iter().filter(|gate| gate.blocking).collect()
    }
}

fn parse_gates(workload: &str, raw: Vec<RawGate>) -> Result<Vec<Gate>, WorkloadError> {
    let mut gates = Vec::new();
    for gate in raw {
        let kind = match gate.kind.as_str() {
            "budget" => GateKind::Budget,
            "diagnostic" => GateKind::Diagnostic,
            "slo" => {
                return Err(WorkloadError::SloGate {
                    workload: workload.to_owned(),
                    gate: gate.id,
                });
            }
            other => {
                return Err(WorkloadError::UnknownGateKind {
                    workload: workload.to_owned(),
                    gate: gate.id,
                    kind: other.to_owned(),
                });
            }
        };
        if kind == GateKind::Budget && gate.blocking && gate.source.trim().is_empty() {
            return Err(WorkloadError::UnsourcedBudget {
                workload: workload.to_owned(),
                gate: gate.id,
            });
        }
        gates.push(Gate {
            id: gate.id,
            kind,
            blocking: gate.blocking,
            assert: gate.assert,
            source: gate.source,
        });
    }
    if gates.is_empty() {
        return Err(WorkloadError::NoGate {
            workload: workload.to_owned(),
        });
    }
    Ok(gates)
}

fn parse_metrics(workload: &str, declared: &[String]) -> Result<Vec<Metric>, WorkloadError> {
    let mut metrics = Vec::new();
    for name in declared {
        let metric = Metric::parse(name).ok_or_else(|| WorkloadError::UnknownMetric {
            workload: workload.to_owned(),
            metric: name.clone(),
        })?;
        metrics.push(metric);
    }
    let missing: Vec<&str> = Metric::MANDATORY
        .into_iter()
        .filter(|metric| !metrics.contains(metric))
        .map(Metric::as_str)
        .collect();
    if missing.is_empty() {
        Ok(metrics)
    } else {
        Err(WorkloadError::MissingMetrics {
            workload: workload.to_owned(),
            missing: missing.join(", "),
        })
    }
}

fn check_arrival(workload: &str, arrival: &RawArrival) -> Result<(), WorkloadError> {
    if !arrival.mix.is_normalized() {
        return Err(WorkloadError::MixNotNormalized {
            workload: workload.to_owned(),
        });
    }
    match arrival.mode {
        Arrival::Closed if arrival.concurrency.is_none() => Err(WorkloadError::ArrivalIncomplete {
            workload: workload.to_owned(),
            mode: "closed",
            field: "concurrency",
        }),
        Arrival::Open if arrival.rate_per_s.is_none() => Err(WorkloadError::ArrivalIncomplete {
            workload: workload.to_owned(),
            mode: "open",
            field: "rate_per_s",
        }),
        _ => Ok(()),
    }
}

/// Reads `90s`, `30m`, `4h` or `2d` into milliseconds.
#[must_use]
pub fn parse_duration(text: &str) -> Option<u64> {
    let trimmed = text.trim();
    let (digits, unit) = trimmed.split_at(trimmed.len().checked_sub(1)?);
    let value: u64 = digits.parse().ok()?;
    let multiplier = match unit {
        "s" => 1_000,
        "m" => 60 * 1_000,
        "h" => 60 * 60 * 1_000,
        "d" => 24 * 60 * 60 * 1_000,
        _ => return None,
    };
    value.checked_mul(multiplier)
}

/// The tier profiles found under `tests/load/profiles`.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct TierProfile {
    /// The tier id, matching the file stem.
    pub id: String,
    /// What the tier describes.
    pub description: String,
    /// The Area 10 or `PERF-*` row the vector comes from.
    pub source: String,
    /// The named capacity vector.
    pub vector: BTreeMap<String, u64>,
}

#[cfg(test)]
mod tests {
    use super::{GateKind, Phase, WorkloadDescriptor, WorkloadError, parse_duration};

    const METRICS: &str = r#"metrics = ["completed_per_s", "queue_delay_ms", "time_to_first_token_ms",
           "end_to_end_ms", "provider_wait_ms", "local_cpu_ms", "rss_peak_bytes",
           "open_fds", "tokio_tasks", "sockets", "journal_bytes", "checkpoint_bytes",
           "affinity_hit_rate", "scale_event_s", "recovery_s", "duplicate_effects",
           "gaps", "cost_micro_usd_per_completed_turn"]"#;

    fn descriptor(gate_kind: &str, extra: &str) -> String {
        format!(
            r#"
id = "brain-100-mixed"
owner = "brain-core"
target = "aex-live-brain-mux"
tier = "t2"
duration = "30m"
budget_micro_usd = 40000000
ttl = "4h"
{extra}

[arrival]
mode = "closed"
concurrency = 100
mix = {{ short_turn = 0.7, long_context_1mib = 0.05, tool_heavy = 0.15, subagent_fanout = 0.10 }}

[[gate]]
id = "LOAD-100-COMPLETE"
kind = "{gate_kind}"
blocking = true
assert = "completed == offered"
source = "PERF-02; Area 10 T2"

[report]
{METRICS}
"#
        )
    }

    #[test]
    fn a_complete_descriptor_parses_and_verifies() {
        let parsed = WorkloadDescriptor::parse("brain-100-mixed.toml", &descriptor("budget", ""))
            .expect("the descriptor is complete");
        assert_eq!(parsed.duration_ms, 30 * 60 * 1_000);
        assert_eq!(parsed.ttl_ms, 4 * 60 * 60 * 1_000);
        assert_eq!(parsed.concurrency, Some(100));
        assert_eq!(parsed.metrics.len(), 18);
        assert_eq!(parsed.gates[0].kind, GateKind::Budget);
        assert_eq!(parsed.blocking_gates().len(), 1);
        parsed
            .verify(&["t2".to_owned()], Phase::Candidate)
            .expect("the tier exists and the budget is set");
    }

    #[test]
    fn an_slo_gate_is_rejected_with_the_declared_message() {
        let error = WorkloadDescriptor::parse("x.toml", &descriptor("slo", ""))
            .expect_err("an SLO gate is not a prelaunch gate");
        assert_eq!(
            error.to_string(),
            "[perf-workload-undeclared] workload `brain-100-mixed` gate `LOAD-100-COMPLETE` declares kind `slo`; a prelaunch gate is a budget or a diagnostic, never a customer SLO"
        );
    }

    #[test]
    fn an_omitted_mandatory_metric_names_what_is_missing() {
        let text = descriptor("budget", "").replace("\"gaps\", ", "");
        let error = WorkloadDescriptor::parse("x.toml", &text).expect_err("a metric is missing");
        assert_eq!(
            error,
            WorkloadError::MissingMetrics {
                workload: "brain-100-mixed".to_owned(),
                missing: "gaps".to_owned()
            }
        );
    }

    #[test]
    fn an_unknown_metric_is_rejected_because_the_set_is_closed() {
        let text = descriptor("budget", "").replace("\"gaps\"", "\"throughput\"");
        let error = WorkloadDescriptor::parse("x.toml", &text).expect_err("the set is closed");
        assert!(
            matches!(error, WorkloadError::UnknownMetric { .. }),
            "{error}"
        );
    }

    #[test]
    fn a_mix_that_does_not_sum_to_one_is_rejected() {
        let text = descriptor("budget", "").replace("short_turn = 0.7", "short_turn = 0.9");
        let error =
            WorkloadDescriptor::parse("x.toml", &text).expect_err("the mix is unnormalized");
        assert!(
            matches!(error, WorkloadError::MixNotNormalized { .. }),
            "{error}"
        );
    }

    #[test]
    fn closed_arrival_without_concurrency_is_rejected() {
        let text = descriptor("budget", "").replace("concurrency = 100\n", "");
        let error =
            WorkloadDescriptor::parse("x.toml", &text).expect_err("concurrency is required");
        assert_eq!(
            error.to_string(),
            "workload `brain-100-mixed` declares closed arrival without `concurrency`"
        );
    }

    #[test]
    fn a_blocking_budget_without_a_source_is_rejected() {
        let text =
            descriptor("budget", "").replace("source = \"PERF-02; Area 10 T2\"", "source = \"\"");
        let error =
            WorkloadDescriptor::parse("x.toml", &text).expect_err("a budget needs a source");
        assert!(
            matches!(error, WorkloadError::UnsourcedBudget { .. }),
            "{error}"
        );
    }

    #[test]
    fn an_unset_budget_passes_the_source_rewrite_and_blocks_a_candidate() {
        let text = descriptor("budget", "").replace("budget_micro_usd = 40000000\n", "");
        let parsed = WorkloadDescriptor::parse("x.toml", &text).expect("the budget is optional");
        parsed
            .verify(&["t2".to_owned()], Phase::SourceRewrite)
            .expect("an unset budget is a recorded gap before deployment");
        let error = parsed
            .verify(&["t2".to_owned()], Phase::Candidate)
            .expect_err("a candidate campaign needs a ceiling");
        assert!(
            matches!(error, WorkloadError::BudgetUnset { .. }),
            "{error}"
        );
    }

    #[test]
    fn an_unknown_tier_is_rejected() {
        let parsed =
            WorkloadDescriptor::parse("x.toml", &descriptor("budget", "")).expect("parses");
        let error = parsed
            .verify(&["t1".to_owned()], Phase::SourceRewrite)
            .expect_err("t2 has no profile");
        assert!(
            matches!(error, WorkloadError::UnknownTier { .. }),
            "{error}"
        );
    }

    #[test]
    fn durations_are_read_in_seconds_minutes_hours_and_days() {
        assert_eq!(parse_duration("90s"), Some(90_000));
        assert_eq!(parse_duration("30m"), Some(1_800_000));
        assert_eq!(parse_duration("4h"), Some(14_400_000));
        assert_eq!(parse_duration("2d"), Some(172_800_000));
        assert_eq!(parse_duration("30"), None);
        assert_eq!(parse_duration("thirty minutes"), None);
        assert_eq!(parse_duration(""), None);
    }
}
