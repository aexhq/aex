//! `observation-export-launcher` composition root (Rust Lambda ZIP).
//!
//! Exclusive responsibility: the durable export claim and the idempotent `ECS`
//! task launch.
//!
//! This deployable holds **no observation read permission at all** — its grant
//! is exactly `launch_export_tasks` — so a launcher bug cannot become a data
//! path. That is asserted at start-up against the shared role table, not assumed
//! from the `IAM` policy it happens to be deployed with.
//!
//! Its whole loop is a bounded due-scan over the sparse control index, one
//! conditional claim per export and one `RunTask`. An ambiguous launch outcome
//! is reconciled by identity rather than retried, because a duplicated export
//! task means a duplicated artifact and duplicated billed compute.

mod aws;
mod config;
mod launcher;
mod mount;

use std::sync::Arc;

use aex_observation_store_dynamodb::composition::{Capability, Role, assert_grant};
use aex_observation_store_dynamodb::health::{Probe, Readiness, readiness};
use aex_wire::types::Timestamp;
use lambda_runtime::{LambdaEvent, service_fn};

use crate::aws::{DynamoExportRows, EcsTaskLauncher};
use crate::config::{Config, ObservationExportLauncherConfigError, REQUIRED_VARS};
use crate::launcher::{ExportRows, LaunchSettings, Launcher, TaskLauncher};
use crate::mount::AppState;

/// The capability grant this deployable is allowed to hold.
pub const ROLE: Role = Role::ExportLauncher;

/// The dependencies this deployable proves before it reports ready.
pub const REQUIRED_PROBES: &[Probe] = &[Probe::ExportCluster];

/// Environment variable naming the release digest both health endpoints report.
///
/// Optional, and the only optional variable this deployable reads: it identifies
/// a build, not a resource, so a missing one cannot bind the process anywhere.
pub const RELEASE_DIGEST_VAR: &str = "AEX_RELEASE_DIGEST";

/// Why `observation-export-launcher` stopped.
#[derive(Debug, thiserror::Error)]
pub enum ObservationExportLauncherRunError {
    /// Start-up configuration was rejected.
    #[error(transparent)]
    Config(#[from] ObservationExportLauncherConfigError),
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
/// Returns [`ObservationExportLauncherRunError::Capability`] when the process holds a capability its role
/// must not, and [`ObservationExportLauncherRunError::NotReady`] when a declared probe has not passed. A
/// probe that has not passed is never assumed.
pub fn compose(observed: &[Capability], passed: &[Probe]) -> Result<(), ObservationExportLauncherRunError> {
    assert_grant(ROLE, observed).map_err(|violation| ObservationExportLauncherRunError::Capability {
        capability: violation.capability.as_str(),
    })?;
    match readiness(REQUIRED_PROBES, passed) {
        Readiness::Ready => Ok(()),
        Readiness::NotReady { outstanding } => Err(ObservationExportLauncherRunError::NotReady {
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
pub async fn run(config: Config) -> Result<(), ObservationExportLauncherRunError> {
    let sdk = aws_config::from_env()
        .region(aws_config::Region::new(config.region.as_str()))
        .load()
        .await;
    let rows = DynamoExportRows::new(
        aws_sdk_dynamodb::Client::new(&sdk),
        config.observation_table.clone(),
    );
    let tasks = EcsTaskLauncher::new(
        aws_sdk_ecs::Client::new(&sdk),
        config.export_cluster.clone(),
    );

    rows.probe().await.map_err(|error| ObservationExportLauncherRunError::Probe {
        reason: error.to_string(),
    })?;
    tasks
        .probe(&config.export_task_definition)
        .await
        .map_err(|error| ObservationExportLauncherRunError::Probe {
            reason: error.to_string(),
        })?;
    let passed = [Probe::ExportCluster];
    compose(ROLE.granted(), &passed)?;

    let settings = LaunchSettings {
        cluster: config.export_cluster,
        task_definition: config.export_task_definition,
        container: config.export_container,
        subnets: config.export_subnets,
        security_groups: config.export_security_groups,
        shards: config.launch_shards,
        max_concurrent: config.max_concurrent,
        lease: config.lease,
    };
    let launcher = Arc::new(Launcher::new(rows, tasks, settings));
    let state = Arc::new(AppState {
        ready: true,
        release_digest: release_digest(),
    });

    lambda_runtime::run(service_fn(move |event| {
        let launcher = Arc::clone(&launcher);
        let state = Arc::clone(&state);
        async move { invoke(launcher.as_ref(), state.as_ref(), event).await }
    }))
    .await
    .map_err(|error| ObservationExportLauncherRunError::Runtime {
        reason: error.to_string(),
    })
}

/// Answers one invocation: a health probe, or one bounded launch sweep.
async fn invoke<R, T>(
    launcher: &Launcher<R, T>,
    state: &AppState,
    event: LambdaEvent<serde_json::Value>,
) -> Result<serde_json::Value, lambda_runtime::Error>
where
    R: ExportRows,
    T: TaskLauncher,
{
    if let Some(path) = mount::probe_path(&event.payload) {
        let reply = mount::health(state, path).ok_or_else(|| {
            lambda_runtime::Error::from(format!("`{path}` is not a path this deployable answers"))
        })?;
        return Ok(reply.to_response(state));
    }
    let now = Timestamp::from_datetime_trunc_ms(time::OffsetDateTime::now_utc())
        .map_err(|error| lambda_runtime::Error::from(error.to_string()))?;
    let report = launcher.sweep(&event.context.request_id, now).await?;
    tracing::info!(
        scanned = report.scanned,
        claimed = report.claimed,
        launched = report.launched,
        unresolved = report.unresolved,
        "export launch sweep complete"
    );
    serde_json::to_value(report).map_err(|error| lambda_runtime::Error::from(error.to_string()))
}

/// The release digest both health endpoints report.
fn release_digest() -> String {
    std::env::var(RELEASE_DIGEST_VAR).unwrap_or_else(|_| "unreleased".to_owned())
}

#[tokio::main]
async fn main() -> std::process::ExitCode {
    let config = match Config::from_env() {
        Ok(config) => config,
        Err(error) => {
            eprintln!("observation-export-launcher: refusing to start: {error}");
            eprintln!(
                "observation-export-launcher: required configuration: {}",
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
        eprintln!(
            "observation-export-launcher: telemetry flush left {pending} record(s) undelivered"
        );
    }
    match outcome {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("observation-export-launcher: stopped: {error}");
            std::process::ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use aex_observation_store_dynamodb::composition::Capability;
    use aex_observation_store_dynamodb::health::Probe;

    use super::{REQUIRED_PROBES, ROLE, ObservationExportLauncherRunError, compose};

    #[test]
    fn its_own_grant_and_a_complete_probe_set_start() {
        compose(ROLE.granted(), REQUIRED_PROBES).expect("the declared composition starts");
        assert_eq!(ROLE.granted(), &[Capability::LaunchExportTasks]);
        assert_eq!(REQUIRED_PROBES, &[Probe::ExportCluster]);
    }

    #[test]
    fn a_capability_outside_the_grant_refuses_to_start() {
        for denied in ROLE.denied() {
            let error = compose(&[denied], REQUIRED_PROBES).expect_err("refused");
            assert!(matches!(error, ObservationExportLauncherRunError::Capability { .. }), "{error:?}");
        }
    }

    #[test]
    fn the_launcher_refuses_every_capability_outside_its_exact_grant() {
        // The plan's claim about this deployable is negative: it can read no
        // observation, write no authority row and delete nothing. Deriving the
        // refusals from the shared inventory makes adding a capability fail
        // closed here until the role deliberately grants it.
        for forbidden in ROLE.denied() {
            assert!(
                !ROLE.granted().contains(&forbidden),
                "`{}` must not be in the launcher grant",
                forbidden.as_str()
            );
            let error = compose(&[forbidden], REQUIRED_PROBES).expect_err("refused");
            match error {
                ObservationExportLauncherRunError::Capability { capability } => {
                    assert_eq!(capability, forbidden.as_str());
                }
                other => panic!("expected a capability failure, got {other:?}"),
            }
        }
    }

    #[test]
    fn an_unproven_probe_is_never_assumed() {
        let first = REQUIRED_PROBES.first().expect("a probe set is declared");
        let error = compose(ROLE.granted(), &[]).expect_err("refused");
        match error {
            ObservationExportLauncherRunError::NotReady { probe } => assert_eq!(probe, first.as_str()),
            other => panic!("expected a readiness failure, got {other:?}"),
        }
    }

    #[test]
    fn a_grant_carrying_one_forbidden_capability_among_allowed_ones_still_refuses() {
        let observed = [Capability::LaunchExportTasks, Capability::ReadAuthority];
        let error = compose(&observed, REQUIRED_PROBES).expect_err("refused");
        assert!(matches!(error, ObservationExportLauncherRunError::Capability { .. }), "{error:?}");
    }
}
