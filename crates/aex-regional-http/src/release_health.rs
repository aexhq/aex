//! Closed public release identity and readiness endpoint.

use axum::extract::State;
use axum::http::{StatusCode, header};
use axum::response::IntoResponse as _;
use axum::routing::get;
use axum::{Json, Router};
use serde::Serialize;

use crate::health::Readiness;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ReleaseHealthBody<'a> {
    schema: &'static str,
    release_id: &'a str,
    status: &'static str,
}

/// Builds the unauthenticated public release-health endpoint.
///
/// This router is separate from the internal health surface so only the
/// deployable selected as the public release-identity authority can mount it.
pub fn router(readiness: Readiness) -> Router {
    Router::new()
        .route("/api/release/health", get(release_health))
        .with_state(readiness)
}

async fn release_health(State(readiness): State<Readiness>) -> impl axum::response::IntoResponse {
    let ready = readiness.is_ready();
    let body = ReleaseHealthBody {
        schema: "aex.release-health.v1",
        release_id: readiness.release_digest(),
        status: if ready { "ready" } else { "not_ready" },
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
