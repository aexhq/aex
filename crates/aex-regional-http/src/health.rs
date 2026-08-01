//! Shared internal health and fail-closed readiness endpoints.

use axum::extract::State;
use axum::http::{StatusCode, header};
use axum::response::IntoResponse as _;
use axum::routing::get;
use axum::{Json, Router};
use serde::Serialize;

/// Resolved readiness inputs for one deployable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Readiness {
    release_digest: String,
    unavailable: Vec<String>,
}

impl Readiness {
    /// Marks every declared dependency ready.
    #[must_use]
    pub fn ready(release_digest: impl Into<String>) -> Self {
        Self {
            release_digest: release_digest.into(),
            unavailable: Vec::new(),
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
        })
    }

    const fn is_ready(&self) -> bool {
        self.unavailable.is_empty()
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
    let body = HealthBody {
        status: if ready { "ready" } else { "not_ready" },
        release_digest: &readiness.release_digest,
        unavailable: &readiness.unavailable,
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
