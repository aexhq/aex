//! `finance-ingest` composition root (Rust Lambda ZIP, direct invoke).
//!
//! Parses typed configuration, builds the Aurora inbox, installs telemetry and
//! serves the `lambda_runtime` loop. No business logic lives here.

use std::sync::Arc;

use aex_platform_telemetry::{FlushOutcome, Handle, Record, Settings};
use aex_rds_data::{AwsTransport, DataApiClient};
use aex_telemetry_schema::generated::{
    AEX_DEPLOYABLE, AEX_PLANE, AEX_REGION, EVENT_AEX_PROCESS_CONFIGURATION_REJECTED,
    EVENT_AEX_PROCESS_STARTED,
};
use finance_ingest::config::Config;
use finance_ingest::handler::{IngestRequest, IngestResponse, handle};
use finance_ingest::inbox::AuroraProviderEventInbox;
use lambda_runtime::{LambdaEvent, service_fn};

/// The identity this deployable reports in every record.
const DEPLOYABLE: &str = "finance-ingest";

#[tokio::main]
async fn main() -> std::process::ExitCode {
    let settings = Settings::lambda();
    let telemetry = Handle::install(&settings, None);

    let config = match Config::from_env() {
        Ok(config) => config,
        Err(error) => {
            telemetry.emit(
                Record::event(EVENT_AEX_PROCESS_CONFIGURATION_REJECTED)
                    .with(AEX_DEPLOYABLE, DEPLOYABLE),
            );
            let _ = telemetry.flush(settings.flush_deadline);
            eprintln!("{DEPLOYABLE}: refusing to start: {error}");
            return std::process::ExitCode::FAILURE;
        }
    };

    telemetry.emit(
        Record::event(EVENT_AEX_PROCESS_STARTED)
            .with(AEX_DEPLOYABLE, DEPLOYABLE)
            .with(AEX_PLANE, config.plane.clone())
            .with(AEX_REGION, config.region.clone()),
    );

    let outcome = run(config).await;
    if let FlushOutcome::DeadlineExceeded { pending } = telemetry.flush(settings.flush_deadline) {
        eprintln!("{DEPLOYABLE}: telemetry flush left {pending} record(s) undelivered");
    }
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
