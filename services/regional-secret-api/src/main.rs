//! `regional-secret-api` composition root (Rust Lambda ZIP).
//!
//! Exclusive responsibility: the plaintext-bearing secret and provider-credential
//! admission routes. It mounts nothing else, and its configuration refuses a
//! session, content, work, registry or queue binding outright.
//!
//! The binary is a composition root only: it validates configuration, builds the
//! real adapters, assembles the router from the generated route table, and hands
//! it to `lambda_http`. Behaviour lives in the library crates it composes.

use std::process::ExitCode;
use std::sync::Arc;

use aex_regional_http::config::ConfigError;
use aex_regional_http::health::Readiness;
use regional_secret_api::config::Config;

/// Why `regional-secret-api` stopped.
#[derive(Debug, thiserror::Error)]
enum RunError {
    /// Start-up configuration was rejected.
    #[error(transparent)]
    Config(#[from] ConfigError),
    /// The `HTTP` runtime stopped.
    #[error("the lambda runtime stopped: {0}")]
    Runtime(String),
}

#[tokio::main]
async fn main() -> ExitCode {
    let config = match Config::from_env() {
        Ok(config) => config,
        Err(error) => {
            eprintln!("regional-secret-api: refusing to start: {error}");
            return ExitCode::FAILURE;
        }
    };
    let settings = aex_platform_telemetry::Settings::default();
    let telemetry = aex_platform_telemetry::Handle::install(&settings, None);
    let outcome = run(&config, &telemetry).await;
    if let aex_platform_telemetry::FlushOutcome::DeadlineExceeded { pending } =
        telemetry.flush(settings.flush_deadline)
    {
        eprintln!("regional-secret-api: telemetry flush left {pending} record(s) undelivered");
    }
    match outcome {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("regional-secret-api: stopped: {error}");
            ExitCode::FAILURE
        }
    }
}

/// Builds the real adapters, assembles the router and serves it.
async fn run(config: &Config, telemetry: &aex_platform_telemetry::Handle) -> Result<(), RunError> {
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

    let aws = aws_config::load_defaults(aws_config::BehaviorVersion::latest()).await;
    let dynamodb = aws_sdk_dynamodb::Client::new(&aws);
    let kms = aws_sdk_kms::Client::new(&aws);

    // The two adapters this deployable is allowed to hold, and nothing else: the
    // custody authority and the envelope crypto over the *secret* KMS key.
    let custody = Arc::new(aex_secret_custody_dynamodb::CustodyStore::new(
        dynamodb,
        config.secret_custody_table.clone(),
    ));
    let crypto = Arc::new(aex_secret_aws::crypto::EnvelopeCrypto::new(
        Box::new(aex_secret_aws::keystore::KmsBranchKeys::new(
            kms,
            config.secret_kms_key.value.clone(),
        )),
        config.crypto_partition(),
    ));
    let edge = regional_secret_api::SecretEdge::new(custody, crypto);

    // Readiness is fail-closed and is resolved from the composition, not from a
    // constant: it reports `ready` only when every declared dependency of the
    // served route set is bound.
    let readiness = match edge.unresolved(config) {
        unresolved if unresolved.is_empty() => Readiness::ready(config.release_digest.clone()),
        unresolved => Readiness::not_ready(config.release_digest.clone(), unresolved)
            .map_err(|error| RunError::Runtime(error.to_string()))?,
    };

    lambda_http::run(aex_regional_http::health::router(readiness))
        .await
        .map_err(|error| RunError::Runtime(error.to_string()))
}
