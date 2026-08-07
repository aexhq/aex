//! The two health bodies every Rust deployable answers with.
//!
//! This deployable is a scheduled Lambda: it has no listener, and the Lambda
//! runtime API loop is owned by `lambda_runtime`, so a second HTTP server could
//! never be reached. The cross-stream health surface is therefore rendered here
//! as the two JSON bodies and routed by
//! [`crate::handler::classify`](crate::handler::classify) against
//! [`aex_observation_store_dynamodb::health::HEALTHZ`] and
//! [`aex_observation_store_dynamodb::health::READYZ`] — the paths are read from those
//! constants and never spelled out.
//!
//! `readyz` answers `200` only once **every** declared probe has actually
//! passed, and names the first outstanding one otherwise. A readiness endpoint
//! that answers before its dependencies are proven is worse than none.

use aex_observation_store_dynamodb::health::{Probe, Readiness, readiness};

/// The dependencies this deployable proves before it reports ready.
pub const REQUIRED_PROBES: &[Probe] = &[Probe::ObservationTable, Probe::ObservationBucket];

/// The HTTP status a proven readiness answers with.
pub const READY_STATUS: u16 = 200;

/// The HTTP status an unproven readiness answers with.
pub const NOT_READY_STATUS: u16 = 503;

/// Liveness: the process is running.
#[must_use]
pub fn health_body(release_digest: &str) -> serde_json::Value {
    serde_json::json!({
        "status": "healthy",
        "releaseDigest": release_digest,
    })
}

/// Readiness: every declared probe has actually passed.
///
/// Returns the status alongside the body so the caller renders one decision
/// rather than re-deriving it from the payload.
#[must_use]
pub fn readiness_body(release_digest: &str, passed: &[Probe]) -> (u16, serde_json::Value) {
    match readiness(REQUIRED_PROBES, passed) {
        Readiness::Ready => (
            READY_STATUS,
            serde_json::json!({
                "status": "ready",
                "releaseDigest": release_digest,
            }),
        ),
        Readiness::NotReady { outstanding } => (
            NOT_READY_STATUS,
            serde_json::json!({
                "status": "not_ready",
                "releaseDigest": release_digest,
                "outstanding": outstanding.as_str(),
            }),
        ),
    }
}

#[cfg(test)]
mod tests {
    use aex_observation_store_dynamodb::health::Probe;

    use super::{NOT_READY_STATUS, READY_STATUS, REQUIRED_PROBES, health_body, readiness_body};

    #[test]
    fn a_prefix_of_the_probe_set_is_not_the_probe_set() {
        let (status, body) = readiness_body("d", &[Probe::ObservationTable]);
        assert_eq!(status, NOT_READY_STATUS);
        assert_eq!(
            body["outstanding"].as_str(),
            Some(Probe::ObservationBucket.as_str())
        );
    }

    #[test]
    fn the_complete_probe_set_reports_ready_with_the_release_digest() {
        let (status, body) = readiness_body("sha256:feed", REQUIRED_PROBES);
        assert_eq!(status, READY_STATUS);
        assert_eq!(body["status"].as_str(), Some("ready"));
        assert_eq!(body["releaseDigest"].as_str(), Some("sha256:feed"));
        assert!(body.get("outstanding").is_none());
    }

    #[test]
    fn liveness_never_claims_readiness() {
        let body = health_body("unreleased");
        assert_eq!(body["status"].as_str(), Some("healthy"));
        assert_ne!(body["status"].as_str(), Some("ready"));
        assert_eq!(REQUIRED_PROBES.len(), 2);
    }
}
