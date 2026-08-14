//! `billing-worker` composition root for provider ingest, settlement, and reconciliation.

use std::sync::Arc;

use aex_rds_data::{AwsTransport, DataApiClient};
use finance_ingest::config::Config as IngestConfig;
use finance_ingest::handler::handle as handle_ingest;
use finance_ingest::inbox::{AuroraProviderEventInbox, ProviderEventInbox as _};
use finance_ingest::reconcile::config::Config as ReconcileConfig;
use finance_ingest::reconcile::gateway::{StripeEffectRecoveryGateway, StripeSecret};
use finance_ingest::reconcile::handler::handle as handle_reconcile;
use finance_ingest::reconcile::sweep::{
    AuroraReconcileAuthority, ReconcileAuthority as _, SnsOperationsAlarm,
};
use finance_ingest::runtime::{Invocation, classify};
use finance_ingest::settlement::config::Config as SettlementConfig;
use finance_ingest::settlement::handler::handle as handle_settlement;
use finance_ingest::settlement::settle::{AuroraSettlementAuthority, SettlementAuthority as _};
use lambda_runtime::{LambdaEvent, service_fn};
use serde_json::{Value, json};

const DEPLOYABLE: &str = "billing-worker";

#[tokio::main]
async fn main() -> std::process::ExitCode {
    if let Err(error) = aex_platform_diagnostics::install_json() {
        eprintln!("{DEPLOYABLE}: diagnostics installation failed: {error}");
        return std::process::ExitCode::FAILURE;
    }

    let configs = load_configs();
    let (ingest, settlement, reconcile) = match configs {
        Ok(configs) => configs,
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
        plane = %ingest.plane,
        region = %ingest.region,
        "process started"
    );

    match run(ingest, settlement, reconcile).await {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{DEPLOYABLE}: stopped: {error}");
            std::process::ExitCode::FAILURE
        }
    }
}

fn load_configs() -> Result<(IngestConfig, SettlementConfig, ReconcileConfig), String> {
    let ingest = IngestConfig::from_env().map_err(|error| error.to_string())?;
    let settlement = SettlementConfig::from_env().map_err(|error| error.to_string())?;
    let reconcile = ReconcileConfig::from_env().map_err(|error| error.to_string())?;
    if ingest.plane != settlement.plane
        || ingest.plane != reconcile.plane
        || ingest.region != settlement.region
        || ingest.region != reconcile.region
    {
        return Err("all billing modes must bind the same plane and region".to_owned());
    }
    Ok((ingest, settlement, reconcile))
}

async fn run(
    ingest_config: IngestConfig,
    settlement_config: SettlementConfig,
    reconcile_config: ReconcileConfig,
) -> Result<(), lambda_runtime::Error> {
    let aws = aws_config::load_defaults(aws_config::BehaviorVersion::latest()).await;

    let ingest_client = client(&aws, &ingest_config.data_api);
    let inbox = Arc::new(AuroraProviderEventInbox::new(
        ingest_client,
        ingest_config.database_role,
        ingest_config.pinned_api_version,
    ));

    let settlement_client = client(&aws, &settlement_config.data_api);
    let settlement = Arc::new(AuroraSettlementAuthority::new(
        settlement_client,
        settlement_config.database_role,
    ));

    let stripe_secret = aws_sdk_secretsmanager::Client::new(&aws)
        .get_secret_value()
        .secret_id(&reconcile_config.stripe_api_secret_arn)
        .send()
        .await
        .map_err(|error| lambda_runtime::Error::from(error.to_string()))?
        .secret_string()
        .ok_or_else(|| lambda_runtime::Error::from("Stripe secret has no SecretString"))?
        .to_owned();
    let recovery = Arc::new(
        StripeEffectRecoveryGateway::new(
            StripeSecret::new(stripe_secret)
                .map_err(|error| lambda_runtime::Error::from(error.to_string()))?,
        )
        .map_err(|error| lambda_runtime::Error::from(error.to_string()))?,
    );
    let reconcile_client = client(&aws, &reconcile_config.data_api);
    let reconcile = Arc::new(AuroraReconcileAuthority::new(
        reconcile_client,
        reconcile_config.database_role,
        recovery,
    ));
    let alarm = Arc::new(SnsOperationsAlarm::new(
        aws_sdk_sns::Client::new(&aws),
        reconcile_config.alarm_topic_arn,
    ));

    settlement
        .probe_role()
        .await
        .map_err(|error| lambda_runtime::Error::from(error.to_string()))?;
    reconcile
        .probe_role()
        .await
        .map_err(|error| lambda_runtime::Error::from(error.to_string()))?;

    let max_group_batch = settlement_config.max_group_batch;
    let serialization_retry_max = settlement_config.serialization_retry_max;
    let page_limit = reconcile_config.sweep_page;
    let retry_window = reconcile_config.unknown_effect_retry_window;

    lambda_runtime::run(service_fn(move |event: LambdaEvent<Value>| {
        let inbox = Arc::clone(&inbox);
        let settlement = Arc::clone(&settlement);
        let reconcile = Arc::clone(&reconcile);
        let alarm = Arc::clone(&alarm);
        async move {
            let response = match classify(event.payload)
                .map_err(|error| lambda_runtime::Error::from(error.to_string()))?
            {
                Invocation::Ingest(request) => serde_json::to_value(
                    handle_ingest(&inbox, request)
                        .await
                        .map_err(|error| lambda_runtime::Error::from(error.to_string()))?,
                ),
                Invocation::Settlement(event) => {
                    let (response, _report) = handle_settlement(
                        &settlement,
                        event,
                        max_group_batch,
                        serialization_retry_max,
                    )
                    .await;
                    serde_json::to_value(response)
                }
                Invocation::Reconcile(request) => serde_json::to_value(
                    handle_reconcile(
                        &reconcile,
                        &alarm,
                        request,
                        page_limit,
                        retry_window,
                        time::OffsetDateTime::now_utc(),
                    )
                    .await
                    .map_err(|error| lambda_runtime::Error::from(error.to_string()))?,
                ),
                Invocation::Readyz => {
                    let ingest = inbox.probe_role().await.map_err(|error| error.to_string());
                    let settlement = settlement
                        .probe_role()
                        .await
                        .map_err(|error| error.to_string());
                    let reconcile = reconcile
                        .probe_role()
                        .await
                        .map_err(|error| error.to_string());
                    let ready = ingest.is_ok() && settlement.is_ok() && reconcile.is_ok();
                    Ok(json!({
                        "result": "ready",
                        "ready": ready,
                        "ingest": ingest.err(),
                        "settlement": settlement.err(),
                        "reconcile": reconcile.err(),
                    }))
                }
            }
            .map_err(|error| lambda_runtime::Error::from(error.to_string()))?;
            Ok::<Value, lambda_runtime::Error>(response)
        }
    }))
    .await
}

fn client(
    aws: &aws_config::SdkConfig,
    config: &aex_rds_data::config::DataApiConfig,
) -> Arc<DataApiClient> {
    let transport = AwsTransport::new(aws_sdk_rdsdata::Client::new(aws), config);
    Arc::new(DataApiClient::new(Arc::new(transport), config.clone()))
}
