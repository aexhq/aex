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
mod mount;
mod staging;

use std::sync::Arc;

use aex_observation_store_aws::composition::{Capability, Role, assert_grant};
use aex_observation_store_aws::health::{Probe, Readiness, readiness};
use aex_otlp_admission::MemoryBudget;
use aex_wire::dispatch::RequestLimits;

use crate::admission::{CustodyManifests, OtlpService};
use crate::authority::AdmissionAuthority;
use crate::config::{Config, ConfigError, REQUIRED_VARS};
use crate::mount::{AUDIENCE, AppState};

/// The capability grant this deployable is allowed to hold.
pub const ROLE: Role = Role::Otlp;

/// The dependencies this deployable proves before it reports ready.
pub const REQUIRED_PROBES: &[Probe] = &[
    Probe::ObservationTable,
    Probe::ObservationBucket,
    Probe::RedactionKey,
    Probe::IngressGate,
];

/// Why `regional-otlp` stopped.
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
    let secrets = aws_sdk_secretsmanager::Client::new(&aws);

    let authority = AdmissionAuthority::new(
        dynamodb.clone(),
        s3,
        config.observation_table.clone(),
        config.observation_bucket.clone(),
        config.region,
    );

    let mut passed = Vec::new();
    authority.probe().await.map_err(|error| RunError::Probe {
        reason: error.to_string(),
    })?;
    passed.push(Probe::ObservationTable);
    passed.push(Probe::ObservationBucket);

    let redaction_key = resolve_redaction_key(&secrets, &config.redaction_key_ref).await?;
    passed.push(Probe::RedactionKey);

    authority
        .ingress_gate()
        .await
        .map_err(|error| RunError::Probe {
            reason: error.to_string(),
        })?;
    passed.push(Probe::IngressGate);

    compose(ROLE.granted(), &passed)?;

    // The one shared edge, over the one shared exchange. `central-authz` is
    // IAM-invoked rather than routed, so this is a direct invoke and the caller's
    // execution role is the authentication; the credential is named by
    // `(keyId, presentedDigest)` and never sent.
    let parameters = aex_regional_http::authz::ParameterStore::new(aws_sdk_ssm::Client::new(&aws));
    let anchors = parameters
        .trust_anchors(&config.authz_verify_keys_param)
        .await
        .map_err(|error| RunError::Edge {
            reason: error.to_string(),
        })?;
    let projection = aex_session_dynamodb::projection::ProjectionReader::new(
        dynamodb.clone(),
        config.authz_projection_table.clone(),
    );
    let edge = aex_regional_http::edge::RegionalEdge::new(
        aex_regional_http::authz::LambdaAssertionSource::new(
            aws_sdk_lambda::Client::new(&aws),
            config.authz_function_arn.clone(),
            AUDIENCE,
            config.region,
        ),
        anchors,
        aex_regional_http::authz::RegionalProjection::new(projection.clone(), config.region),
        aex_regional_http::capacity::CapacityProjection::new(projection),
        aex_regional_http::edge::SystemClock,
        aex_regional_http::edge::EdgeBinding {
            plane: config.plane,
            audience: AUDIENCE,
            region: config.region,
            cache_budget_bytes: config.assertion_cache_bytes,
        },
    )
    .map_err(|error| RunError::Edge {
        reason: error.to_string(),
    })?;

    let service = OtlpService::new(
        authority,
        CustodyManifests::new(dynamodb, config.secret_custody_table.clone()),
        config.limits,
        MemoryBudget::new(config.memory_budget_bytes),
        config.reserve_wait,
        redaction_key,
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
        .map_err(|error| RunError::Runtime {
            reason: error.to_string(),
        })
}

/// Resolves the regional redaction key.
///
/// The key never reaches a log, a span attribute or an error message: the only
/// thing a failure reports is that the reference did not resolve.
async fn resolve_redaction_key(
    secrets: &aws_sdk_secretsmanager::Client,
    reference: &str,
) -> Result<Vec<u8>, RunError> {
    use base64::Engine as _;

    let response = secrets
        .get_secret_value()
        .secret_id(reference)
        .send()
        .await
        .map_err(|error| RunError::Probe {
            reason: format!("the redaction key reference did not resolve: {error}"),
        })?;
    let material = response.secret_string().ok_or_else(|| RunError::Probe {
        reason: "the redaction key reference carries no string value".to_owned(),
    })?;
    let key = base64::engine::general_purpose::STANDARD
        .decode(material.trim())
        .map_err(|_| RunError::Probe {
            reason: "the redaction key is not base64".to_owned(),
        })?;
    if key.len() < 32 {
        return Err(RunError::Probe {
            reason: "the redaction key is shorter than 32 bytes".to_owned(),
        });
    }
    Ok(key)
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
    let outcome = run(config).await;
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
        // Proving a prefix is not proving the set.
        let error = compose(ROLE.granted(), &[Probe::ObservationTable]).expect_err("refused");
        assert!(matches!(error, RunError::NotReady { .. }), "{error:?}");
    }
}
