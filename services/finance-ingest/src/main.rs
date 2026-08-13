//! `finance-ingest` composition root (Rust Lambda ZIP, direct invoke).
//!
//! Parses typed configuration, builds the Aurora inbox, installs telemetry and
//! serves the `lambda_runtime` loop. No business logic lives here.

use std::sync::Arc;

use aex_rds_data::{AwsTransport, DataApiClient};
use finance_ingest::config::Config;
use finance_ingest::handler::{IngestRequest, IngestResponse, handle};
use finance_ingest::inbox::AuroraProviderEventInbox;
use lambda_runtime::{LambdaEvent, service_fn};

/// The identity this deployable reports in every record.
const DEPLOYABLE: &str = "finance-ingest";

#[tokio::main]
async fn main() -> std::process::ExitCode {
    if let Err(error) = aex_platform_diagnostics::install_json() {
        eprintln!("{DEPLOYABLE}: diagnostics installation failed: {error}");
        return std::process::ExitCode::FAILURE;
    }

    let config = match Config::from_env() {
        Ok(config) => config,
        Err(error) => {
            tracing::error!(
                target: "aex::diagnostics",
                event_name = "process.configuration_rejected",
                deployable = DEPLOYABLE,
                error = %error,
                "process configuration rejected"
            );
            eprintln!("{DEPLOYABLE}: refusing to start: {error}");
            return std::process::ExitCode::FAILURE;
        }
    };

    tracing::info!(
        target: "aex::diagnostics",
        event_name = "process.started",
        deployable = DEPLOYABLE,
        plane = %config.plane,
        region = %config.region,
        "process started"
    );

    let outcome = run(config).await;
    match outcome {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{DEPLOYABLE}: stopped: {error}");
            std::process::ExitCode::FAILURE
        }
    }
}

/// Builds the adapters and serves until the runtime stops.
async fn run(config: Config) -> Result<(), lambda_runtime::Error> {
    let aws = aws_config::load_defaults(aws_config::BehaviorVersion::latest()).await;
    let transport = AwsTransport::new(aws_sdk_rdsdata::Client::new(&aws), &config.data_api);
    let client = Arc::new(DataApiClient::new(
        Arc::new(transport),
        config.data_api.clone(),
    ));
    let inbox = Arc::new(AuroraProviderEventInbox::new(
        Arc::clone(&client),
        config.database_role.clone(),
        config.pinned_api_version.clone(),
    ));

    lambda_runtime::run(service_fn(move |event: LambdaEvent<IngestRequest>| {
        let inbox = Arc::clone(&inbox);
        async move {
            handle(&inbox, event.payload)
                .await
                .map_err(|error| lambda_runtime::Error::from(error.to_string()))
                .map(|answer: IngestResponse| answer)
        }
    }))
    .await
}
