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
async fn compose(config: &Config) -> Result<(), CentralControlApiRunError> {
    use aex_identity_app::ports::PepperPurpose;

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
            verification_set: Some(aex_control_aurora::sql::LIVE_CONTROL_PEPPERS),
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
    let cursor_keys = cursor_peppers
        .verification_set(PepperPurpose::Cursor)
        .await
        .map_err(|error| {
            CentralControlApiRunError::Dependency("cursor-secret", error.to_string())
        })?;
    let cursor_secret = Arc::new(
        aex_control_domain::CursorSecret::with_id(
            cursor_keys.current.0.get(),
            cursor_keys.current.1.expose_copy(),
        )
        .with_overlap(
            cursor_keys
                .retiring
                .into_iter()
                .map(|(version, material)| (version.get(), material.expose_copy())),
        )
        .map_err(|error| {
            CentralControlApiRunError::Dependency("cursor-secret", error.to_string())
        })?,
    );

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
    run(config, service, edge, Probes::READY).await
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
    if let Err(error) = aex_platform_diagnostics::install_json() {
        eprintln!("central-control-api: refusing to start: {error}");
        return std::process::ExitCode::FAILURE;
    }
    let config = match Config::from_env() {
        Ok(config) => config,
        Err(error) => {
            tracing::error!(
                target: "aex::diagnostics",
                event_name = "process.configuration_rejected",
                deployable = DEPLOYABLE.as_str(),
                error = %error,
                "configuration rejected"
            );
            eprintln!("central-control-api: refusing to start: {error}");
            eprintln!(
                "central-control-api: required configuration: {}",
                keys::ALL.join(", ")
            );
            return std::process::ExitCode::FAILURE;
        }
    };
    let outcome = compose(&config).await;
    match outcome {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("central-control-api: stopped: {error}");
            std::process::ExitCode::FAILURE
        }
    }
}
