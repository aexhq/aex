//! `observation-export-task` composition root (one-shot Rust OCI task).
//!
//! Exclusive responsibility: generate **exactly one** bounded-memory multipart
//! export artifact and exit.
//!
//! This is a Fargate task, not a Lambda: it runs once per admitted export, holds
//! a lease while it generates, and stops. Its memory profile is flat because
//! every buffer is reserved before the producing loop starts (see
//! [`crate::budget`]), so a multi-GB export costs the same resident bytes as a
//! one-page export.
//!
//! Two outcomes are both success. A published artifact exits zero, and so does a
//! **superseded** export: a cancel, a deletion or a lease takeover is
//! authoritative, and reporting it as a failure would produce spurious alarms
//! and retry storms on a correct outcome.
//!
//! This role holds no delete capability anywhere, which the start-up composition
//! assertion refuses to run without.

mod aws;
mod budget;
mod config;
mod health;
mod task;

use std::sync::Arc;

use aex_observation_store_dynamodb::composition::{Capability, Role, assert_grant};
use aex_observation_store_dynamodb::health::{Probe, Readiness, readiness};
use aex_otlp_admission::MemoryBudget;
use aex_wire::types::Timestamp;

use crate::config::{Config, ObservationExportTaskConfigError, REQUIRED_VARS};
use crate::health::{HealthError, HealthServer, HealthState};
use crate::task::{ExportOutcome, ExportTask, TaskError, TaskSettings, object_prefix};

/// The capability grant this deployable is allowed to hold.
///
/// The export task reads the authority and the bodies and writes export objects.
/// It holds **no** delete of any kind, so composing it with one refuses to start
/// rather than running with authority it never declared.
pub const ROLE: Role = Role::ExportTask;

/// The dependencies this deployable proves before it reports ready.
pub const REQUIRED_PROBES: &[Probe] = &[Probe::ObservationTable, Probe::ObservationBucket];

/// Why `observation-export-task` stopped.
#[derive(Debug, thiserror::Error)]
pub enum ObservationExportTaskRunError {
    /// Start-up configuration was rejected.
    #[error(transparent)]
    Config(#[from] ObservationExportTaskConfigError),
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
    /// The health listener could not be bound.
    #[error(transparent)]
    Health(#[from] HealthError),
    /// A start-up probe failed against a real resource.
    #[error("a start-up probe failed: {reason}")]
    Probe {
        /// What failed.
        reason: String,
    },
    /// The clock is outside the wire range.
    #[error("the process clock is outside the representable wire range")]
    Clock,
    /// The export itself failed.
    #[error(transparent)]
    Export(#[from] TaskError),
}

/// Asserts the observed capability grant and the readiness probe set.
///
/// # Errors
///
/// Returns [`ObservationExportTaskRunError::Capability`] when the process holds a capability its role
/// must not, and [`ObservationExportTaskRunError::NotReady`] when a declared probe has not passed. A
/// probe that has not passed is never assumed.
pub fn compose(observed: &[Capability], passed: &[Probe]) -> Result<(), ObservationExportTaskRunError> {
    assert_grant(ROLE, observed).map_err(|violation| ObservationExportTaskRunError::Capability {
        capability: violation.capability.as_str(),
    })?;
    match readiness(REQUIRED_PROBES, passed) {
        Readiness::Ready => Ok(()),
        Readiness::NotReady { outstanding } => Err(ObservationExportTaskRunError::NotReady {
            probe: outstanding.as_str(),
        }),
    }
}

/// Builds every adapter, proves every probe and generates one export.
///
/// # Errors
///
/// Returns the typed failure of the first start-up stage that refused, or the
/// export's own failure. Nothing is read before every declared probe has
/// actually passed.
pub async fn run(config: Config) -> Result<(), ObservationExportTaskRunError> {
    let aws = aws_config::from_env()
        .region(aws_config::Region::new(config.region.as_str()))
        .load()
        .await;
    let dynamodb = aws_sdk_dynamodb::Client::new(&aws);
    let s3 = aws_sdk_s3::Client::new(&aws);

    let authority = aws::DynamoExportAuthority::new(dynamodb, s3.clone(), &config);
    let objects = aws::S3ExportObjects::new(s3, config.observation_bucket.clone());

    let mut passed = Vec::new();
    authority.probe().await.map_err(|error| ObservationExportTaskRunError::Probe {
        reason: error.to_string(),
    })?;
    passed.push(Probe::ObservationTable);
    objects.probe().await.map_err(|error| ObservationExportTaskRunError::Probe {
        reason: error.to_string(),
    })?;
    passed.push(Probe::ObservationBucket);
    compose(ROLE.granted(), &passed)?;

    let health = serve_health(&config, passed).await?;
    let outcome = export(config, authority, objects).await;
    if let Some(server) = health {
        server.shutdown().await;
    }
    let outcome = outcome?;
    report(&outcome);
    // Both outcomes exit zero: the distinction is recorded, never signalled.
    tracing::debug!(
        published = outcome.is_published(),
        superseded_by = outcome.superseded_by().unwrap_or("none"),
        "the export task finished"
    );
    Ok(())
}

/// Binds the health listener, when a port is configured.
async fn serve_health(
    config: &Config,
    passed: Vec<Probe>,
) -> Result<Option<HealthServer>, ObservationExportTaskRunError> {
    let Some(port) = config.health_port else {
        // The Fargate row declares `port = 0`, so nothing is bound; the same
        // bodies stay available through `health_body` and `readiness_body`.
        return Ok(None);
    };
    let state = Arc::new(HealthState {
        release_digest: release_digest(),
        required: REQUIRED_PROBES,
        passed,
    });
    let server = health::serve(port, state).await?;
    tracing::info!(addr = %server.local_addr(), "the health listener is bound");
    Ok(Some(server))
}

/// Runs the one export this task exists for.
async fn export(
    config: Config,
    authority: aws::DynamoExportAuthority,
    objects: aws::S3ExportObjects,
) -> Result<ExportOutcome, ObservationExportTaskRunError> {
    let settings = TaskSettings {
        plan: config.memory_plan(),
        budget: MemoryBudget::new(config.memory_budget_bytes),
        page_limit: config.page_limit,
        part_bytes: config.part_bytes,
        object_prefix: object_prefix(config.workspace_id, config.export_id).into(),
    };
    let now = Timestamp::from_datetime_trunc_ms(time::OffsetDateTime::now_utc())
        .map_err(|_| ObservationExportTaskRunError::Clock)?;
    Ok(ExportTask::new(authority, objects, settings)
        .run(now)
        .await?)
}

/// Records what the one export did.
fn report(outcome: &ExportOutcome) {
    match outcome {
        ExportOutcome::Published {
            object_key,
            object_bytes,
            manifest_hash,
            parts,
            records,
        } => tracing::info!(
            object_key = %object_key,
            object_bytes,
            manifest_hash = %manifest_hash,
            parts,
            records,
            "the export is ready"
        ),
        ExportOutcome::Superseded { reason } => {
            tracing::info!(reason, "the export was superseded; exiting zero");
        }
    }
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
            eprintln!("observation-export-task: refusing to start: {error}");
            eprintln!(
                "observation-export-task: required configuration: {}",
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
        eprintln!("observation-export-task: telemetry flush left {pending} record(s) undelivered");
    }
    match outcome {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("observation-export-task: stopped: {error}");
            std::process::ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use aex_observation_store_dynamodb::composition::Capability;
    use aex_observation_store_dynamodb::health::Probe;

    use super::{REQUIRED_PROBES, ROLE, ObservationExportTaskRunError, compose, report};
    use crate::task::ExportOutcome;

    #[test]
    fn its_own_grant_and_a_complete_probe_set_start() {
        compose(ROLE.granted(), REQUIRED_PROBES).expect("the declared composition starts");
        assert_eq!(
            ROLE.granted(),
            &[
                Capability::ReadAuthority,
                Capability::ReadBodies,
                Capability::WriteExportObjects
            ]
        );
    }

    #[test]
    fn a_capability_outside_the_grant_refuses_to_start() {
        for denied in ROLE.denied() {
            let error = compose(&[denied], REQUIRED_PROBES).expect_err("refused");
            assert!(matches!(error, ObservationExportTaskRunError::Capability { .. }), "{error:?}");
        }
    }

    #[test]
    fn the_export_task_can_never_link_a_delete_capability() {
        // The task writes an artifact and reads observations. A delete here
        // would turn an export role into a data-destroying one, so composing
        // with either delete refuses by name rather than by convention.
        for forbidden in [Capability::DeleteBodies, Capability::DeleteObservations] {
            assert!(
                !ROLE.granted().contains(&forbidden),
                "`{}` must not be in the export-task grant",
                forbidden.as_str()
            );
            let error = compose(&[forbidden], REQUIRED_PROBES).expect_err("refused");
            match error {
                ObservationExportTaskRunError::Capability { capability } => {
                    assert_eq!(capability, forbidden.as_str());
                }
                other => panic!("expected a capability violation, got {other:?}"),
            }
        }
    }

    #[test]
    fn the_export_task_can_never_link_a_write_or_launch_capability_either() {
        for forbidden in [
            Capability::WriteAdmission,
            Capability::WriteBodies,
            Capability::LaunchExportTasks,
            Capability::DeliverUsage,
        ] {
            assert!(!ROLE.granted().contains(&forbidden));
            assert!(compose(&[forbidden], REQUIRED_PROBES).is_err());
        }
    }

    #[test]
    fn an_unproven_probe_is_never_assumed() {
        let first = REQUIRED_PROBES.first().expect("a probe set is declared");
        let error = compose(ROLE.granted(), &[]).expect_err("refused");
        match error {
            ObservationExportTaskRunError::NotReady { probe } => assert_eq!(probe, first.as_str()),
            other => panic!("expected a readiness failure, got {other:?}"),
        }
        // Proving a prefix is not proving the set.
        let error = compose(ROLE.granted(), &[Probe::ObservationTable]).expect_err("refused");
        assert!(matches!(error, ObservationExportTaskRunError::NotReady { .. }), "{error:?}");
    }

    #[test]
    fn both_outcomes_are_success_and_both_are_reportable() {
        let superseded = ExportOutcome::Superseded {
            reason: "a cancel won",
        };
        report(&superseded);
        assert!(!superseded.is_published());
        assert_eq!(superseded.superseded_by(), Some("a cancel won"));

        let published = ExportOutcome::Published {
            object_key: "exports/wsp/exp.jsonl".into(),
            object_bytes: 42,
            manifest_hash: "a".repeat(64).into(),
            parts: 2,
            records: 7,
        };
        report(&published);
        assert!(published.is_published());
        assert_eq!(published.superseded_by(), None);
    }
}
