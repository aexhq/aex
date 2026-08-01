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
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Readiness {
    deployable: &'static str,
    dependencies: Vec<Dependency>,
}

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
        }
    }

    /// Whether every declared dependency resolved.
    #[must_use]
    pub fn is_ready(&self) -> bool {
        !self.dependencies.is_empty() && self.dependencies.iter().all(|it| it.resolved)
    }

    /// The dependencies that did not resolve, by name.
    #[must_use]
    pub fn unresolved(&self) -> Vec<&'static str> {
        if self.dependencies.is_empty() {
            return vec!["no dependency was probed"];
        }
        self.dependencies
            .iter()
            .filter(|it| !it.resolved)
            .map(|it| it.name)
            .collect()
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
}
