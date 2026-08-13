//! `finance-settlement-worker` composition root (Rust Lambda ZIP, SQS FIFO).

use std::sync::Arc;

use aex_rds_data::{AwsTransport, DataApiClient};
use aws_lambda_events::sqs::SqsEvent;
use finance_settlement_worker::config::Config;
use finance_settlement_worker::handler::handle;
use finance_settlement_worker::settle::{AuroraSettlementAuthority, SettlementAuthority as _};
use lambda_runtime::{LambdaEvent, service_fn};

/// The identity this deployable reports in every record.
const DEPLOYABLE: &str = "finance-settlement-worker";

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

/// Builds the adapters and drains the FIFO backlog until the runtime stops.
async fn run(config: Config) -> Result<(), lambda_runtime::Error> {
    let aws = aws_config::load_defaults(aws_config::BehaviorVersion::latest()).await;
    let transport = AwsTransport::new(aws_sdk_rdsdata::Client::new(&aws), &config.data_api);
    let client = Arc::new(DataApiClient::new(
        Arc::new(transport),
        config.data_api.clone(),
    ));
    let authority = Arc::new(AuroraSettlementAuthority::new(
        Arc::clone(&client),
        config.database_role.clone(),
    ));

    // The role probe runs once at init. A worker that cannot prove its own
    // grants stops rather than draining a money queue it cannot settle.
    authority
        .probe_role()
        .await
        .map_err(|error| lambda_runtime::Error::from(error.to_string()))?;

    let max_group_batch = config.max_group_batch;
    let serialization_retry_max = config.serialization_retry_max;
    // The event is decoded raw and each record's body is parsed individually
    // inside the handler: a typed event layer here would fail the whole batch
    // into the DLQ on one undecodable body.
    lambda_runtime::run(service_fn(move |event: LambdaEvent<SqsEvent>| {
        let authority = Arc::clone(&authority);
        async move {
            let (response, _report) = handle(
                &authority,
                event.payload,
                max_group_batch,
                serialization_retry_max,
            )
            .await;
            Ok::<_, lambda_runtime::Error>(response)
        }
    }))
    .await
}
