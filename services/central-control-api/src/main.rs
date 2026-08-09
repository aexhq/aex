//! `central-control-api` composition root (Rust Lambda ZIP).
//!
//! Everything this binary serves lives in the [`central_control_api`] library:
//! the handlers, the configuration reader, the capability manifest, the
//! readiness projection and the router. This file is only what a Lambda
//! deployment adds — the real AWS adapters, the start-up probes and the
//! `lambda_http` entry point.
//!
//! The split is what lets `services/central-api` compose the same five route
//! groups into a long-lived Fargate process without duplicating a line of it.

use std::sync::Arc;

use central_control_api::{CentralControlApiRunError, Config, DEPLOYABLE, Probes, api, keys, run};

use aex_central_http::router::EdgeStack;

/// Builds the real adapters, proves each one answers, and serves.
///
/// Every probe runs **before** `lambda_http::run`: a composition that cannot
/// reach Aurora, cannot resolve its peppers and cannot sign a cursor can serve
/// nothing, and a listener that answers a permanent failure on every route is
/// worse than a process that refuses to start.
async fn compose(
    config: &Config,
    telemetry: &aex_platform_telemetry::Handle,
) -> Result<(), CentralControlApiRunError> {
    use aex_identity_app::ports::{PepperKeystore as _, PepperPurpose};

    let aws = aws_config::load_defaults(aws_config::BehaviorVersion::latest()).await;
    let data_api = aex_rds_data::DataApiConfig::new(
        aex_rds_data::ResourceArn::parse(&config.aurora_cluster_arn)
            .map_err(|error| CentralControlApiRunError::Dependency("aurora", error.to_string()))?,
        aex_rds_data::SecretArn::parse(&config.aurora_secret_arn)
            .map_err(|error| CentralControlApiRunError::Dependency("aurora", error.to_string()))?,
        aex_rds_data::DatabaseName::parse(&config.database)
            .map_err(|error| CentralControlApiRunError::Dependency("aurora", error.to_string()))?,
    );
    let client = aex_rds_data::DataApiClient::new(
        Arc::new(aex_rds_data::AwsTransport::new(
            aws_sdk_rdsdata::Client::new(&aws),
            &data_api,
        )),
        data_api,
    );
    client
        .query::<Ok1>(aex_rds_data::Statement::new(
            aex_control_aurora::sql::READINESS_PROBE,
        ))
        .await
        .map_err(|error| CentralControlApiRunError::Dependency("aurora", error.to_string()))?;

    let directory = Arc::new(aex_central_aws::DataApiPepperDirectory::new(
        client.clone(),
        aex_central_aws::PepperStatements {
            active: aex_control_aurora::sql::ACTIVE_CONTROL_PEPPER,
            by_version: aex_control_aurora::sql::CONTROL_PEPPER_BY_VERSION,
        },
    ));
    let api_peppers = Arc::new(aex_central_aws::SecretsManagerPepperKeystore::new(
        aws_sdk_secretsmanager::Client::new(&aws),
        config.pepper_secret_id.clone(),
        Arc::clone(&directory) as Arc<dyn aex_central_aws::PepperDirectory>,
    ));
    api_peppers
        .probe(PepperPurpose::ApiKey)
        .await
        .map_err(|error| {
            CentralControlApiRunError::Dependency("api-key-pepper", error.to_string())
        })?;
    let cursor_peppers = Arc::new(aex_central_aws::SecretsManagerPepperKeystore::new(
        aws_sdk_secretsmanager::Client::new(&aws),
        config.cursor_secret_id.clone(),
        directory,
    ));
    cursor_peppers
        .probe(PepperPurpose::Cursor)
        .await
        .map_err(|error| {
            CentralControlApiRunError::Dependency("cursor-secret", error.to_string())
        })?;
    let (_, cursor_material) =
        cursor_peppers
            .active(PepperPurpose::Cursor)
            .await
            .map_err(|error| {
                CentralControlApiRunError::Dependency("cursor-secret", error.to_string())
            })?;
    let cursor_secret = Arc::new(aex_control_domain::CursorSecret::new(
        cursor_material.expose_copy(),
    ));

    let concrete_store = Arc::new(aex_control_aurora::AuroraControlStore::new(client));
    let api_store: Arc<dyn api::Store> = concrete_store.clone();
    let target_store: Arc<dyn aex_control_app::ports::ControlStore> = concrete_store;
    let clock: Arc<dyn aex_identity_app::ports::Clock> = Arc::new(aex_central_aws::SystemClock);
    let regional: Arc<dyn aex_control_app::ports::RegionalControlPort> =
        Arc::new(aex_central_aws::LambdaRegionalControl::new(
            aws_sdk_lambda::Client::new(&aws),
            config.regional_functions.clone(),
        ));
    let service = Arc::new(api::ControlService::new(
        api_store,
        Arc::clone(&api_peppers) as Arc<dyn aex_identity_app::ports::PepperKeystore>,
        regional,
        Arc::clone(&clock),
        Arc::new(aex_central_aws::Uuid7Factory),
        Arc::new(aex_central_aws::OsSecretRng),
        Arc::clone(&cursor_secret),
        config.http.region,
        config.api_urls.clone(),
    ));
    let edge = EdgeStack::new(
        config.http.clone(),
        Arc::new(aex_central_http::target::ControlStoreTargets::new(
            target_store,
        )),
        clock,
        cursor_secret,
    );
    run(config, service, edge, Probes::READY, telemetry).await
}

struct Ok1;

impl aex_rds_data::Row for Ok1 {
    fn from_record(record: &aex_rds_data::Record<'_>) -> Result<Self, aex_rds_data::DecodeError> {
        record.expect_arity(1)?;
        record.i64(0)?;
        Ok(Self)
    }
}

#[tokio::main]
async fn main() -> std::process::ExitCode {
    // Telemetry first: a configuration refusal must reach the wire, or a
    // crash-looping deployment is visible only to whoever tails stderr.
    let settings = aex_platform_telemetry::Settings::default();
    let telemetry = aex_platform_telemetry::Handle::install(&settings, None);
    let config = match Config::from_env() {
        Ok(config) => config,
        Err(error) => {
            telemetry.emit(
                aex_platform_telemetry::Record::event(
                    aex_telemetry_schema::generated::EVENT_AEX_PROCESS_CONFIGURATION_REJECTED,
                )
                .with(
                    aex_telemetry_schema::generated::AEX_DEPLOYABLE,
                    DEPLOYABLE.as_str(),
                ),
            );
            let _ = telemetry.flush(settings.flush_deadline);
            eprintln!("central-control-api: refusing to start: {error}");
            eprintln!(
                "central-control-api: required configuration: {}",
                keys::ALL.join(", ")
            );
            return std::process::ExitCode::FAILURE;
        }
    };
    let outcome = compose(&config, &telemetry).await;
    let _ = telemetry.flush(settings.flush_deadline);
    match outcome {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("central-control-api: stopped: {error}");
            std::process::ExitCode::FAILURE
        }
    }
}
