//! The one handler, and the order it does things in.
//!
//! `POST /internal/tool-exec` is the entire request surface. There is no route
//! table, no dispatcher and no generated contract, because there is one
//! operation and it is internal: a second route here would be a second thing to
//! reason about on the only path that touches the platform's own money.

use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use aex_internal_contracts::SchemaVersion;
use aex_internal_contracts::tool_exec::{
    ToolExecRefusal, ToolExecRequest, ToolExecResponse, ToolResultPart,
};
use aex_wire::ids::ContentHash;
use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};

use crate::admit::Admitter;
use crate::config::MAX_IN_FLIGHT;
use crate::run::{RunRefusal, ToolRunner};
use crate::spend::{CeilingRefusal, OrganizationCeiling};

/// Everything one request needs, assembled once at start-up.
pub struct Executor {
    admitter: Admitter,
    manifest: ContentHash,
    ceiling: Arc<dyn OrganizationCeiling>,
    runner: Arc<dyn ToolRunner>,
    /// The backstop, not the bound. Sized at every call the deployed Brain fleet
    /// can produce at once, so under the fleet it never blocks; it exists so that
    /// a fleet larger than the one this number was derived from queues here
    /// rather than exhausting sockets and memory.
    in_flight: tokio::sync::Semaphore,
}

impl Executor {
    /// Composes the executor.
    #[must_use]
    pub fn new(
        admitter: Admitter,
        manifest: ContentHash,
        ceiling: Arc<dyn OrganizationCeiling>,
        runner: Arc<dyn ToolRunner>,
    ) -> Self {
        Self {
            admitter,
            manifest,
            ceiling,
            runner,
            in_flight: tokio::sync::Semaphore::new(MAX_IN_FLIGHT),
        }
    }

    /// Whether this process can serve.
    ///
    /// Readiness rather than liveness: a process with no verification key would
    /// refuse every request while looking perfectly healthy, and that is exactly
    /// the shape of failure a readiness probe exists to catch.
    #[must_use]
    pub fn is_ready(&self) -> bool {
        self.admitter.has_keys()
    }

    /// Serves one call.
    ///
    /// The order is the contract: verify, admit against the ceiling, and only
    /// then run — which is the only step that touches the vendor credential.
    pub async fn execute(&self, request: &ToolExecRequest, at: SystemTime) -> ToolExecResponse {
        // The semaphore is never closed while the process serves, so a closed one
        // is shutdown. Refusing is the honest answer: the caller retries or
        // settles, and neither is helped by a request that waits for a process
        // that is leaving.
        let Ok(_in_flight) = self.in_flight.acquire().await else {
            return refused(ToolExecRefusal::NotAuthorized);
        };
        let now_ms = at.duration_since(UNIX_EPOCH).map_or(u64::MAX, |since| {
            u64::try_from(since.as_millis()).unwrap_or(u64::MAX)
        });

        // Step 2-7. Every refusal collapses to one answer on the wire:
        // distinguishing them would tell a caller which rule it failed.
        let Ok(call) = self.admitter.admit(request, now_ms) else {
            return refused(ToolExecRefusal::NotAuthorized);
        };

        // The assertion authenticates the caller, not the schema it validated.
        // Refuse a different catalog before the spend ceiling or credential is
        // touched so two release revisions cannot silently disagree.
        if request.manifest != self.manifest {
            return refused(ToolExecRefusal::Unsupported);
        }

        // Step 8, before the credential is touched, and independent of whatever
        // the Brain checked on its own side. This is the only bound in the
        // system that survives a buggy or compromised Brain.
        let permit = match self.ceiling.admit(call.organization(), at).await {
            Ok(permit) => permit,
            // An unreadable ceiling is refused rather than admitted. A ceiling
            // nobody could read is how an organization keeps spending.
            Err(CeilingRefusal::LimitExceeded | CeilingRefusal::Unavailable(_)) => {
                return refused(ToolExecRefusal::LimitExceeded);
            }
        };

        // Step 9. `run` requires the permit as an argument, so there is no path
        // to here that skipped step 8.
        let started = SystemTime::now();
        match self
            .runner
            .run(&permit, &request.tool, &request.arguments_jcs)
            .await
        {
            Ok(outcome) => {
                let duration_ms = started
                    .elapsed()
                    .ok()
                    .and_then(|elapsed| u32::try_from(elapsed.as_millis()).ok())
                    .unwrap_or(u32::MAX);
                ToolExecResponse::Completed {
                    schema_version: SchemaVersion::V1,
                    checksum: checksum(&outcome.content),
                    content: outcome.content,
                    is_error: outcome.is_error,
                    duration_ms,
                }
            }
            Err(RunRefusal::Unsupported { .. }) => refused(ToolExecRefusal::Unsupported),
            // A vendor fault and a malformed argument document are both "we did
            // not run it", and neither is the caller's ceiling. They collapse to
            // `unsupported` on the wire rather than acquiring an arm that would
            // let a caller distinguish our failure from their own.
            Err(RunRefusal::InvalidArguments | RunRefusal::Vendor) => {
                refused(ToolExecRefusal::Unsupported)
            }
        }
    }
}

const fn refused(reason: ToolExecRefusal) -> ToolExecResponse {
    ToolExecResponse::Refused {
        schema_version: SchemaVersion::V1,
        reason,
    }
}

/// A digest over the canonical result content, so a caller can verify the body
/// it received is the body that was produced.
fn checksum(content: &[ToolResultPart]) -> ContentHash {
    let canonical = serde_json::to_vec(content).unwrap_or_default();
    ContentHash::from_bytes(*blake3::hash(&canonical).as_bytes())
}

/// The private listener's routes: one operation, one liveness probe, one
/// readiness probe, and nothing else.
pub fn router(executor: Arc<Executor>) -> axum::Router {
    axum::Router::new()
        .route("/internal/tool-exec", post(execute))
        .route("/internal/healthz", get(|| async { StatusCode::OK }))
        .route(
            "/internal/readyz",
            get(|State(state): State<Arc<Executor>>| async move {
                if state.is_ready() {
                    StatusCode::OK
                } else {
                    StatusCode::SERVICE_UNAVAILABLE
                }
            }),
        )
        .with_state(executor)
}

async fn execute(
    State(executor): State<Arc<Executor>>,
    Json(request): Json<ToolExecRequest>,
) -> Response {
    let response = executor.execute(&request, SystemTime::now()).await;
    // Every outcome is a 200 with a typed body, refusals included. A refusal is
    // an answer the authority gave, not a transport failure, and a caller that
    // had to read a status code to tell them apart would have two vocabularies
    // for one thing.
    (StatusCode::OK, Json(response)).into_response()
}
