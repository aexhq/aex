//! The two internal health endpoints every Rust deployable mounts.
//!
//! This deployable is invoked on a schedule rather than over `HTTP`, so the two
//! paths are matched from the invocation payload instead of a router. The paths
//! themselves come from
//! [`aex_observation_store_aws::health`], never from a literal here: a
//! hand-typed `/internal/readyz` that drifts from the shared constant is a probe
//! that silently never runs.
//!
//! `healthz` answers as soon as the process is alive; `readyz` answers only once
//! every declared probe has actually passed, because a readiness endpoint that
//! answers before its dependencies are proven is worse than none.

use aex_observation_store_aws::health::{HEALTHZ, READYZ};

/// Everything a health answer needs, resolved once at start-up.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AppState {
    /// Whether every declared readiness probe has passed.
    pub ready: bool,
    /// The release digest both endpoints report.
    pub release_digest: String,
}

/// One rendered health answer.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HealthReply {
    /// The `HTTP` status the answer carries.
    pub status: u16,
    /// The reported status word.
    pub state: &'static str,
}

impl HealthReply {
    /// The status a live process reports.
    pub const HEALTHY: &'static str = "healthy";
    /// The status a fully probed process reports.
    pub const READY: &'static str = "ready";
    /// The status a process with an outstanding probe reports.
    pub const NOT_READY: &'static str = "not_ready";

    /// Renders the answer as the invocation result.
    #[must_use]
    pub fn to_response(&self, state: &AppState) -> serde_json::Value {
        serde_json::json!({
            "statusCode": self.status,
            "headers": {
                "content-type": "application/json",
                "cache-control": "no-store",
            },
            "body": serde_json::json!({
                "status": self.state,
                "releaseDigest": state.release_digest,
            })
            .to_string(),
        })
    }
}

/// The two paths this deployable answers, in declaration order.
pub const HEALTH_PATHS: [&str; 2] = [HEALTHZ, READYZ];

/// The path a probe invocation names, if it names one.
///
/// A scheduled sweep carries no path at all, which is how the two are told
/// apart without a second entry point.
#[must_use]
pub fn probe_path(payload: &serde_json::Value) -> Option<&str> {
    payload
        .get("rawPath")
        .or_else(|| payload.get("path"))
        .and_then(serde_json::Value::as_str)
}

/// Answers a health probe, or `None` when the path is not one of the two.
#[must_use]
pub fn health(state: &AppState, path: &str) -> Option<HealthReply> {
    if !HEALTH_PATHS.contains(&path) {
        return None;
    }
    if path == HEALTHZ {
        return Some(HealthReply {
            status: 200,
            state: HealthReply::HEALTHY,
        });
    }
    Some(if state.ready {
        HealthReply {
            status: 200,
            state: HealthReply::READY,
        }
    } else {
        HealthReply {
            status: 503,
            state: HealthReply::NOT_READY,
        }
    })
}

#[cfg(test)]
mod tests {
    use super::{AppState, HEALTH_PATHS, HealthReply, health, probe_path};

    fn state(ready: bool) -> AppState {
        AppState {
            ready,
            release_digest: "sha256:feed".to_owned(),
        }
    }

    #[test]
    fn the_health_paths_are_the_shared_cross_stream_constants() {
        assert_eq!(HEALTH_PATHS[0], aex_observation_store_aws::health::HEALTHZ);
        assert_eq!(HEALTH_PATHS[1], aex_observation_store_aws::health::READYZ);
        assert_eq!(HEALTH_PATHS[0], "/internal/healthz");
        assert_eq!(HEALTH_PATHS[1], "/internal/readyz");
    }

    #[test]
    fn liveness_answers_while_readiness_still_refuses() {
        let unready = state(false);
        let live = health(&unready, HEALTH_PATHS[0]).expect("healthz answers");
        assert_eq!(live.status, 200);
        assert_eq!(live.state, HealthReply::HEALTHY);

        let not_ready = health(&unready, HEALTH_PATHS[1]).expect("readyz answers");
        assert_eq!(not_ready.status, 503);
        assert_eq!(not_ready.state, HealthReply::NOT_READY);

        let ready = health(&state(true), HEALTH_PATHS[1]).expect("readyz answers");
        assert_eq!(ready.status, 200);
        assert_eq!(ready.state, HealthReply::READY);
    }

    #[test]
    fn no_other_path_is_answered() {
        for path in ["/readyz", "/internal/health", "/", "/internal/readyz/"] {
            assert!(
                health(&state(true), path).is_none(),
                "`{path}` must not be answered"
            );
        }
    }

    #[test]
    fn a_scheduled_sweep_carries_no_path_and_a_probe_does() {
        assert_eq!(probe_path(&serde_json::json!({})), None);
        assert_eq!(
            probe_path(&serde_json::json!({ "rawPath": "/internal/readyz" })),
            Some("/internal/readyz")
        );
        assert_eq!(
            probe_path(&serde_json::json!({ "path": "/internal/healthz" })),
            Some("/internal/healthz")
        );
        assert_eq!(probe_path(&serde_json::json!({ "path": 7 })), None);
    }

    #[test]
    fn the_rendered_answer_carries_the_release_digest_and_no_cache() {
        let state = state(true);
        let rendered = health(&state, HEALTH_PATHS[1])
            .expect("readyz answers")
            .to_response(&state);
        assert_eq!(rendered["statusCode"], 200);
        assert_eq!(rendered["headers"]["cache-control"], "no-store");
        let body: serde_json::Value =
            serde_json::from_str(rendered["body"].as_str().expect("a string body"))
                .expect("the body is JSON");
        assert_eq!(body["status"], HealthReply::READY);
        assert_eq!(body["releaseDigest"], "sha256:feed");
    }
}
