//! `/internal/healthz` and `/internal/readyz`, on the constant paths the
//! observation store crate declares.
//!
//! The paths are never spelled here: they are
//! [`aex_observation_store_aws::health::HEALTHZ`] and
//! [`aex_observation_store_aws::health::READYZ`], so a rename in the
//! cross-stream contract cannot leave this deployable answering the old one.
//!
//! This is a one-shot task and its Fargate row declares `port = 0`, so the
//! loopback listener is bound **only** when a port is configured. The same two
//! bodies are always available through [`health_body`] and [`readiness_body`],
//! which is what the endpoints themselves answer with.

use std::net::{Ipv4Addr, SocketAddr};
use std::sync::Arc;

use aex_observation_store_aws::health::{HEALTHZ, Probe, READYZ, Readiness, readiness};
use axum::extract::State;
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};

/// What both endpoints report about this task.
#[derive(Clone, Debug)]
pub struct HealthState {
    /// The release digest both endpoints report.
    pub release_digest: String,
    /// The probes this deployable declares.
    pub required: &'static [Probe],
    /// The probes that have actually passed.
    pub passed: Vec<Probe>,
}

/// Why the health listener could not be bound.
#[derive(Debug, thiserror::Error)]
pub enum HealthError {
    /// The loopback listener could not be bound.
    #[error("the health listener could not bind to port {port}: {reason}")]
    Bind {
        /// The requested port.
        port: u16,
        /// What the operating system reported.
        reason: String,
    },
}

/// The body `/internal/healthz` answers: the process is alive.
#[must_use]
pub fn health_body(state: &HealthState) -> String {
    serde_json::json!({
        "status": "healthy",
        "releaseDigest": state.release_digest,
    })
    .to_string()
}

/// The status and body `/internal/readyz` answers.
///
/// A probe that has not passed is outstanding, never assumed, so an unproven
/// dependency answers `503` and names itself.
#[must_use]
pub fn readiness_body(state: &HealthState) -> (StatusCode, String) {
    match readiness(state.required, &state.passed) {
        Readiness::Ready => (
            StatusCode::OK,
            serde_json::json!({
                "status": "ready",
                "releaseDigest": state.release_digest,
            })
            .to_string(),
        ),
        Readiness::NotReady { outstanding } => (
            StatusCode::SERVICE_UNAVAILABLE,
            serde_json::json!({
                "status": "not_ready",
                "outstanding": outstanding.as_str(),
                "releaseDigest": state.release_digest,
            })
            .to_string(),
        ),
    }
}

/// The router carrying exactly the two internal endpoints.
pub fn router(state: Arc<HealthState>) -> axum::Router {
    axum::Router::new()
        .route(HEALTHZ, axum::routing::get(healthz))
        .route(READYZ, axum::routing::get(readyz))
        .with_state(state)
}

/// Liveness.
async fn healthz(State(state): State<Arc<HealthState>>) -> Response {
    (
        StatusCode::OK,
        [
            (header::CACHE_CONTROL, "no-store"),
            (header::CONTENT_TYPE, "application/json"),
        ],
        health_body(&state),
    )
        .into_response()
}

/// Readiness.
async fn readyz(State(state): State<Arc<HealthState>>) -> Response {
    let (status, body) = readiness_body(&state);
    (
        status,
        [
            (header::CACHE_CONTROL, "no-store"),
            (header::CONTENT_TYPE, "application/json"),
        ],
        body,
    )
        .into_response()
}

/// A bound loopback health listener.
#[derive(Debug)]
pub struct HealthServer {
    shutdown: tokio::sync::oneshot::Sender<()>,
    served: tokio::task::JoinHandle<()>,
    local_addr: SocketAddr,
}

impl HealthServer {
    /// Where the listener is bound.
    #[must_use]
    pub const fn local_addr(&self) -> SocketAddr {
        self.local_addr
    }

    /// Stops the listener and waits for it.
    pub async fn shutdown(self) {
        if self.shutdown.send(()).is_err() {
            tracing::debug!("the health listener had already stopped");
        }
        if let Err(error) = self.served.await {
            tracing::error!(error = %error, "the health listener did not stop cleanly");
        }
    }
}

/// Binds the loopback health listener.
///
/// # Errors
///
/// Returns [`HealthError::Bind`] when the port cannot be bound. A task that
/// declares a health port and cannot serve it refuses to run rather than
/// pretending to be observable.
pub async fn serve(port: u16, state: Arc<HealthState>) -> Result<HealthServer, HealthError> {
    let listener = tokio::net::TcpListener::bind(SocketAddr::from((Ipv4Addr::LOCALHOST, port)))
        .await
        .map_err(|error| HealthError::Bind {
            port,
            reason: error.to_string(),
        })?;
    let local_addr = listener.local_addr().map_err(|error| HealthError::Bind {
        port,
        reason: error.to_string(),
    })?;
    let (shutdown, signal) = tokio::sync::oneshot::channel();
    let served = tokio::spawn(async move {
        let outcome = axum::serve(listener, router(state))
            .with_graceful_shutdown(async move {
                if signal.await.is_err() {
                    tracing::debug!("the health shutdown channel closed; stopping");
                }
            })
            .await;
        if let Err(error) = outcome {
            tracing::error!(error = %error, "the health listener stopped");
        }
    });
    Ok(HealthServer {
        shutdown,
        served,
        local_addr,
    })
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use aex_observation_store_aws::health::{HEALTHZ, Probe, READYZ};
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use tower::ServiceExt as _;

    use super::{HealthState, health_body, readiness_body, router, serve};

    const REQUIRED: &[Probe] = &[Probe::ObservationTable, Probe::ObservationBucket];

    fn state(passed: Vec<Probe>) -> Arc<HealthState> {
        Arc::new(HealthState {
            release_digest: "sha256:test".to_owned(),
            required: REQUIRED,
            passed,
        })
    }

    async fn get(path: &str, state: Arc<HealthState>) -> (StatusCode, String) {
        let response = router(state)
            .oneshot(
                Request::builder()
                    .uri(path)
                    .body(Body::empty())
                    .expect("the request builds"),
            )
            .await
            .expect("the router answers");
        let status = response.status();
        let bytes = axum::body::to_bytes(response.into_body(), 64 * 1024)
            .await
            .expect("the body reads");
        (
            status,
            String::from_utf8(bytes.to_vec()).expect("the body is utf8"),
        )
    }

    #[test]
    fn the_two_bodies_are_available_without_any_listener() {
        let ready = state(REQUIRED.to_vec());
        assert!(health_body(&ready).contains(r#""status":"healthy""#));
        assert!(health_body(&ready).contains("sha256:test"));
        let (status, body) = readiness_body(&ready);
        assert_eq!(status, StatusCode::OK);
        assert!(body.contains(r#""status":"ready""#), "{body}");
    }

    #[test]
    fn an_unproven_probe_answers_service_unavailable_and_names_itself() {
        let (status, body) = readiness_body(&state(Vec::new()));
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
        assert!(body.contains("observation_table"), "{body}");

        let (status, body) = readiness_body(&state(vec![Probe::ObservationTable]));
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
        assert!(
            body.contains("observation_bucket"),
            "proving a prefix is not proving the set: {body}"
        );
    }

    #[tokio::test]
    async fn both_endpoints_answer_on_the_declared_paths() {
        let (status, body) = get(HEALTHZ, state(REQUIRED.to_vec())).await;
        assert_eq!(status, StatusCode::OK);
        assert!(body.contains("healthy"), "{body}");

        let (status, body) = get(READYZ, state(REQUIRED.to_vec())).await;
        assert_eq!(status, StatusCode::OK);
        assert!(body.contains("ready"), "{body}");

        let (status, _) = get(READYZ, state(Vec::new())).await;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    }

    #[tokio::test]
    async fn the_superseded_brain_paths_are_not_mounted() {
        for path in ["/healthz", "/readyz", "/internal/health"] {
            let (status, _) = get(path, state(REQUIRED.to_vec())).await;
            assert_eq!(status, StatusCode::NOT_FOUND, "{path} must not be mounted");
        }
    }

    #[tokio::test]
    async fn a_configured_port_binds_a_real_loopback_listener() {
        let server = serve(0, state(REQUIRED.to_vec()))
            .await
            .expect("an ephemeral port binds");
        assert!(server.local_addr().ip().is_loopback());
        assert_ne!(server.local_addr().port(), 0);
        server.shutdown().await;
    }
}
