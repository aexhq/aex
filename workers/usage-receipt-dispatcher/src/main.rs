//! `usage-receipt-dispatcher` composition root (Rust Lambda ZIP, scheduled).

use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use aex_platform_telemetry::{FlushOutcome, Handle, Record, Settings};
use aex_rds_data::{AwsTransport, DataApiClient};
use aex_telemetry_schema::generated::{
    AEX_DEPLOYABLE, AEX_PLANE, AEX_REGION, EVENT_AEX_PROCESS_CONFIGURATION_REJECTED,
    EVENT_AEX_PROCESS_STARTED,
};
use lambda_runtime::{LambdaEvent, service_fn};
use usage_receipt_dispatcher::config::{Config, DEADLINE_SAFETY_MARGIN};
use usage_receipt_dispatcher::handler::{DispatchRequest, handle};
use usage_receipt_dispatcher::outbox::{
    AuroraReceiptOutbox, ReceiptOutbox as _, SqsReceiptPublisher,
};

/// The identity this deployable reports in every record.
const DEPLOYABLE: &str = "usage-receipt-dispatcher";

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

/// Builds the adapters and drains the outbox until the runtime stops.
async fn run(config: Config) -> Result<(), lambda_runtime::Error> {
    let aws = aws_config::load_defaults(aws_config::BehaviorVersion::latest()).await;
    let transport = AwsTransport::new(aws_sdk_rdsdata::Client::new(&aws), &config.data_api);
    let client = Arc::new(DataApiClient::new(
        Arc::new(transport),
        config.data_api.clone(),
    ));
    let outbox = Arc::new(AuroraReceiptOutbox::new(
        Arc::clone(&client),
        config.database_role.clone(),
    ));
    let publisher = Arc::new(SqsReceiptPublisher::new(
        aws_sdk_sqs::Client::new(&aws),
        config.receipt_queue_url_by_category.clone(),
    ));

    outbox
        .probe_role()
        .await
        .map_err(|error| lambda_runtime::Error::from(error.to_string()))?;

    let categories: Vec<String> = config
        .receipt_queue_url_by_category
        .keys()
        .cloned()
        .collect();
    let page_limit = config.batch_size;
    let max_attempts = config.max_attempts;
    lambda_runtime::run(service_fn(move |event: LambdaEvent<serde_json::Value>| {
        let outbox = Arc::clone(&outbox);
        let publisher = Arc::clone(&publisher);
        let categories = categories.clone();
        async move {
            let stop = drain_stop(event.context.deadline);
            // A scheduled rule may carry an EventBridge envelope this worker has
            // no use for; an unrecognised payload is the ordinary drain.
            let request = serde_json::from_value::<DispatchRequest>(event.payload)
                .unwrap_or(DispatchRequest::Drain);
            handle(
                &outbox,
                &publisher,
                request,
                &categories,
                page_limit,
                max_attempts,
                stop,
            )
            .await
            .map_err(|error| lambda_runtime::Error::from(error.to_string()))
        }
    }))
    .await
}

/// Where paging must stop: the runtime's own deadline minus the margin.
///
/// The runtime hands each invocation its deadline as epoch milliseconds. A
/// deadline at or before now yields a stop that has already passed, which the
/// drain treats as "one guaranteed page per category" rather than zero work.
fn drain_stop(deadline_epoch_ms: u64) -> Instant {
    let now_epoch_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| {
            u64::try_from(since.as_millis()).unwrap_or(u64::MAX)
        });
    let remaining = Duration::from_millis(deadline_epoch_ms.saturating_sub(now_epoch_ms));
    Instant::now() + remaining.saturating_sub(DEADLINE_SAFETY_MARGIN)
}
