//! `finance-reconcile` composition root (Rust Lambda ZIP, scheduled).

use std::sync::Arc;

use aex_platform_telemetry::{FlushOutcome, Handle, Record, Settings};
use aex_rds_data::{AwsTransport, DataApiClient};
use aex_telemetry_schema::generated::{
    AEX_DEPLOYABLE, AEX_PLANE, AEX_REGION, EVENT_AEX_PROCESS_CONFIGURATION_REJECTED,
    EVENT_AEX_PROCESS_STARTED,
};
use finance_reconcile::config::Config;
use finance_reconcile::handler::{ReconcileRequest, handle};
use finance_reconcile::sweep::{
    AuroraReconcileAuthority, ReconcileAuthority as _, SnsOperationsAlarm,
};
use lambda_runtime::{LambdaEvent, service_fn};

/// The identity this deployable reports in every record.
const DEPLOYABLE: &str = "finance-reconcile";

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

/// Builds the adapters and sweeps until the runtime stops.
async fn run(config: Config) -> Result<(), lambda_runtime::Error> {
    let aws = aws_config::load_defaults(aws_config::BehaviorVersion::latest()).await;
    let transport = AwsTransport::new(aws_sdk_rdsdata::Client::new(&aws), &config.data_api);
    let client = Arc::new(DataApiClient::new(
        Arc::new(transport),
        config.data_api.clone(),
    ));
    let authority = Arc::new(AuroraReconcileAuthority::new(
        Arc::clone(&client),
        config.database_role.clone(),
    ));
    let alarm = Arc::new(SnsOperationsAlarm::new(
        aws_sdk_sns::Client::new(&aws),
        config.alarm_topic_arn.clone(),
    ));

    // A reconciler that cannot prove its own grants would report "all clear"
    // about a database it cannot read.
    authority
        .probe_role()
        .await
        .map_err(|error| lambda_runtime::Error::from(error.to_string()))?;

    let page_limit = config.sweep_page;
    let retry_window = config.unknown_effect_retry_window;
    lambda_runtime::run(service_fn(move |event: LambdaEvent<serde_json::Value>| {
        let authority = Arc::clone(&authority);
        let alarm = Arc::clone(&alarm);
        async move {
            // A scheduled rule may carry an EventBridge envelope this worker has
            // no use for; an unrecognised payload is the ordinary sweep.
            let request = serde_json::from_value::<ReconcileRequest>(event.payload)
                .unwrap_or(ReconcileRequest::Sweep);
            handle(
                &authority,
                &alarm,
                request,
                page_limit,
                retry_window,
                time::OffsetDateTime::now_utc(),
            )
            .await
            .map_err(|error| lambda_runtime::Error::from(error.to_string()))
        }
    }))
    .await
}
