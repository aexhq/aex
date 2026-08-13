//! `provider-cost-reconciler` composition root (Rust Lambda ZIP, daily).

use std::sync::Arc;

use aex_rds_data::{AwsTransport, DataApiClient};
use lambda_runtime::{LambdaEvent, service_fn};
use provider_cost_reconciler::config::Config;
use provider_cost_reconciler::cost::{AuroraProviderCostLedger, ProviderCostLedger as _};
use provider_cost_reconciler::export::S3CostExport;
use provider_cost_reconciler::handler::{CostRequest, handle};

/// The identity this deployable reports in every record.
const DEPLOYABLE: &str = "provider-cost-reconciler";

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

/// Builds the adapters and reconciles until the runtime stops.
async fn run(config: Config) -> Result<(), lambda_runtime::Error> {
    let aws = aws_config::load_defaults(aws_config::BehaviorVersion::latest()).await;
    let transport = AwsTransport::new(aws_sdk_rdsdata::Client::new(&aws), &config.data_api);
    let client = Arc::new(DataApiClient::new(
        Arc::new(transport),
        config.data_api.clone(),
    ));
    let ledger = Arc::new(AuroraProviderCostLedger::new(
        Arc::clone(&client),
        config.database_role.clone(),
    ));
    let export = Arc::new(S3CostExport::new(
        aws_sdk_s3::Client::new(&aws),
        config.cur_bucket.clone(),
        config.cur_prefix.clone(),
    ));

    // The probe refuses to start a process that can reach a customer posting,
    // which is the enforcement of F-26 rather than a note about it.
    ledger
        .probe_role()
        .await
        .map_err(|error| lambda_runtime::Error::from(error.to_string()))?;

    let max_scan_bytes = config.max_scan_bytes;
    let threshold_bps = config.margin_alert_threshold_bps;
    lambda_runtime::run(service_fn(move |event: LambdaEvent<serde_json::Value>| {
        let ledger = Arc::clone(&ledger);
        let export = Arc::clone(&export);
        async move {
            // A scheduled rule that names no period reconciles the month that
            // has just closed; guessing the current month would rate a period
            // whose export is still being written.
            let request =
                serde_json::from_value::<CostRequest>(event.payload).unwrap_or_else(|_| {
                    CostRequest::Reconcile {
                        period: previous_period(time::OffsetDateTime::now_utc()),
                    }
                });
            handle(&ledger, &export, request, max_scan_bytes, threshold_bps)
                .await
                .map_err(|error| lambda_runtime::Error::from(error.to_string()))
        }
    }))
    .await
}

/// The `YYYY-MM` period before the one `now` falls in.
fn previous_period(now: time::OffsetDateTime) -> String {
    let (year, month) = if now.month() == time::Month::January {
        (now.year() - 1, 12)
    } else {
        (now.year(), u8::from(now.month()) - 1)
    };
    format!("{year:04}-{month:02}")
}
