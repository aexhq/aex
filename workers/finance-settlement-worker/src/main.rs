//! `finance-settlement-worker` composition root (Rust Lambda ZIP, SQS FIFO).

use std::sync::Arc;

use aex_platform_telemetry::{FlushOutcome, Handle, Record, Settings};
use aex_rds_data::{AwsTransport, DataApiClient};
use aex_telemetry_schema::generated::{
    AEX_DEPLOYABLE, AEX_PLANE, AEX_REGION, EVENT_AEX_PROCESS_CONFIGURATION_REJECTED,
    EVENT_AEX_PROCESS_STARTED,
};
use aws_lambda_events::sqs::SqsEvent;
use finance_settlement_worker::config::Config;
use finance_settlement_worker::handler::handle;
use finance_settlement_worker::settle::{AuroraSettlementAuthority, SettlementAuthority as _};
use lambda_runtime::{LambdaEvent, service_fn};

/// The identity this deployable reports in every record.
const DEPLOYABLE: &str = "finance-settlement-worker";

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
