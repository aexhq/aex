//! `finance-reconcile` composition root (Rust Lambda ZIP, scheduled).

use std::sync::Arc;

use aex_rds_data::{AwsTransport, DataApiClient};
use finance_reconcile::config::Config;
use finance_reconcile::gateway::LambdaEffectRecoveryGateway;
use finance_reconcile::handler::{ReconcileRequest, handle};
use finance_reconcile::sweep::{
    AuroraReconcileAuthority, ReconcileAuthority as _, SnsOperationsAlarm,
};
use lambda_runtime::{LambdaEvent, service_fn};

/// The identity this deployable reports in every record.
const DEPLOYABLE: &str = "finance-reconcile";

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

/// Builds the adapters and sweeps until the runtime stops.
async fn run(config: Config) -> Result<(), lambda_runtime::Error> {
    let aws = aws_config::load_defaults(aws_config::BehaviorVersion::latest()).await;
    let transport = AwsTransport::new(aws_sdk_rdsdata::Client::new(&aws), &config.data_api);
    let client = Arc::new(DataApiClient::new(
        Arc::new(transport),
        config.data_api.clone(),
    ));
    let recovery = Arc::new(LambdaEffectRecoveryGateway::new(
        aws_sdk_lambda::Client::new(&aws),
        config.command_edge_arn.clone(),
    ));
    let authority = Arc::new(AuroraReconcileAuthority::new(
        Arc::clone(&client),
        config.database_role.clone(),
        recovery,
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
