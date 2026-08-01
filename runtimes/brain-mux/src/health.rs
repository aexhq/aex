//! Liveness and readiness for `brain-mux`.
//!
//! The paths are `/internal/healthz` and `/internal/readyz`. They are the ALB target, so
//! they are pinned by test rather than by convention: renaming one silently takes the task
//! out of service.
//!
//! The two answer different questions and must not be conflated.
//!
//! - **Liveness** asks whether the process can still do work at all. A live-but-busy task
//!   must stay live, because failing liveness gets it killed and its in-flight
//!   non-replayable effects become interrupted runs.
//! - **Readiness** asks whether it should receive *more* work. It fails during drain, over
//!   the safety cap, and whenever a binding it depends on is unvalidated.
//!
//! Readiness never lies to keep a task in service. A task that reports ready while its
//! catalog signature is unverified would admit work it cannot serve correctly, which is
//! worse than being replaced.

use core::fmt;

/// A health route this process serves.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HealthRoute {
    /// `/internal/healthz`.
    Live,
    /// `/internal/readyz`.
    Ready,
}

/// The liveness path.
pub const LIVE_PATH: &str = "/internal/healthz";

/// The readiness path.
pub const READY_PATH: &str = "/internal/readyz";

impl HealthRoute {
    /// The route for `path`, if this process serves one.
    #[must_use]
    pub fn from_path(path: &str) -> Option<Self> {
        match path {
            LIVE_PATH => Some(Self::Live),
            READY_PATH => Some(Self::Ready),
            _ => None,
        }
    }

    /// The path this route is served at.
    #[must_use]
    pub const fn path(self) -> &'static str {
        match self {
            Self::Live => LIVE_PATH,
            Self::Ready => READY_PATH,
        }
    }
}

/// Whether one thing a probe depends on is in order.
///
/// A named state rather than a `bool`: a probe depends on several independent things, and
/// a call site reading `true, false, true, true` says nothing about which is which.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dependency {
    /// In order.
    Satisfied,
    /// Not in order. The probe fails and names it.
    Unsatisfied,
}

impl Dependency {
    /// Whether the dependency is in order.
    #[must_use]
    pub const fn is_satisfied(self) -> bool {
        matches!(self, Self::Satisfied)
    }
}

/// Everything liveness depends on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LivenessInputs {
    /// Whether every supervised task is still running.
    pub supervisors: Dependency,
    /// Whether the dedicated control runtime answered its last probe.
    pub control_runtime: Dependency,
    /// The reactor's observed p99 scheduling lateness.
    pub reactor_delay_p99_ms: u32,
    /// The bound above which the reactor is considered wedged.
    pub reactor_delay_bound_ms: u32,
}

/// Everything readiness depends on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReadinessInputs {
    /// Whether configuration and every secret binding validated at start-up.
    pub bindings: Dependency,
    /// Whether the pinned catalog loaded and its signature verified.
    pub catalog: Dependency,
    /// Whether the store answered its last probe.
    pub store: Dependency,
    /// Whether the generated schema hashes match this build's expectations.
    pub schema_hashes: Dependency,
    /// Whether drain has started.
    pub draining: bool,
    /// How many activations are running.
    pub active_activations: u32,
    /// The safety cap above which no further work is admitted.
    pub safety_cap: u32,
}

/// Why a probe failed.
///
/// Enumerated rather than a boolean so an operator sees *which* dependency is unsatisfied.
/// A bare "not ready" turns every readiness failure into a log-reading exercise.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Unhealthy {
    /// A supervised task died.
    SupervisorLost,
    /// The control runtime stopped answering.
    ControlRuntimeStalled,
    /// The reactor is above its lateness bound.
    ReactorWedged {
        /// The observed p99.
        observed_ms: u32,
        /// The bound.
        bound_ms: u32,
    },
    /// Configuration or a secret binding did not validate.
    BindingsUnvalidated,
    /// The catalog is absent or its signature did not verify.
    CatalogUnverified,
    /// The store is unreachable.
    StoreUnreachable,
    /// The generated schema hashes do not match this build.
    SchemaMismatch,
    /// The process is draining.
    Draining,
    /// The process is at or above its safety cap.
    AtSafetyCap {
        /// How many activations are running.
        active: u32,
        /// The cap.
        cap: u32,
    },
}

impl fmt::Display for Unhealthy {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::SupervisorLost => formatter.write_str("a supervised task is not running"),
            Self::ControlRuntimeStalled => formatter.write_str("the control runtime is stalled"),
            Self::ReactorWedged {
                observed_ms,
                bound_ms,
            } => write!(
                formatter,
                "reactor delay p99 {observed_ms} ms exceeds the {bound_ms} ms bound"
            ),
            Self::BindingsUnvalidated => {
                formatter.write_str("configuration bindings not validated")
            }
            Self::CatalogUnverified => formatter.write_str("catalog absent or unverified"),
            Self::StoreUnreachable => formatter.write_str("store unreachable"),
            Self::SchemaMismatch => formatter.write_str("schema hashes do not match this build"),
            Self::Draining => formatter.write_str("draining"),
            Self::AtSafetyCap { active, cap } => {
                write!(formatter, "{active} activations at the {cap} safety cap")
            }
        }
    }
}

/// What a probe returns.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Probe {
    /// The HTTP status.
    pub status: u16,
    /// Why the probe failed, when it did.
    pub reasons: Vec<Unhealthy>,
}

impl Probe {
    /// Whether the probe passed.
    #[must_use]
    pub fn is_healthy(&self) -> bool {
        self.status == 200
    }

    /// The response body: `ok`, or one reason per line.
    #[must_use]
    pub fn body(&self) -> String {
        if self.reasons.is_empty() {
            return "ok".to_owned();
        }
        self.reasons
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join("\n")
    }
}

/// Answers `/internal/healthz`.
///
/// Deliberately narrow: it asks only whether the process can still make progress. Load,
/// drain and dependency state belong to readiness, because failing liveness for any of them
/// gets a working task killed along with its in-flight effects.
#[must_use]
pub fn liveness(inputs: &LivenessInputs) -> Probe {
    let mut reasons = Vec::new();
    if !inputs.supervisors.is_satisfied() {
        reasons.push(Unhealthy::SupervisorLost);
    }
    if !inputs.control_runtime.is_satisfied() {
        reasons.push(Unhealthy::ControlRuntimeStalled);
    }
    if inputs.reactor_delay_p99_ms > inputs.reactor_delay_bound_ms {
        reasons.push(Unhealthy::ReactorWedged {
            observed_ms: inputs.reactor_delay_p99_ms,
            bound_ms: inputs.reactor_delay_bound_ms,
        });
    }
    Probe {
        status: if reasons.is_empty() { 200 } else { 503 },
        reasons,
    }
}

/// Answers `/internal/readyz`.
#[must_use]
pub fn readiness(inputs: &ReadinessInputs) -> Probe {
    let mut reasons = Vec::new();
    if !inputs.bindings.is_satisfied() {
        reasons.push(Unhealthy::BindingsUnvalidated);
    }
    if !inputs.catalog.is_satisfied() {
        reasons.push(Unhealthy::CatalogUnverified);
    }
    if !inputs.store.is_satisfied() {
        reasons.push(Unhealthy::StoreUnreachable);
    }
    if !inputs.schema_hashes.is_satisfied() {
        reasons.push(Unhealthy::SchemaMismatch);
    }
    if inputs.draining {
        reasons.push(Unhealthy::Draining);
    }
    if inputs.active_activations >= inputs.safety_cap {
        reasons.push(Unhealthy::AtSafetyCap {
            active: inputs.active_activations,
            cap: inputs.safety_cap,
        });
    }
    Probe {
        status: if reasons.is_empty() { 200 } else { 503 },
        reasons,
    }
}

#[cfg(test)]
mod tests {
    use super::{
        Dependency, HealthRoute, LIVE_PATH, LivenessInputs, READY_PATH, ReadinessInputs, Unhealthy,
        liveness, readiness,
    };

    fn healthy_liveness() -> LivenessInputs {
        LivenessInputs {
            supervisors: Dependency::Satisfied,
            control_runtime: Dependency::Satisfied,
            reactor_delay_p99_ms: 12,
            reactor_delay_bound_ms: 50,
        }
    }

    fn ready() -> ReadinessInputs {
        ReadinessInputs {
            bindings: Dependency::Satisfied,
            catalog: Dependency::Satisfied,
            store: Dependency::Satisfied,
            schema_hashes: Dependency::Satisfied,
            draining: false,
            active_activations: 40,
            safety_cap: 200,
        }
    }

    /// The ALB targets these exact paths. A rename takes the task out of service silently,
    /// so the names are pinned here rather than left to convention.
    #[test]
    fn the_health_paths_are_the_ones_the_load_balancer_targets() {
        assert_eq!(LIVE_PATH, "/internal/healthz");
        assert_eq!(READY_PATH, "/internal/readyz");
        assert_eq!(HealthRoute::from_path(LIVE_PATH), Some(HealthRoute::Live));
        assert_eq!(HealthRoute::from_path(READY_PATH), Some(HealthRoute::Ready));
        assert_eq!(HealthRoute::Live.path(), LIVE_PATH);
        assert_eq!(HealthRoute::Ready.path(), READY_PATH);
    }

    /// The superseded names must not resolve. Serving both would let a stale target group
    /// keep working and hide the misconfiguration until a deploy.
    #[test]
    fn the_superseded_paths_are_not_served() {
        for path in [
            "/livez",
            "/readyz",
            "/healthz",
            "/health",
            "/internal/livez",
        ] {
            assert_eq!(HealthRoute::from_path(path), None, "{path}");
        }
    }

    #[test]
    fn a_healthy_process_passes_both_probes() {
        assert!(liveness(&healthy_liveness()).is_healthy());
        assert!(readiness(&ready()).is_healthy());
        assert_eq!(liveness(&healthy_liveness()).body(), "ok");
    }

    #[test]
    fn each_liveness_failure_names_itself() {
        let mut inputs = healthy_liveness();
        inputs.supervisors = Dependency::Unsatisfied;
        assert_eq!(liveness(&inputs).reasons, vec![Unhealthy::SupervisorLost]);

        let mut inputs = healthy_liveness();
        inputs.control_runtime = Dependency::Unsatisfied;
        assert_eq!(
            liveness(&inputs).reasons,
            vec![Unhealthy::ControlRuntimeStalled]
        );

        let mut inputs = healthy_liveness();
        inputs.reactor_delay_p99_ms = 51;
        assert_eq!(
            liveness(&inputs).reasons,
            vec![Unhealthy::ReactorWedged {
                observed_ms: 51,
                bound_ms: 50
            }]
        );
        assert_eq!(liveness(&inputs).status, 503);
    }

    /// Drain fails readiness while liveness still passes. Reversing this would have the
    /// orchestrator kill a draining task along with the non-replayable effects it is
    /// trying to finish.
    #[test]
    fn drain_fails_readiness_and_leaves_liveness_alone() {
        let mut inputs = ready();
        inputs.draining = true;
        let probe = readiness(&inputs);
        assert!(!probe.is_healthy());
        assert!(probe.reasons.contains(&Unhealthy::Draining));
        assert!(
            liveness(&healthy_liveness()).is_healthy(),
            "a draining process is still alive"
        );
    }

    /// Load is a readiness concern, never a liveness one.
    #[test]
    fn the_safety_cap_fails_readiness_only() {
        let mut inputs = ready();
        inputs.active_activations = inputs.safety_cap;
        let probe = readiness(&inputs);
        assert!(!probe.is_healthy());
        assert!(probe.reasons.contains(&Unhealthy::AtSafetyCap {
            active: 200,
            cap: 200
        }));
        assert!(liveness(&healthy_liveness()).is_healthy());
    }

    /// Readiness never lies to keep a task in service: an unverified catalog fails it even
    /// though the process is otherwise perfectly healthy.
    #[test]
    fn an_unverified_catalog_fails_readiness() {
        let mut inputs = ready();
        inputs.catalog = Dependency::Unsatisfied;
        let probe = readiness(&inputs);
        assert!(!probe.is_healthy());
        assert!(probe.reasons.contains(&Unhealthy::CatalogUnverified));
        assert!(liveness(&healthy_liveness()).is_healthy());
    }

    #[test]
    fn every_readiness_dependency_is_reported_not_just_the_first() {
        let inputs = ReadinessInputs {
            bindings: Dependency::Unsatisfied,
            catalog: Dependency::Unsatisfied,
            store: Dependency::Unsatisfied,
            schema_hashes: Dependency::Unsatisfied,
            draining: true,
            active_activations: 300,
            safety_cap: 200,
        };
        let probe = readiness(&inputs);
        assert_eq!(probe.reasons.len(), 6, "{:?}", probe.reasons);
        assert_eq!(probe.body().lines().count(), 6);
    }
}
