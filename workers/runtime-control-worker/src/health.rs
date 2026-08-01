//! The internal health surface, and the work domains this worker owns.
//!
//! The paths are `/internal/healthz` and `/internal/readyz` on every Rust
//! deployable, which is also what the load balancer targets. One naming, checked
//! here, rather than a per-service convention nobody can remember.

use std::sync::Arc;

use axum::extract::State;
use axum::http::StatusCode;
use axum::routing::get;
use axum::{Json, Router};
use serde::Serialize;

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
    /// The authoritative open-Hands-effect count in `session-authority`. Separate
    /// from the activity store because the two read different tables, and because
    /// the suspend transition is unsound without the second one.
    SessionEffects,
    /// The `MicroVM` control plane.
    MicrovmControl,
    /// The compute-authority usage sink.
    ComputeSink,
    /// The storage-authority usage sink.
    StorageSink,
}

impl Dependency {
    /// Every dependency, in the order readiness reports them.
    pub const ALL: [Self; 5] = [
        Self::RuntimeActivity,
        Self::SessionEffects,
        Self::MicrovmControl,
        Self::ComputeSink,
        Self::StorageSink,
    ];

    /// The name a probe reports.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::RuntimeActivity => "runtime-activity",
            Self::SessionEffects => "session-open-effects",
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

/// The body both probes answer with.
///
/// Readiness also states what this worker handles and what it may write to. An
/// operator reading a 503 needs to know which role is degraded, not only that
/// something is.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Probe {
    /// `ok`, `ready` or `not_ready`.
    pub status: &'static str,
    /// Which dependencies are not bound. Empty when ready.
    pub missing: Vec<&'static str>,
    /// The work domains this worker is the sole handler for.
    pub work_domains: &'static [&'static str],
    /// The usage authorities it may write to. Never three.
    pub usage_categories: &'static [&'static str],
}

/// The internal health surface.
///
/// The paths are the workspace-wide ones, which is also what the load balancer
/// targets. Readiness names what is missing, because "not ready" with no reason is
/// an alarm nobody can act on.
pub fn router(bindings: Bindings) -> Router {
    Router::new()
        .route(HEALTHZ_PATH, get(healthz))
        .route(READYZ_PATH, get(readyz))
        .with_state(Arc::new(bindings))
}

/// Liveness: the process is up.
async fn healthz() -> Json<Probe> {
    Json(Probe {
        status: "ok",
        missing: Vec::new(),
        work_domains: &WORK_DOMAINS,
        usage_categories: &USAGE_CATEGORIES,
    })
}

/// Readiness: every composed dependency is bound.
async fn readyz(State(bindings): State<Arc<Bindings>>) -> (StatusCode, Json<Probe>) {
    let readiness = bindings.readiness();
    let status =
        StatusCode::from_u16(readiness.http_status()).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
    let body = match readiness {
        Readiness::Ready => Probe {
            status: "ready",
            missing: Vec::new(),
            work_domains: &WORK_DOMAINS,
            usage_categories: &USAGE_CATEGORIES,
        },
        Readiness::NotReady { missing } => Probe {
            status: "not_ready",
            missing,
            work_domains: &WORK_DOMAINS,
            usage_categories: &USAGE_CATEGORIES,
        },
    };
    (status, Json(body))
}

#[cfg(test)]
mod tests {
    use super::{
        Bindings, Dependency, HEALTHZ_PATH, READYZ_PATH, Readiness, USAGE_CATEGORIES, WORK_DOMAINS,
        router,
    };
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use tower::ServiceExt as _;

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
                    "session-open-effects",
                    "microvm-control",
                    "usage-compute-sink",
                    "usage-storage-sink"
                ]
            }
        );
        assert_eq!(none.readiness().http_status(), 503);

        let partial = Bindings::default()
            .with(Dependency::RuntimeActivity)
            .with(Dependency::SessionEffects)
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

    async fn status_and_body(bindings: Bindings, path: &str) -> (StatusCode, serde_json::Value) {
        let response = router(bindings)
            .oneshot(Request::get(path).body(Body::empty()).expect("a request"))
            .await
            .expect("a response");
        let status = response.status();
        let bytes = axum::body::to_bytes(response.into_body(), 64 * 1024)
            .await
            .expect("a bounded body");
        (
            status,
            serde_json::from_slice(&bytes).expect("the probe body is JSON"),
        )
    }

    #[tokio::test]
    async fn liveness_answers_while_readiness_still_names_what_is_missing() {
        let (status, body) = status_and_body(Bindings::default(), HEALTHZ_PATH).await;
        assert_eq!(status, StatusCode::OK, "liveness is not readiness");
        assert_eq!(body["status"], "ok");

        let (status, body) = status_and_body(Bindings::default(), READYZ_PATH).await;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(body["status"], "not_ready");
        assert_eq!(
            body["missing"],
            serde_json::json!([
                "runtime-activity",
                "session-open-effects",
                "microvm-control",
                "usage-compute-sink",
                "usage-storage-sink"
            ])
        );

        let bound = Dependency::ALL
            .into_iter()
            .fold(Bindings::default(), Bindings::with);
        let (status, body) = status_and_body(bound, READYZ_PATH).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["status"], "ready");
        assert_eq!(body["missing"], serde_json::json!([]));
        assert_eq!(
            body["workDomains"],
            serde_json::json!(["runtime.workspace_discard", "runtime.live_workspace_wake"])
        );
        assert_eq!(
            body["usageCategories"],
            serde_json::json!(["compute", "storage"]),
            "a probe that named a transfer authority would be the first sign of a wrong binding"
        );
    }
}
