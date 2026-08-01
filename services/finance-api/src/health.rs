//! `/internal/healthz` and `/internal/readyz`.
//!
//! The two are deliberately different questions. Liveness asks whether the
//! process is running; readiness asks whether this deployable has proved its
//! own database grants. A process that answers `healthz` but cannot prove its
//! role must not receive traffic, so `readyz` fails until the probe passes and
//! fails again as soon as it stops passing.

use std::sync::atomic::{AtomicBool, Ordering};

use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use serde::Serialize;

use crate::mount::AppState;

/// The readiness gate.
#[derive(Debug, Default)]
pub struct Readiness {
    ready: AtomicBool,
    /// Why readiness is not held, when it is not.
    reason: parking_lot::Mutex<Option<String>>,
}

impl Readiness {
    /// A gate that is not ready yet.
    #[must_use]
    pub fn pending() -> Self {
        Self {
            ready: AtomicBool::new(false),
            reason: parking_lot::Mutex::new(Some("the role probe has not run".to_owned())),
        }
    }

    /// Records that this deployable proved its own grants.
    pub fn hold(&self) {
        *self.reason.lock() = None;
        self.ready.store(true, Ordering::Release);
    }

    /// Records that this deployable can no longer prove its own grants.
    pub fn release(&self, reason: impl Into<String>) {
        *self.reason.lock() = Some(reason.into());
        self.ready.store(false, Ordering::Release);
    }

    /// Whether traffic may be served.
    #[must_use]
    pub fn is_ready(&self) -> bool {
        self.ready.load(Ordering::Acquire)
    }

    /// Why readiness is not held.
    #[must_use]
    pub fn reason(&self) -> Option<String> {
        self.reason.lock().clone()
    }
}

/// The probe body. Deliberately tiny and free of any account detail.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Probe {
    /// `ok`, `ready` or `not_ready`.
    pub status: &'static str,
    /// Why readiness is not held, when it is not.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// Liveness: the process is running and can answer.
pub async fn health() -> Json<Probe> {
    Json(Probe {
        status: "ok",
        reason: None,
    })
}

/// Readiness: this deployable has proved its own database grants.
pub async fn ready<E, A>(State(state): State<AppState<E, A>>) -> (StatusCode, Json<Probe>)
where
    E: Send + Sync + 'static,
    A: Send + Sync + 'static,
{
    if state.readiness.is_ready() {
        (
            StatusCode::OK,
            Json(Probe {
                status: "ready",
                reason: None,
            }),
        )
    } else {
        (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(Probe {
                status: "not_ready",
                reason: state.readiness.reason(),
            }),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::Readiness;

    #[test]
    fn a_fresh_gate_is_closed_and_says_why() {
        let gate = Readiness::pending();
        assert!(!gate.is_ready());
        assert!(gate.reason().is_some());
    }

    #[test]
    fn the_gate_opens_only_on_a_proved_probe_and_closes_again() {
        let gate = Readiness::pending();
        gate.hold();
        assert!(gate.is_ready());
        assert!(gate.reason().is_none());
        gate.release("the role lost SELECT on finance.account_balance");
        assert!(!gate.is_ready());
        assert!(
            gate.reason()
                .is_some_and(|reason| reason.contains("SELECT"))
        );
    }
}
