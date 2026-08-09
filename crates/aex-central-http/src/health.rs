//! The two internal endpoints every central deployable exposes.
//!
//! `/internal/healthz` says the process is alive; `/internal/readyz` says every
//! declared dependency resolved. They are separate because a process that is
//! alive but not ready must be left running and taken out of rotation, not
//! killed and restarted into the same unresolved dependency.
//!
//! Readiness is **fail-closed**: a composition starts not-ready and becomes
//! ready only once each probe has actually answered.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use axum::extract::State;
use axum::http::{StatusCode, header};
use axum::response::IntoResponse as _;
use axum::routing::get;
use axum::{Json, Router};
use serde::Serialize;

/// The path the load balancer polls for liveness.
pub const HEALTH_PATH: &str = "/internal/healthz";
/// The path the load balancer polls for readiness.
pub const READY_PATH: &str = "/internal/readyz";

/// The one name a draining process reports as unresolved.
///
/// A long-lived host raises its drain flag **before** it asks the listener to
/// stop, so the load balancer sees `503` on [`READY_PATH`] and deregisters the
/// target while the requests it already accepted finish. The name is part of the
/// readiness body, so it is stated once here rather than re-spelled by every
/// host that drains.
pub const DRAINING: &str = "draining";

/// One named start-up dependency.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Dependency {
    /// A stable, low-cardinality name. Never a provider message.
    pub name: &'static str,
    /// Whether it resolved.
    pub resolved: bool,
}

impl Dependency {
    /// A dependency that resolved.
    #[must_use]
    pub const fn resolved(name: &'static str) -> Self {
        Self {
            name,
            resolved: true,
        }
    }

    /// A dependency that did not.
    #[must_use]
    pub const fn unresolved(name: &'static str) -> Self {
        Self {
            name,
            resolved: false,
        }
    }
}

/// The readiness of one composition.
///
/// A Lambda resolves its dependencies once and never drains, so it carries no
/// drain signal and this value is constant for the life of the process. A
/// long-lived host binds one with [`Readiness::with_drain_signal`] and the same
/// two endpoints then answer `503` for as long as the flag is raised.
#[derive(Debug, Clone)]
pub struct Readiness {
    deployable: &'static str,
    dependencies: Vec<Dependency>,
    draining: Option<Arc<AtomicBool>>,
}

/// Equality is over what the endpoints would render, which is why the drain
/// signal is compared by its current value rather than by handle identity: two
/// readiness values that answer identically are the same readiness.
impl PartialEq for Readiness {
    fn eq(&self, other: &Self) -> bool {
        self.deployable == other.deployable
            && self.dependencies == other.dependencies
            && self.is_draining() == other.is_draining()
    }
}

impl Eq for Readiness {}

impl Readiness {
    /// Builds a readiness projection over the declared dependencies.
    ///
    /// A composition with **no** declared dependency is never ready: a probe
    /// list nobody filled in is a missing probe, not a healthy process.
    #[must_use]
    pub fn new(deployable: &'static str, dependencies: Vec<Dependency>) -> Self {
        Self {
            deployable,
            dependencies,
            draining: None,
        }
    }

    /// Binds a live drain signal to an already-resolved readiness.
    ///
    /// Additive on purpose: a Lambda never drains, so a composition that does
    /// not bind one keeps the exact status and body it had before this existed.
    /// A host that binds one answers `503` on [`READY_PATH`] the moment it
    /// raises the flag, which is what lets the load balancer deregister the
    /// target before the drain deadline runs.
    #[must_use]
    pub fn with_drain_signal(mut self, draining: Arc<AtomicBool>) -> Self {
        self.draining = Some(draining);
        self
    }

    /// Whether a bound drain signal is currently raised.
    #[must_use]
    pub fn is_draining(&self) -> bool {
        self.draining
            .as_ref()
            .is_some_and(|flag| flag.load(Ordering::Acquire))
    }

    /// Whether every declared dependency resolved and no drain is running.
    #[must_use]
    pub fn is_ready(&self) -> bool {
        !self.dependencies.is_empty()
            && self.dependencies.iter().all(|it| it.resolved)
            && !self.is_draining()
    }

    /// The dependencies that did not resolve, by name.
    ///
    /// A draining process names [`DRAINING`] first: the reason it is refusing
    /// traffic is that it is going away, not that a dependency broke, and an
    /// operator reading the body during a rollout must be able to tell those
    /// two apart at a glance.
    #[must_use]
    pub fn unresolved(&self) -> Vec<&'static str> {
        let mut names = if self.is_draining() {
            vec![DRAINING]
        } else {
            Vec::new()
        };
        if self.dependencies.is_empty() {
            names.push("no dependency was probed");
            return names;
        }
        names.extend(
            self.dependencies
                .iter()
                .filter(|it| !it.resolved)
                .map(|it| it.name),
        );
        names
    }
}

/// The rendered probe body.
#[derive(Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ProbeBody {
    /// `healthy`, `ready` or `not_ready`.
    pub status: &'static str,
    /// Which deployable answered.
    pub deployable: &'static str,
    /// Every dependency that did not resolve.
    pub unresolved: Vec<&'static str>,
}

/// The internal router. A public listener must never forward these paths.
pub fn router(readiness: Readiness) -> Router {
    Router::new()
        .route(HEALTH_PATH, get(healthz))
        .route(READY_PATH, get(readyz))
        .with_state(Arc::new(readiness))
}

async fn healthz(State(readiness): State<Arc<Readiness>>) -> axum::response::Response {
    (
        StatusCode::OK,
        [(header::CACHE_CONTROL, "no-store")],
        Json(ProbeBody {
            status: "healthy",
            deployable: readiness.deployable,
            unresolved: Vec::new(),
        }),
    )
        .into_response()
}

async fn readyz(State(readiness): State<Arc<Readiness>>) -> axum::response::Response {
    let ready = readiness.is_ready();
    (
        if ready {
            StatusCode::OK
        } else {
            StatusCode::SERVICE_UNAVAILABLE
        },
        [(header::CACHE_CONTROL, "no-store")],
        Json(ProbeBody {
            status: if ready { "ready" } else { "not_ready" },
            deployable: readiness.deployable,
            unresolved: readiness.unresolved(),
        }),
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::{Dependency, HEALTH_PATH, READY_PATH, Readiness, router};
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use http_body_util::BodyExt as _;
    use tower::ServiceExt as _;

    async fn probe(readiness: Readiness, path: &str) -> (StatusCode, String) {
        let response = router(readiness)
            .oneshot(
                Request::builder()
                    .uri(path)
                    .body(Body::empty())
                    .expect("a valid request"),
            )
            .await
            .expect("the router answers");
        let status = response.status();
        let bytes = response
            .into_body()
            .collect()
            .await
            .expect("a complete body")
            .to_bytes();
        (status, String::from_utf8_lossy(&bytes).into_owned())
    }

    #[tokio::test]
    async fn liveness_answers_while_a_dependency_is_still_unresolved() {
        let readiness = Readiness::new(
            "central-control-api",
            vec![Dependency::unresolved("aurora")],
        );
        let (status, body) = probe(readiness, HEALTH_PATH).await;
        assert_eq!(status, StatusCode::OK);
        assert!(body.contains("healthy"), "{body}");
    }

    #[tokio::test]
    async fn readiness_names_every_unresolved_dependency() {
        let readiness = Readiness::new(
            "central-control-api",
            vec![
                Dependency::resolved("aurora"),
                Dependency::unresolved("control-pepper"),
            ],
        );
        let (status, body) = probe(readiness, READY_PATH).await;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
        assert!(body.contains("control-pepper"), "{body}");
        assert!(
            !body.contains("aurora"),
            "a resolved dependency is not named"
        );
    }

    #[tokio::test]
    async fn a_composition_that_probed_nothing_is_never_ready() {
        let readiness = Readiness::new("central-authz", Vec::new());
        assert!(!readiness.is_ready());
        let (status, _) = probe(readiness, READY_PATH).await;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    }

    #[tokio::test]
    async fn a_fully_resolved_composition_is_ready() {
        let readiness = Readiness::new("central-authz", vec![Dependency::resolved("aurora")]);
        let (status, body) = probe(readiness, READY_PATH).await;
        assert_eq!(status, StatusCode::OK);
        assert!(body.contains("ready"), "{body}");
    }

    #[tokio::test]
    async fn a_raised_drain_flag_takes_a_ready_composition_out_of_rotation() {
        let draining = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let readiness = Readiness::new("central-api", vec![Dependency::resolved("aurora")])
            .with_drain_signal(std::sync::Arc::clone(&draining));
        let (status, _) = probe(readiness.clone(), READY_PATH).await;
        assert_eq!(status, StatusCode::OK);

        draining.store(true, std::sync::atomic::Ordering::Release);
        let (status, body) = probe(readiness.clone(), READY_PATH).await;
        assert_eq!(
            status,
            StatusCode::SERVICE_UNAVAILABLE,
            "a draining target must be deregistered before its listener stops"
        );
        assert!(body.contains(super::DRAINING), "{body}");

        // Liveness is unchanged: a draining process is still running, and a
        // liveness failure would have the runtime kill it mid-drain.
        let (status, _) = probe(readiness, HEALTH_PATH).await;
        assert_eq!(status, StatusCode::OK);
    }

    #[tokio::test]
    async fn a_composition_that_binds_no_drain_signal_is_unchanged() {
        let readiness = Readiness::new("finance-api", vec![Dependency::resolved("aurora")]);
        assert!(!readiness.is_draining());
        assert!(readiness.is_ready());
        assert!(readiness.unresolved().is_empty());
    }
}
