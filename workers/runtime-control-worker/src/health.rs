//! The internal health surface, and the work domains this worker owns.
//!
//! The paths are `/internal/healthz` and `/internal/readyz` on every Rust
//! deployable, which is also what the load balancer targets. One naming, checked
//! here, rather than a per-service convention nobody can remember.

/// Liveness. Answers as soon as the process is up.
pub const HEALTHZ_PATH: &str = "/internal/healthz";

/// Readiness. Answers only once every composed dependency is bound.
pub const READYZ_PATH: &str = "/internal/readyz";

/// What a readiness probe found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Readiness {
    /// Every dependency is bound.
    Ready,
    /// Something is not bound yet. Named, because "not ready" with no reason is an
    /// alarm nobody can act on.
    NotReady {
        /// Which dependencies are missing.
        missing: Vec<&'static str>,
    },
}

impl Readiness {
    /// The HTTP status this readiness answers with.
    #[must_use]
    pub const fn http_status(&self) -> u16 {
        match self {
            Self::Ready => 200,
            Self::NotReady { .. } => 503,
        }
    }
}

/// One dependency this worker must have bound before it is ready.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Dependency {
    /// The runtime-activity store.
    RuntimeActivity,
    /// The `MicroVM` control plane.
    MicrovmControl,
    /// The compute-authority usage sink.
    ComputeSink,
    /// The storage-authority usage sink.
    StorageSink,
}

impl Dependency {
    /// Every dependency, in the order readiness reports them.
    pub const ALL: [Self; 4] = [
        Self::RuntimeActivity,
        Self::MicrovmControl,
        Self::ComputeSink,
        Self::StorageSink,
    ];

    /// The name a probe reports.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::RuntimeActivity => "runtime-activity",
            Self::MicrovmControl => "microvm-control",
            Self::ComputeSink => "usage-compute-sink",
            Self::StorageSink => "usage-storage-sink",
        }
    }
}

/// The dependencies this worker has actually bound.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Bindings {
    /// What is bound.
    bound: Vec<Dependency>,
}

impl Bindings {
    /// Records a bound dependency.
    #[must_use]
    pub fn with(mut self, dependency: Dependency) -> Self {
        if !self.bound.contains(&dependency) {
            self.bound.push(dependency);
        }
        self
    }

    /// Evaluates readiness.
    #[must_use]
    pub fn readiness(&self) -> Readiness {
        let missing: Vec<&'static str> = Dependency::ALL
            .into_iter()
            .filter(|dependency| !self.bound.contains(dependency))
            .map(Dependency::as_str)
            .collect();
        if missing.is_empty() {
            Readiness::Ready
        } else {
            Readiness::NotReady { missing }
        }
    }
}

/// The work domains this worker is the sole handler for.
///
/// It is the **only** ordinary `MicroVM` control role. Nothing else suspends,
/// resumes or terminates a generation, which is what makes the single suspend
/// fence sufficient.
pub const WORK_DOMAINS: [&str; 2] = ["runtime.workspace_discard", "runtime.live_workspace_wake"];

/// A usage category this worker may write to.
///
/// Transfer is deliberately absent: Hands Internet egress is not charged at launch
/// (OD-26) and snapshot I/O is zero-dollar observability (OD-25), so the worker has
/// no reason to hold a transfer-authority binding at all. Not holding one is
/// stronger than holding one and not using it.
pub const USAGE_CATEGORIES: [&str; 2] = ["compute", "storage"];

#[cfg(test)]
mod tests {
    use super::{
        Bindings, Dependency, HEALTHZ_PATH, READYZ_PATH, Readiness, USAGE_CATEGORIES, WORK_DOMAINS,
    };

    #[test]
    fn the_health_paths_are_the_workspace_wide_ones() {
        assert_eq!(HEALTHZ_PATH, "/internal/healthz");
        assert_eq!(READYZ_PATH, "/internal/readyz");
        // The superseded Brain spelling must not come back.
        assert_ne!(HEALTHZ_PATH, "/livez");
        assert_ne!(READYZ_PATH, "/readyz");
    }

    #[test]
    fn readiness_names_every_unbound_dependency() {
        let none = Bindings::default();
        assert_eq!(
            none.readiness(),
            Readiness::NotReady {
                missing: vec![
                    "runtime-activity",
                    "microvm-control",
                    "usage-compute-sink",
                    "usage-storage-sink"
                ]
            }
        );
        assert_eq!(none.readiness().http_status(), 503);

        let partial = Bindings::default()
            .with(Dependency::RuntimeActivity)
            .with(Dependency::MicrovmControl)
            .with(Dependency::StorageSink);
        assert_eq!(
            partial.readiness(),
            Readiness::NotReady {
                missing: vec!["usage-compute-sink"]
            },
            "an unnamed not-ready is an alarm nobody can act on"
        );

        let all = Dependency::ALL
            .into_iter()
            .fold(Bindings::default(), Bindings::with);
        assert_eq!(all.readiness(), Readiness::Ready);
        assert_eq!(all.readiness().http_status(), 200);
    }

    #[test]
    fn the_worker_owns_exactly_two_work_domains() {
        assert_eq!(
            WORK_DOMAINS,
            ["runtime.workspace_discard", "runtime.live_workspace_wake"]
        );
    }

    #[test]
    fn the_worker_binds_no_transfer_authority() {
        assert_eq!(USAGE_CATEGORIES, ["compute", "storage"]);
        assert!(
            !USAGE_CATEGORIES.contains(&"transfer"),
            "not holding the binding is stronger than holding it and not using it"
        );
    }
}
