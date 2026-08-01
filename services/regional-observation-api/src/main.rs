//! `regional-observation-api` composition root (Rust Lambda ZIP).
//!
//! Exclusive responsibility: the finite customer observation query, gap and
//! export-control surface.
//!
//! There is no projection and no materializer. The `observation-authority`
//! table is both the authority and the query engine, so `503
//! observability_unavailable` now means the authority itself is unreachable and
//! `caughtUp: false` means "inside the two-second index settle window" rather
//! than an unbounded materializer backlog.

mod api;
mod config;
mod edge;
mod mount;
mod ndjson;
mod query;
mod reader;

use std::sync::Arc;

use aex_observation_store_aws::composition::{Capability, Role, assert_grant};
use aex_observation_store_aws::health::{Probe, Readiness, readiness};
use aex_regional_http::cursor::{CursorKey, CursorKeyRing};
use aex_wire::dispatch::RequestLimits;

use crate::api::ObservationService;
use crate::config::{Config, ConfigError, REQUIRED_VARS};
use crate::edge::{AUDIENCE, Ed25519Anchors, HttpAssertionSource, RequestAuthority};
use crate::mount::AppState;
use crate::reader::ObservationReader;

/// The capability grant this deployable is allowed to hold.
pub const ROLE: Role = Role::ObservationApi;

/// The dependencies this deployable proves before it reports ready.
pub const REQUIRED_PROBES: &[Probe] = &[
    Probe::ObservationTable,
    Probe::ObservationBucket,
    Probe::CursorKeyRing,
    Probe::SessionAuthority,
];

/// The key id every cursor this deployment mints is signed under.
pub const CURSOR_KEY_ID: &str = "observation-cursor";

/// Why `regional-observation-api` stopped.
#[derive(Debug, thiserror::Error)]
pub enum RunError {
    /// Start-up configuration was rejected.
    #[error(transparent)]
    Config(#[from] ConfigError),
    /// The process holds a capability its role must not.
    #[error("this deployable must not hold the `{capability}` capability")]
    Capability {
        /// The offending capability.
        capability: &'static str,
    },
    /// A declared readiness probe has not passed.
    #[error("the `{probe}` dependency has not been proven")]
    NotReady {
        /// The outstanding probe.
        probe: &'static str,
    },
    /// The edge could not be composed.
    #[error("the authenticated edge could not be composed: {reason}")]
    Edge {
        /// What failed.
        reason: String,
    },
    /// A start-up probe failed against a real resource.
    #[error("a start-up probe failed: {reason}")]
    Probe {
        /// What failed.
        reason: String,
    },
    /// The Lambda runtime stopped.
    #[error("the runtime stopped: {reason}")]
    Runtime {
        /// What failed.
        reason: String,
    },
}

/// Asserts the observed capability grant and the readiness probe set.
///
/// # Errors
///
/// Returns [`RunError::Capability`] when the process holds a capability its role
/// must not, and [`RunError::NotReady`] when a declared probe has not passed. A
/// probe that has not passed is never assumed.
pub fn compose(observed: &[Capability], passed: &[Probe]) -> Result<(), RunError> {
    assert_grant(ROLE, observed).map_err(|violation| RunError::Capability {
        capability: violation.capability.as_str(),
    })?;
    match readiness(REQUIRED_PROBES, passed) {
        Readiness::Ready => Ok(()),
        Readiness::NotReady { outstanding } => Err(RunError::NotReady {
            probe: outstanding.as_str(),
        }),
    }
}

/// Builds every adapter, proves every probe and serves until the runtime stops.
///
/// # Errors
///
/// Returns the typed failure of the first start-up stage that refused. Nothing
/// is served before every declared probe has actually passed.
pub async fn run(config: Config) -> Result<(), RunError> {
    let aws = aws_config::from_env()
        .region(aws_config::Region::new(config.region.as_str()))
        .load()
        .await;
    let dynamodb = aws_sdk_dynamodb::Client::new(&aws);
    let s3 = aws_sdk_s3::Client::new(&aws);

    let reader = ObservationReader::new(
        dynamodb.clone(),
        s3,
        config.observation_table.clone(),
        config.observation_bucket.clone(),
        config.index_settle_ms,
    );

    let mut passed = Vec::new();
    reader.probe().await.map_err(|error| RunError::Probe {
        reason: error.to_string(),
    })?;
    passed.push(Probe::ObservationTable);
    passed.push(Probe::ObservationBucket);

    // The `events` signal is read through the semantic port over
    // `session-authority`; the table must be reachable before a route that
    // serves `events` is mounted.
    dynamodb
        .describe_table()
        .table_name(&config.session_table)
        .send()
        .await
        .map_err(|error| RunError::Probe {
            reason: format!("`session-authority` is not readable: {error}"),
        })?;

    let cursor_key = CursorKey::new(CURSOR_KEY_ID, config.cursor_key.clone()).map_err(|error| {
        RunError::Probe {
            reason: error.to_string(),
        }
    })?;
    let ring_key = CursorKey::new(CURSOR_KEY_ID, config.cursor_key.clone()).map_err(|error| {
        RunError::Probe {
            reason: error.to_string(),
        }
    })?;
    let ring = CursorKeyRing::new(ring_key, Vec::new()).map_err(|error| RunError::Probe {
        reason: error.to_string(),
    })?;
    passed.push(Probe::CursorKeyRing);
    passed.push(Probe::SessionAuthority);

    compose(ROLE.granted(), &passed)?;

    let source = HttpAssertionSource::new(&config.central_authz_url, AUDIENCE, config.region)
        .map_err(|error| RunError::Edge {
            reason: error.to_string(),
        })?;
    let anchors = Ed25519Anchors::new(config.trust_anchors.clone());
    if anchors.is_empty() {
        return Err(RunError::Edge {
            reason: "no assertion trust anchor resolved".to_owned(),
        });
    }
    tracing::info!(anchors = anchors.len(), "assertion trust anchors resolved");
    let edge = RequestAuthority::new(
        source,
        anchors,
        AUDIENCE,
        config.region,
        config.assertion_cache_bytes,
    )
    .map_err(|error| RunError::Edge {
        reason: error.to_string(),
    })?;

    let service = ObservationService::new(
        reader,
        config.budget,
        config.metric_aggregate_scan,
        cursor_key,
        ring,
        config.region,
    );
    let state = Arc::new(AppState {
        edge: Arc::new(edge),
        service: Arc::new(service),
        limits: RequestLimits::DEFAULT,
        ready: true,
        release_digest: release_digest(),
    });
    lambda_http::run(mount::router(state))
        .await
        .map_err(|error| RunError::Runtime {
            reason: error.to_string(),
        })
}

/// The release digest both health endpoints report.
fn release_digest() -> String {
    std::env::var("AEX_RELEASE_DIGEST").unwrap_or_else(|_| "unreleased".to_owned())
}

#[tokio::main]
async fn main() -> std::process::ExitCode {
    let config = match Config::from_env() {
        Ok(config) => config,
        Err(error) => {
            eprintln!("regional-observation-api: refusing to start: {error}");
            eprintln!(
                "regional-observation-api: required configuration: {}",
                REQUIRED_VARS.join(", ")
            );
            return std::process::ExitCode::FAILURE;
        }
    };
    let settings = aex_platform_telemetry::Settings::default();
    let telemetry = aex_platform_telemetry::Handle::install(&settings, None);
    telemetry.emit(
        aex_platform_telemetry::Record::event(
            aex_telemetry_schema::generated::EVENT_AEX_PROCESS_STARTED,
        )
        .with(
            aex_telemetry_schema::generated::AEX_PLANE,
            config.plane.clone(),
        )
        .with(
            aex_telemetry_schema::generated::AEX_REGION,
            config.region.as_str().to_owned(),
        ),
    );
    let outcome = run(config).await;
    if let aex_platform_telemetry::FlushOutcome::DeadlineExceeded { pending } =
        telemetry.flush(settings.flush_deadline)
    {
        eprintln!("regional-observation-api: telemetry flush left {pending} record(s) undelivered");
    }
    match outcome {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("regional-observation-api: stopped: {error}");
            std::process::ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use aex_observation_store_aws::composition::Capability;
    use aex_observation_store_aws::health::Probe;

    use super::{REQUIRED_PROBES, ROLE, RunError, compose};

    #[test]
    fn its_own_grant_and_a_complete_probe_set_start() {
        compose(ROLE.granted(), REQUIRED_PROBES).expect("the declared composition starts");
    }

    #[test]
    fn a_capability_outside_the_grant_refuses_to_start() {
        for denied in ROLE.denied() {
            let error = compose(&[denied], REQUIRED_PROBES).expect_err("refused");
            assert!(matches!(error, RunError::Capability { .. }), "{error:?}");
        }
    }

    #[test]
    fn the_query_role_can_never_link_a_write_or_delete_capability() {
        // A read route that could write would make the query surface a mutation
        // surface. The refusal is by name, not by convention.
        for forbidden in [
            Capability::WriteAdmission,
            Capability::WriteBodies,
            Capability::DeleteObservations,
            Capability::DeleteBodies,
            Capability::WriteExportObjects,
            Capability::LaunchExportTasks,
            Capability::DeliverUsage,
        ] {
            assert!(
                !ROLE.granted().contains(&forbidden),
                "`{}` must not be in the observation-api grant",
                forbidden.as_str()
            );
            let error = compose(&[forbidden], REQUIRED_PROBES).expect_err("refused");
            assert!(matches!(error, RunError::Capability { .. }), "{error:?}");
        }
    }

    #[test]
    fn an_unproven_probe_is_never_assumed() {
        let first = REQUIRED_PROBES.first().expect("a probe set is declared");
        let error = compose(ROLE.granted(), &[]).expect_err("refused");
        match error {
            RunError::NotReady { probe } => assert_eq!(probe, first.as_str()),
            other => panic!("expected a readiness failure, got {other:?}"),
        }
        // Proving a prefix is not proving the set: the cursor key ring and the
        // session authority are both load-bearing for routes this binary mounts.
        let error = compose(
            ROLE.granted(),
            &[Probe::ObservationTable, Probe::ObservationBucket],
        )
        .expect_err("refused");
        assert!(matches!(error, RunError::NotReady { .. }), "{error:?}");
    }
}
