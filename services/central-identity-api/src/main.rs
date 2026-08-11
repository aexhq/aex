//! `central-identity-api` composition root (Rust Lambda ZIP).
//!
//! Everything this binary serves lives in the [`central_identity_api`] library:
//! the two device-flow handlers, the configuration reader, the capability
//! manifest, the readiness projection and the router. This file is only what a
//! Lambda deployment adds — the real AWS adapters, the start-up probes and the
//! `lambda_http` entry point.

use std::sync::Arc;

use central_identity_api::{
    CentralIdentityApiRunError, Config, DEPLOYABLE, Probes, api, keys, oauth, run, targets,
};

use aex_central_http::router::EdgeStack;

/// Builds the real adapters, proves each one answers, and serves.
///
/// Both probes run **before** the listener binds, and either failing refuses the
/// process. The alternative — serving with a false readiness flag — admits
/// requests this deployable can only fail, and a device-flow route that answers
/// without an identity store is one that mints nothing and says it did.
async fn compose(
    config: &Config,
    telemetry: &aex_platform_telemetry::Handle,
) -> Result<(), CentralIdentityApiRunError> {
    let aws = aws_config::load_defaults(aws_config::BehaviorVersion::latest()).await;
    let data_api = aex_rds_data::DataApiConfig::new(
        aex_rds_data::ResourceArn::parse(&config.aurora_cluster_arn)
            .map_err(|error| CentralIdentityApiRunError::Dependency("aurora", error.to_string()))?,
        aex_rds_data::SecretArn::parse(&config.aurora_secret_arn)
            .map_err(|error| CentralIdentityApiRunError::Dependency("aurora", error.to_string()))?,
        aex_rds_data::DatabaseName::parse(&config.database)
            .map_err(|error| CentralIdentityApiRunError::Dependency("aurora", error.to_string()))?,
    );
    let client = aex_rds_data::DataApiClient::new(
        Arc::new(aex_rds_data::AwsTransport::new(
            aws_sdk_rdsdata::Client::new(&aws),
            &data_api,
        )),
        data_api,
    );

    // Probe one: the login role can read. `SELECT 1` as `aex_identity_api` is
    // the cheapest statement that proves the cluster is awake, the credential
    // resolves and the role exists.
    client
        .query::<Ok1>(aex_rds_data::Statement::new(
            aex_identity_aurora::sql::READINESS_PROBE,
        ))
        .await
        .map_err(|error| CentralIdentityApiRunError::Dependency("aurora", error.to_string()))?;

    let secrets = aws_sdk_secretsmanager::Client::new(&aws);
    let peppers = Arc::new(aex_central_aws::SecretsManagerPepperKeystore::new(
        secrets.clone(),
        config.pepper_secret_id.clone(),
        Arc::new(aex_central_aws::DataApiPepperDirectory::new(
            client.clone(),
            aex_central_aws::PepperStatements {
                active: aex_identity_aurora::sql::ACTIVE_IDENTITY_PEPPER,
                by_version: aex_identity_aurora::sql::IDENTITY_PEPPER_BY_VERSION,
            },
        )),
    ));

    // Probe two: the active identity pepper resolves, end to end — the
    // lifecycle row, then its material by that row's version id. A process that
    // cannot load it can verify nothing and mint nothing.
    peppers
        .probe(aex_identity_app::ports::PepperPurpose::Identity)
        .await
        .map_err(|error| {
            CentralIdentityApiRunError::Dependency("identity-pepper", error.to_string())
        })?;

    // Probe three: Google's sign-in OAuth client loads and parses. Without it
    // `dashboard_session_create` can complete no handshake, and a browser
    // session is the only thing that can approve a device authorization — so a
    // process that serves without them answers the whole credential ceremony's
    // second step with a `500` for its entire life. Refusing here turns that
    // into one start-up line naming the dependency.
    let google = oauth::load_oauth_client(&secrets, &config.google_oauth_secret_id)
        .await
        .map_err(|reason| CentralIdentityApiRunError::Dependency("google-oauth-client", reason))?;
    // The handshake is bounded by the same deadline the request it serves is,
    // so a provider that stops answering can never outlive its own request.
    let handshake = Arc::new(
        oauth::HttpProviderHandshake::new(
            google,
            config.sign_in_redirect_uri.clone(),
            config.http.request_deadline,
        )
        .map_err(|error| {
            CentralIdentityApiRunError::Dependency("provider-handshake", error.to_string())
        })?,
    );

    let clock: Arc<dyn aex_identity_app::ports::Clock> = Arc::new(aex_central_aws::SystemClock);
    let account_client = client.clone();
    let store = Arc::new(aex_identity_aurora::AuroraIdentityStore::new(
        client,
        Arc::clone(&peppers) as Arc<dyn aex_identity_app::ports::PepperKeystore>,
    ));
    let verification_uri = aex_wire::types::HttpsUrl::parse(&config.device_verification_uri)
        .map_err(|error| {
            CentralIdentityApiRunError::Dependency("device-verification-uri", error.to_string())
        })?;
    let api = Arc::new(api::AuthService::new(
        store,
        Arc::clone(&peppers) as Arc<dyn aex_identity_app::ports::PepperKeystore>,
        Arc::clone(&clock),
        Arc::new(aex_central_aws::Uuid7Factory),
        Arc::new(aex_central_aws::OsSecretRng),
        verification_uri,
        handshake,
    ));

    // The cursor secret is per-process and never leaves it: this deployable
    // mounts no paged route, so a cursor it minted could only be redeemed by
    // itself, and a configured shared secret would be one more credential to
    // hold for no reader.
    let mut cursor_bytes = [0_u8; 32];
    aex_identity_domain::SecretRng::fill(&aex_central_aws::OsSecretRng, &mut cursor_bytes);
    let edge = EdgeStack::new(
        config.http.clone(),
        Arc::new(targets::NoOrganizationTargets),
        clock,
        Arc::new(aex_control_domain::CursorSecret::new(cursor_bytes)),
    );

    // The account read, over the two statements this login role is granted.
    // It shares the cluster client the device flow already opened, so mounting
    // it costs one more port and no additional connection.
    let account = Arc::new(central_identity_api::account::AccountService::new(
        Arc::new(central_identity_api::account::AuroraAccountReader::new(
            account_client,
        )),
    ));

    run(config, api, account, edge, Probes::READY, telemetry).await
}

/// The one-column `SELECT 1` the readiness probe issues.
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
            eprintln!("central-identity-api: refusing to start: {error}");
            eprintln!(
                "central-identity-api: required configuration: {}",
                keys::ALL.join(", ")
            );
            return std::process::ExitCode::FAILURE;
        }
    };
    let outcome = compose(&config, &telemetry).await;
    if let aex_platform_telemetry::FlushOutcome::DeadlineExceeded { pending } =
        telemetry.flush(settings.flush_deadline)
    {
        eprintln!("central-identity-api: telemetry flush left {pending} record(s) undelivered");
    }
    match outcome {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("central-identity-api: stopped: {error}");
            std::process::ExitCode::FAILURE
        }
    }
}
