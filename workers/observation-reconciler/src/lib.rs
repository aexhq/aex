//! `observation-reconciler`: the scheduled duty runner of the observation
//! authority.
//!
//! Exclusive responsibility: spool repair, batch expiry, export reaping, scope
//! deletion and its proof, index verification, the regional ingress gate and
//! series reclamation — **one duty per deployment**.
//!
//! The duty is configuration, and the capability grant follows from it: the
//! `deletion.execute` deployment runs as
//! [`Role::ReconcilerDeletion`](aex_observation_store_dynamodb::composition::Role::ReconcilerDeletion)
//! and is the only role in the whole stream that may delete an object; every
//! other duty runs as
//! [`Role::Reconciler`](aex_observation_store_dynamodb::composition::Role::Reconciler)
//! and its start-up assertion refuses `delete_bodies` by name. That is what
//! makes "only the deletion deployment may delete" structural rather than a
//! deployment convention.
//!
//! `export.launch` is refused outright: it belongs to
//! `observation-export-launcher`, which holds `ecs:RunTask` and no observation
//! read permission at all.

pub mod config;
pub mod duty;
pub mod handler;
pub mod health;

use aex_observation_domain::keys::ControlDomain;
use aex_observation_store_dynamodb::composition::{Capability, Role, assert_grant};
use aex_observation_store_dynamodb::health::{Probe, Readiness, readiness};
use aex_wire::types::Timestamp;

use crate::config::{Config, ObservationReconcilerConfigError, REQUIRED_VARS};
use crate::duty::{DutyEngine, DutyError, DutySettings};
use crate::handler::Handler;

pub use crate::health::REQUIRED_PROBES;

/// The role every duty deployment other than `deletion.execute` holds.
pub const ROLE: Role = Role::Reconciler;

/// The role the `deletion.execute` deployment holds.
///
/// It is the only role in the stream that carries
/// [`Capability::DeleteBodies`], which is why the duty selects the role rather
/// than the deployment declaring one.
pub const DELETION_ROLE: Role = Role::ReconcilerDeletion;

/// The environment variable carrying the release digest both probes report.
pub const RELEASE_DIGEST_VAR: &str = "AEX_RELEASE_DIGEST";

/// The digest reported when the deployment carries none.
pub const UNRELEASED: &str = "unreleased";

/// The role one duty deployment runs under.
///
/// The `deletion.execute` deployment is the only one that may delete an object;
/// every other duty gets the read-and-repair grant, and its start-up assertion
/// refuses the delete capability by name.
#[must_use]
pub const fn role_for(duty: ControlDomain) -> Role {
    match duty {
        ControlDomain::DeletionExecute => DELETION_ROLE,
        ControlDomain::SpoolRepair
        | ControlDomain::BatchExpire
        | ControlDomain::ExportLaunch
        | ControlDomain::ExportReap
        | ControlDomain::DeletionVerify
        | ControlDomain::IndexVerify
        | ControlDomain::GateEvaluate
        | ControlDomain::SeriesReclaim => ROLE,
    }
}

/// Why `observation-reconciler` stopped.
#[derive(Debug, thiserror::Error)]
pub enum ObservationReconcilerRunError {
    /// Start-up configuration was rejected.
    #[error(transparent)]
    Config(#[from] ObservationReconcilerConfigError),
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

impl From<DutyError> for ObservationReconcilerRunError {
    fn from(error: DutyError) -> Self {
        Self::Probe {
            reason: error.to_string(),
        }
    }
}

/// Asserts the observed capability grant and the readiness probe set.
///
/// # Errors
///
/// Returns [`ObservationReconcilerRunError::Capability`] when the process holds a capability its role
/// must not, and [`ObservationReconcilerRunError::NotReady`] when a declared probe has not passed. A
/// probe that has not passed is never assumed.
pub fn compose(
    role: Role,
    observed: &[Capability],
    passed: &[Probe],
) -> Result<(), ObservationReconcilerRunError> {
    assert_grant(role, observed).map_err(|violation| {
        ObservationReconcilerRunError::Capability {
            capability: violation.capability.as_str(),
        }
    })?;
    match readiness(REQUIRED_PROBES, passed) {
        Readiness::Ready => Ok(()),
        Readiness::NotReady { outstanding } => Err(ObservationReconcilerRunError::NotReady {
            probe: outstanding.as_str(),
        }),
    }
}

/// The engine settings one validated configuration implies.
#[must_use]
pub fn settings_for(config: &Config) -> DutySettings {
    DutySettings {
        table: config.observation_table.clone(),
        session_table: config.session_table.clone(),
        bucket: config.observation_bucket.clone(),
        usage_queue_url: config.usage_queue_url.clone(),
        region: config.region,
        duty: config.duty,
        page: config.page,
        shards: config.shards,
        max_attempts: config.max_attempts,
    }
}

/// Builds every adapter, proves every probe and serves until the runtime stops.
///
/// # Errors
///
/// Returns the typed failure of the first start-up stage that refused. Nothing
/// is served before every declared probe has actually passed, and the capability
/// assertion runs against the role the configured duty selects.
pub async fn run(config: Config) -> Result<(), ObservationReconcilerRunError> {
    let aws = aws_config::from_env()
        .region(aws_config::Region::new(config.region.as_str()))
        .load()
        .await;
    let engine = DutyEngine::new(
        aws_sdk_dynamodb::Client::new(&aws),
        aws_sdk_s3::Client::new(&aws),
        aws_sdk_sqs::Client::new(&aws),
        settings_for(&config),
    );

    engine.probe().await?;
    let passed = vec![Probe::ObservationTable, Probe::ObservationBucket];

    let role = role_for(config.duty);
    compose(role, role.granted(), &passed)?;
    tracing::info!(
        duty = config.duty.as_str(),
        role = ?role,
        shards = config.shards,
        page = config.page,
        "the duty deployment composed its grant"
    );

    let handler = std::sync::Arc::new(Handler::new(engine, release_digest(), passed));
    lambda_runtime::run(lambda_runtime::service_fn(
        move |event: lambda_runtime::LambdaEvent<serde_json::Value>| {
            let handler = std::sync::Arc::clone(&handler);
            async move {
                let now = Timestamp::from_datetime_trunc_ms(time::OffsetDateTime::now_utc())
                    .map_err(|error| lambda_runtime::Error::from(error.to_string()))?;
                handler
                    .respond(&event.payload, now)
                    .await
                    .map_err(|error| lambda_runtime::Error::from(error.to_string()))
            }
        },
    ))
    .await
    .map_err(|error| ObservationReconcilerRunError::Runtime {
        reason: error.to_string(),
    })
}

/// The release digest both health endpoints report.
#[must_use]
pub fn release_digest() -> String {
    std::env::var(RELEASE_DIGEST_VAR).unwrap_or_else(|_| UNRELEASED.to_owned())
}

/// The refusal one unusable configuration prints, in one place.
#[must_use]
pub fn refusal(error: &ObservationReconcilerConfigError) -> String {
    format!(
        "observation-reconciler: refusing to start: {error}\n\
         observation-reconciler: required configuration: {}",
        REQUIRED_VARS.join(", ")
    )
}

#[cfg(test)]
mod tests {
    use aex_observation_domain::keys::ControlDomain;
    use aex_observation_store_dynamodb::composition::{Capability, Role};
    use aex_observation_store_dynamodb::health::Probe;

    use super::{
        DELETION_ROLE, ObservationReconcilerRunError, REQUIRED_PROBES, ROLE, UNRELEASED, compose,
        refusal, role_for,
    };
    use crate::config::{DUTY_VAR, ObservationReconcilerConfigError};

    #[test]
    fn the_deletion_duty_is_the_only_one_that_selects_the_deleting_role() {
        let mut deleting = 0;
        for duty in ControlDomain::ALL {
            if role_for(*duty) == DELETION_ROLE {
                deleting += 1;
                assert_eq!(*duty, ControlDomain::DeletionExecute);
            } else {
                assert_eq!(role_for(*duty), ROLE);
            }
        }
        assert_eq!(deleting, 1);
        assert_ne!(ROLE, DELETION_ROLE);
    }

    #[test]
    fn a_non_deleting_duty_cannot_link_the_delete_capability() {
        for duty in ControlDomain::ALL {
            let role = role_for(*duty);
            if role == DELETION_ROLE {
                continue;
            }
            let error = compose(role, &[Capability::DeleteBodies], REQUIRED_PROBES)
                .expect_err("the delete grant is outside this role");
            match error {
                ObservationReconcilerRunError::Capability { capability } => {
                    assert_eq!(capability, Capability::DeleteBodies.as_str());
                }
                other => panic!("expected a capability refusal, got {other:?}"),
            }
        }
    }

    #[test]
    fn the_deleting_role_also_refuses_everything_outside_its_own_grant() {
        for denied in Role::ReconcilerDeletion.denied() {
            let error =
                compose(DELETION_ROLE, &[denied], REQUIRED_PROBES).expect_err("outside the grant");
            assert!(
                matches!(error, ObservationReconcilerRunError::Capability { .. }),
                "{error:?}"
            );
        }
        compose(DELETION_ROLE, DELETION_ROLE.granted(), REQUIRED_PROBES)
            .expect("its own grant starts");
    }

    #[test]
    fn no_duty_role_holds_an_export_capability() {
        for duty in ControlDomain::ALL {
            let role = role_for(*duty);
            assert!(!role.holds(Capability::LaunchExportTasks), "{duty:?}");
            assert!(!role.holds(Capability::WriteExportObjects), "{duty:?}");
        }
    }

    #[test]
    fn readiness_is_proven_and_never_assumed() {
        assert_eq!(
            REQUIRED_PROBES,
            &[Probe::ObservationTable, Probe::ObservationBucket]
        );
        let error = compose(ROLE, ROLE.granted(), &[]).expect_err("nothing is proven");
        assert!(
            matches!(error, ObservationReconcilerRunError::NotReady { .. }),
            "{error:?}"
        );
    }

    #[test]
    fn the_refusal_lists_every_required_variable() {
        let text = refusal(&ObservationReconcilerConfigError::Missing { name: DUTY_VAR });
        for name in super::REQUIRED_VARS {
            assert!(text.contains(name), "{name} is absent from: {text}");
        }
        assert!(text.contains("refusing to start"));
        assert_eq!(UNRELEASED, "unreleased");
    }
}
