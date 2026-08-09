//! Shared internal health and fail-closed readiness endpoints.

use std::borrow::Cow;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use axum::extract::State;
use axum::http::{StatusCode, header};
use axum::response::IntoResponse as _;
use axum::routing::get;
use axum::{Json, Router};
use serde::Serialize;

/// The one name a draining task reports as unavailable.
///
/// A long-lived host flips its drain flag before it asks the listener to stop,
/// so the load balancer sees `503` and deregisters the target while the
/// in-flight requests finish. The name is part of the readiness body, so it is
/// stated once here rather than re-spelled by every host that drains.
const DRAINING: &str = "draining";

/// Resolved readiness inputs for one deployable.
///
/// A Lambda resolves its dependencies once and never drains, so it carries no
/// drain signal and this value is a constant for the life of the process. A
/// long-lived host adds one with [`Readiness::with_drain_signal`] and the same
/// two endpoints then answer `503` for as long as the flag is raised.
#[derive(Debug, Clone)]
pub struct Readiness {
    release_digest: String,
    unavailable: Vec<String>,
    draining: Option<Arc<AtomicBool>>,
}

/// Equality is over what the endpoints would render, which is why the drain
/// signal is compared by its current value rather than by handle identity: two
/// readiness values that answer identically are the same readiness.
impl PartialEq for Readiness {
    fn eq(&self, other: &Self) -> bool {
        self.release_digest == other.release_digest
            && self.unavailable == other.unavailable
            && self.is_draining() == other.is_draining()
    }
}

impl Eq for Readiness {}

impl Readiness {
    /// Marks every declared dependency ready.
    #[must_use]
    pub fn ready(release_digest: impl Into<String>) -> Self {
        Self {
            release_digest: release_digest.into(),
            unavailable: Vec::new(),
            draining: None,
        }
    }

    /// Names every unavailable dependency without provider detail.
    ///
    /// # Errors
    ///
    /// Returns [`ReadinessError`] for an empty or malformed dependency list.
    pub fn not_ready<I, S>(
        release_digest: impl Into<String>,
        unavailable: I,
    ) -> Result<Self, ReadinessError>
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        let unavailable = unavailable.into_iter().map(Into::into).collect::<Vec<_>>();
        if unavailable.is_empty()
            || unavailable.len() > 32
            || unavailable.iter().any(|name| {
                name.is_empty()
                    || name.len() > 128
                    || name.bytes().any(|byte| byte.is_ascii_control())
            })
        {
            return Err(ReadinessError::InvalidUnavailable);
        }
        Ok(Self {
            release_digest: release_digest.into(),
            unavailable,
            draining: None,
        })
    }

    /// Binds a live drain signal to an already-resolved readiness.
    ///
    /// Additive on purpose: a Lambda never drains, so its readiness keeps the
    /// exact status, body and headers it had before this existed. A host that
    /// binds one gets `503` on both readiness surfaces the moment it raises the
    /// flag, which is what lets the load balancer deregister the target before
    /// the drain deadline runs.
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

    pub(crate) fn is_ready(&self) -> bool {
        self.unavailable.is_empty() && !self.is_draining()
    }

    /// The dependency names the readiness body reports right now.
    ///
    /// Draining is reported as one more unavailable name rather than as a
    /// separate field, so the body shape is the same whether the task is
    /// missing a dependency or shutting down.
    pub(crate) fn reported_unavailable(&self) -> Cow<'_, [String]> {
        if self.is_draining() {
            let mut names = self.unavailable.clone();
            names.push(DRAINING.to_owned());
            Cow::Owned(names)
        } else {
            Cow::Borrowed(&self.unavailable)
        }
    }

    pub(crate) fn release_digest(&self) -> &str {
        &self.release_digest
    }
}

/// Invalid readiness projection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum ReadinessError {
    /// Dependency names were absent or outside the closed bound.
    #[error("readiness unavailable list is invalid")]
    InvalidUnavailable,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct HealthBody<'a> {
    status: &'static str,
    release_digest: &'a str,
    unavailable: &'a [String],
}

/// Builds the two internal endpoints. Public listener policy must not forward them.
pub fn router(readiness: Readiness) -> Router {
    Router::new()
        .route("/internal/healthz", get(healthz))
        .route("/internal/readyz", get(readyz))
        .with_state(readiness)
}

async fn healthz(State(readiness): State<Readiness>) -> impl axum::response::IntoResponse {
    let body = HealthBody {
        status: "healthy",
        release_digest: &readiness.release_digest,
        unavailable: &[],
    };
    (
        StatusCode::OK,
        [(header::CACHE_CONTROL, "no-store")],
        Json(body),
    )
        .into_response()
}

async fn readyz(State(readiness): State<Readiness>) -> impl axum::response::IntoResponse {
    let ready = readiness.is_ready();
    let unavailable = readiness.reported_unavailable();
    let body = HealthBody {
        status: if ready { "ready" } else { "not_ready" },
        release_digest: &readiness.release_digest,
        unavailable: &unavailable,
    };
    (
        if ready {
            StatusCode::OK
        } else {
            StatusCode::SERVICE_UNAVAILABLE
        },
        [(header::CACHE_CONTROL, "no-store")],
        Json(body),
    )
        .into_response()
}
