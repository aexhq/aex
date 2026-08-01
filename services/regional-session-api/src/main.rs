//! `regional-session-api` composition root (Rust Lambda ZIP).
//!
//! Exclusive responsibility: the finite session, run, operation, registry,
//! upload, content-metadata, approval, secret-metadata and usage routes.
//!
//! The binary is a composition root only: it validates configuration, builds the
//! real adapters, assembles the router from the generated route table, and hands
//! it to `lambda_http`. Behaviour lives in the library crates it composes.

use std::process::ExitCode;

use aex_regional_http::config::ConfigError;
use aex_regional_http::health::Readiness;
use regional_session_api::config::Config;

/// Why `regional-session-api` stopped.
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
            eprintln!("regional-session-api: refusing to start: {error}");
            return ExitCode::FAILURE;
        }
    };
    let settings = aex_platform_telemetry::Settings::default();
    let telemetry = aex_platform_telemetry::Handle::install(&settings, None);
    let outcome = run(&config, &telemetry).await;
    if let aex_platform_telemetry::FlushOutcome::DeadlineExceeded { pending } =
        telemetry.flush(settings.flush_deadline)
    {
        eprintln!("regional-session-api: telemetry flush left {pending} record(s) undelivered");
    }
    match outcome {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("regional-session-api: stopped: {error}");
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
    let objects = aws_sdk_s3::Client::new(&aws);

    let stores = regional_session_api::Stores::build(config, &dynamodb, &objects);
    let readiness = match stores.unresolved() {
        unresolved if unresolved.is_empty() => Readiness::ready(config.release_digest.clone()),
        unresolved => Readiness::not_ready(config.release_digest.clone(), unresolved)
            .map_err(|error| RunError::Runtime(error.to_string()))?,
    };

    // The handler surface is complete for `regional_session_api::Routes::served()`
    // and is mounted by `mount_unary` in the `served` target. It is **not**
    // mounted here, and the reason is not this deployable's:
    // `aex_regional_http::mount::mount_unary` requires an `EdgeAdmission`, whose
    // only implementation needs an `AssertionSource` and a `KeyVerifier`. The
    // first has no published `central-authz` request/response shape for a
    // workspace key and no `aws-sdk-lambda` in `[workspace.dependencies]`; the
    // second has no parameter-store reader. Mounting a route whose edge cannot
    // admit anything would answer a permanent failure, which RS-18 forbids, so
    // the routes stay absent and the process serves its health surface only.
    let _served = regional_session_api::Routes::served();
    lambda_http::run(aex_regional_http::health::router(readiness))
        .await
        .map_err(|error| RunError::Runtime(error.to_string()))
}
