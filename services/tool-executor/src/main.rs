//! `tool-executor` composition root (Fargate, private listener).
//!
//! The binary validates configuration, unwraps the one secret it holds, builds
//! the real adapters and serves one route. Behaviour lives in the library.

use std::process::ExitCode;
use std::sync::Arc;

use aex_brain_managed_web::egress::SystemDnsResolver;
use aex_brain_managed_web::search::WebSearchCredential;
use tool_executor::admit::Admitter;
use tool_executor::config::{Config, ConfigError};
use tool_executor::handler::{Executor, router};
use tool_executor::run::ManagedWebRunner;
use tool_executor::spend::DynamoOrganizationCeiling;

/// Why `tool-executor` stopped.
#[derive(Debug, thiserror::Error)]
enum RunError {
    /// Start-up configuration was rejected.
    #[error(transparent)]
    Config(#[from] ConfigError),
    /// The platform credential could not be read or was malformed.
    #[error("the platform search credential is unusable: {0}")]
    Credential(String),
    /// The listener could not be bound or stopped.
    #[error("the listener stopped: {0}")]
    Listener(String),
}

#[tokio::main]
async fn main() -> ExitCode {
    // Telemetry first: a configuration refusal must reach the wire, or a
    // crash-looping deployment is visible only to whoever tails stderr.
    let settings = aex_platform_telemetry::Settings::default();
    let telemetry = aex_platform_telemetry::Handle::install(&settings, None);

    let outcome = run().await;
    if let aex_platform_telemetry::FlushOutcome::DeadlineExceeded { pending } =
        telemetry.flush(settings.flush_deadline)
    {
        eprintln!("tool-executor: telemetry flush left {pending} record(s) undelivered");
    }
    match outcome {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("tool-executor: refusing to start: {error}");
            ExitCode::FAILURE
        }
    }
}

async fn run() -> Result<(), RunError> {
    let config = Config::from_env()?;
    let aws = aws_config::load_from_env().await;

    // The one secret this process holds, unwrapped once and never written
    // anywhere. If it is absent or malformed the process refuses to start: a
    // task that reported ready and then failed every call at the vendor would
    // be indistinguishable from a vendor outage.
    let secrets = aws_sdk_secretsmanager::Client::new(&aws);
    let raw = secrets
        .get_secret_value()
        .secret_id(&config.credential_secret_id)
        .send()
        .await
        .map_err(|error| RunError::Credential(error.into_service_error().to_string()))?;
    let bytes = raw
        .secret_string()
        .map(str::as_bytes)
        .map(<[u8]>::to_vec)
        .ok_or_else(|| RunError::Credential("the secret carries no string value".to_owned()))?;
    let credential = WebSearchCredential::parse(&bytes)
        .map_err(|error| RunError::Credential(error.to_string()))?;

    let executor = Executor::new(
        Admitter::new(config.verification_keys, config.plane, config.region),
        Arc::new(DynamoOrganizationCeiling::new(
            aws_sdk_dynamodb::Client::new(&aws),
            config.ceiling_table,
            config.ceilings,
        )),
        Arc::new(ManagedWebRunner::new(
            credential,
            Box::new(SystemDnsResolver),
        )),
    );

    let listener = tokio::net::TcpListener::bind(("0.0.0.0", config.port))
        .await
        .map_err(|error| RunError::Listener(error.to_string()))?;
    axum::serve(listener, router(Arc::new(executor)))
        .await
        .map_err(|error| RunError::Listener(error.to_string()))
}
