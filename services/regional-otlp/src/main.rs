//! `regional-otlp` composition root (Rust Lambda ZIP).
//!
//! Exclusive responsibility: bounded OTLP admission and the durable observation
//! receipt.
//!
//! This deployable runs on **dedicated memory and reserved concurrency**, which
//! is the whole point of splitting it from the query role: decode memory is
//! bounded by `reserved concurrency × decoded ceiling`, so decompression can
//! never starve query, export or sockets. Overload is an explicit retryable
//! failure raised *before* unbounded allocation, never an OOM kill.

mod admission;
mod authority;
mod config;
mod counters;
mod mount;
mod staging;

use std::sync::Arc;

use aex_observation_store_dynamodb::composition::{Capability, Role, assert_grant};
use aex_observation_store_dynamodb::health::{Probe, Readiness, readiness};
use aex_otlp_admission::MemoryBudget;
use aex_wire::dispatch::RequestLimits;

use crate::admission::OtlpService;
use crate::authority::AdmissionAuthority;
use crate::config::{Config, REQUIRED_VARS, RegionalOtlpConfigError};
use crate::counters::AdmissionTelemetry;
use crate::mount::{AUDIENCE, AppState};

/// The capability grant this deployable is allowed to hold.
pub const ROLE: Role = Role::Otlp;

/// The dependencies this deployable proves before it reports ready.
pub const REQUIRED_PROBES: &[Probe] = &[
    Probe::ObservationTable,
    Probe::ObservationBucket,
    Probe::IngressGate,
];

/// Why `regional-otlp` stopped.
#[derive(Debug, thiserror::Error)]
pub enum RegionalOtlpRunError {
    /// Start-up configuration was rejected.
    #[error(transparent)]
    Config(#[from] RegionalOtlpConfigError),
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
/// Returns [`RegionalOtlpRunError::Capability`] when the process holds a capability its role
/// must not, and [`RegionalOtlpRunError::NotReady`] when a declared probe has not passed. A
/// probe that has not passed is never assumed.
pub fn compose(observed: &[Capability], passed: &[Probe]) -> Result<(), RegionalOtlpRunError> {
    assert_grant(ROLE, observed).map_err(|violation| RegionalOtlpRunError::Capability {
        capability: violation.capability.as_str(),
    })?;
    match readiness(REQUIRED_PROBES, passed) {
        Readiness::Ready => Ok(()),
        Readiness::NotReady { outstanding } => Err(RegionalOtlpRunError::NotReady {
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
pub async fn run(
    config: Config,
    telemetry: aex_platform_telemetry::Handle,
) -> Result<(), RegionalOtlpRunError> {
    let aws = aws_config::from_env()
        .region(aws_config::Region::new(config.region.as_str()))
        .load()
        .await;
    let dynamodb = aws_sdk_dynamodb::Client::new(&aws);
    let s3 = aws_sdk_s3::Client::new(&aws);

    let authority = AdmissionAuthority::new(
        dynamodb.clone(),
        s3,
        config.observation_table.clone(),
        config.observation_bucket.clone(),
        config.region,
    );

    let mut passed = Vec::new();
    authority
        .probe()
        .await
        .map_err(|error| RegionalOtlpRunError::Probe {
            reason: error.to_string(),
        })?;
    passed.push(Probe::ObservationTable);
    passed.push(Probe::ObservationBucket);

    authority
        .ingress_gate()
        .await
        .map_err(|error| RegionalOtlpRunError::Probe {
            reason: error.to_string(),
        })?;
    passed.push(Probe::IngressGate);

    compose(ROLE.granted(), &passed)?;

    // The one shared edge. There is no exchange left to compose it over: a
    // presented key is checked against the verifier this region already reads
    // on every request, so nothing on this path leaves the region.
    let parameters = aex_regional_http::authz::ParameterStore::new(aws_sdk_ssm::Client::new(&aws));
    let peppers = parameters
        .pepper_ring(&config.credential_pepper_ref)
        .await
        .map_err(|error| RegionalOtlpRunError::Edge {
            reason: error.to_string(),
        })?;
    let projection = aex_session_dynamodb::projection::ProjectionReader::new(
        dynamodb.clone(),
        config.authz_projection_table.clone(),
    );
    let edge = aex_regional_http::edge::RegionalEdge::new(
        peppers,
        aex_regional_http::authz::RegionalProjection::new(projection, config.region),
        aex_regional_http::edge::SystemClock,
        aex_regional_http::edge::EdgeBinding {
            audience: AUDIENCE,
            region: config.region,
        },
    );

    let service = OtlpService::new(
        authority,
        config.limits,
        MemoryBudget::new(config.memory_budget_bytes),
        config.reserve_wait,
        AdmissionTelemetry::new(telemetry, config.plane.as_str(), config.region.as_str()),
    );
    let state = Arc::new(AppState {
        edge: Arc::new(edge),
        service: Arc::new(service),
        limits: RequestLimits {
            max_json_body_bytes: RequestLimits::DEFAULT_JSON_BODY_BYTES,
            max_otlp_body_bytes: config.limits.encoded_max,
        },
        ready: true,
        release_digest: release_digest(),
    });
    lambda_http::run(mount::router(state))
        .await
        .map_err(|error| RegionalOtlpRunError::Runtime {
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
            eprintln!("regional-otlp: refusing to start: {error}");
            eprintln!(
                "regional-otlp: required configuration: {}",
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
            config.plane.as_str().to_owned(),
        )
        .with(
            aex_telemetry_schema::generated::AEX_REGION,
            config.region.as_str().to_owned(),
        ),
    );
    let outcome = run(config, telemetry.clone()).await;
    if let aex_platform_telemetry::FlushOutcome::DeadlineExceeded { pending } =
        telemetry.flush(settings.flush_deadline)
    {
        eprintln!("regional-otlp: telemetry flush left {pending} record(s) undelivered");
    }
    match outcome {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("regional-otlp: stopped: {error}");
            std::process::ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use aex_observation_store_dynamodb::composition::Capability;
    use aex_observation_store_dynamodb::health::Probe;

    use super::{REQUIRED_PROBES, ROLE, RegionalOtlpRunError, compose};

    #[test]
    fn its_own_grant_and_a_complete_probe_set_start() {
        compose(ROLE.granted(), REQUIRED_PROBES).expect("the declared composition starts");
    }

    #[test]
    fn a_capability_outside_the_grant_refuses_to_start() {
        for denied in ROLE.denied() {
            let error = compose(&[denied], REQUIRED_PROBES).expect_err("refused");
            assert!(
                matches!(error, RegionalOtlpRunError::Capability { .. }),
                "{error:?}"
            );
        }
    }

    #[test]
    fn the_admission_edge_can_never_link_a_read_or_delete_capability() {
        // A capability an admission role must not hold is refused by name, not
        // by convention: a read capability here would turn the ingest edge into
        // a data path it has no business being.
        for forbidden in [
            Capability::ReadBodies,
            Capability::ReadAuthority,
            Capability::DeleteBodies,
            Capability::DeleteObservations,
            Capability::WriteExportObjects,
            Capability::LaunchExportTasks,
        ] {
            assert!(
                !ROLE.granted().contains(&forbidden),
                "`{}` must not be in the OTLP grant",
                forbidden.as_str()
            );
            let error = compose(&[forbidden], REQUIRED_PROBES).expect_err("refused");
            assert!(
                matches!(error, RegionalOtlpRunError::Capability { .. }),
                "{error:?}"
            );
        }
    }

    #[test]
    fn an_unproven_probe_is_never_assumed() {
        let first = REQUIRED_PROBES.first().expect("a probe set is declared");
        let error = compose(ROLE.granted(), &[]).expect_err("refused");
        match error {
            RegionalOtlpRunError::NotReady { probe } => assert_eq!(probe, first.as_str()),
            other => panic!("expected a readiness failure, got {other:?}"),
        }
        // Proving a prefix is not proving the set.
        let error = compose(ROLE.granted(), &[Probe::ObservationTable]).expect_err("refused");
        assert!(
            matches!(error, RegionalOtlpRunError::NotReady { .. }),
            "{error:?}"
        );
    }
}
