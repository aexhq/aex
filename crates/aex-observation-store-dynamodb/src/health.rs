//! The health surface every observation deployable mounts.
//!
//! `/internal/healthz` and `/internal/readyz` on every Rust deployable, per the
//! cross-stream requirement. `healthz` answers as soon as the process is alive;
//! `readyz` answers only once every declared dependency has actually been
//! probed, because a readiness endpoint that answers before its dependencies
//! are proven is worse than none.

/// The liveness path.
pub const HEALTHZ: &str = "/internal/healthz";

/// The readiness path.
pub const READYZ: &str = "/internal/readyz";

/// One dependency a deployable proves before it reports ready.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum Probe {
    /// `DescribeTable` on the observation authority.
    ObservationTable,
    /// `HeadBucket` on the observation bucket.
    ObservationBucket,
    /// The cursor signing key ring resolves.
    CursorKeyRing,
    /// The regional ingress gate item is readable.
    IngressGate,
    /// `session-authority` is readable for the `events` signal.
    SessionAuthority,
    /// The export cluster and task definition are describable.
    ExportCluster,
}

impl Probe {
    /// The name reported in a readiness failure.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ObservationTable => "observation_table",
            Self::ObservationBucket => "observation_bucket",
            Self::CursorKeyRing => "cursor_key_ring",
            Self::IngressGate => "ingress_gate",
            Self::SessionAuthority => "session_authority",
            Self::ExportCluster => "export_cluster",
        }
    }
}

/// What a deployable reports.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Readiness {
    /// Every declared probe passed.
    Ready,
    /// A probe has not passed. The endpoint answers `503` and names it.
    NotReady {
        /// The first outstanding probe.
        outstanding: Probe,
    },
}

/// Evaluates readiness over a declared probe set.
///
/// A probe absent from `passed` is outstanding, never assumed.
#[must_use]
pub fn readiness(required: &[Probe], passed: &[Probe]) -> Readiness {
    for probe in required {
        if !passed.contains(probe) {
            return Readiness::NotReady {
                outstanding: *probe,
            };
        }
    }
    Readiness::Ready
}

#[cfg(test)]
mod tests {
    use super::{HEALTHZ, Probe, READYZ, Readiness, readiness};

    #[test]
    fn the_health_paths_are_the_cross_stream_declared_ones() {
        assert_eq!(HEALTHZ, "/internal/healthz");
        assert_eq!(READYZ, "/internal/readyz");
        assert_ne!(READYZ, "/readyz", "Brain's naming is superseded");
    }

    #[test]
    fn an_unproven_probe_keeps_the_deployable_not_ready() {
        let required = [Probe::ObservationTable, Probe::ObservationBucket];
        assert_eq!(
            readiness(&required, &[]),
            Readiness::NotReady {
                outstanding: Probe::ObservationTable
            }
        );
        assert_eq!(
            readiness(&required, &[Probe::ObservationTable]),
            Readiness::NotReady {
                outstanding: Probe::ObservationBucket
            }
        );
        assert_eq!(readiness(&required, &required), Readiness::Ready);
    }

    #[test]
    fn a_probe_names_itself_in_a_failure() {
        assert_eq!(Probe::CursorKeyRing.as_str(), "cursor_key_ring");
        assert_eq!(Probe::IngressGate.as_str(), "ingress_gate");
    }
}
