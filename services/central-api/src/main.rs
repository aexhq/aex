//! `central-api` composition root (Rust Fargate OCI).
//!
//! Exclusive responsibility: the twenty-six mounted central operations. One
//! process, one listener, one drain flag, one Aurora login.
//!
//! The binary is a composition root only: it installs diagnostics, validates
//! configuration, admits its capability manifest, builds the real adapters,
//! proves the shared start-up dependencies answer **before** the listener binds, assembles the
//! router from the generated route table, and serves it until the drain
//! completes. Behaviour lives in the library crates it composes.
//!
//! Every start-up failure stops the process. A central edge that cannot verify a
//! credential must not serve: behind a load balancer the alternative is a
//! listener that either refuses every authenticated request or — far worse —
//! admits one it could not check.
//!
//! On `SIGTERM` the drain flag is raised **first**, so `/internal/readyz`
//! answers `503` and the ALB deregisters this target while in-flight requests
//! finish; only then is the listener asked to stop, bounded by a drain deadline
//! that `aex_regional_http::drain` already pins below the ECS stop timeout.

use std::future::IntoFuture as _;
use std::net::{Ipv6Addr, SocketAddr};
use std::process::ExitCode;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use aex_central_http::admission::{CredentialAdmission, PurposedPeppers};
use aex_central_http::router::EdgeStack;
use aex_identity_app::ports::{PepperKeystore, PepperPurpose};
use aex_rds_data::{
    AwsTransport, DataApiClient, DataApiConfig, DatabaseName, ResourceArn, SecretArn, Statement,
};
use central_api::config::Config;
use central_api::{CentralApiRunError, DEPLOYABLE, Probes, app, manifest, readiness};

/// Where a hosted provider page returns a caller that named no URL.
///
/// A constant rather than a variable, exactly as `finance-api` has it: it is a
/// public product URL, not a resource identity, and letting an environment
/// override it would let a misconfiguration redirect a paying customer
/// somewhere else.
const DEFAULT_RETURN_URL: &str = "https://aex.dev/dashboard/billing";

#[tokio::main]
async fn main() -> ExitCode {
    if let Err(error) = aex_platform_diagnostics::install_json() {
        eprintln!("{}: refusing to start: {error}", DEPLOYABLE.as_str());
        return ExitCode::FAILURE;
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
            eprintln!("{}: refusing to start: {error}", DEPLOYABLE.as_str());
            eprintln!(
                "{}: required configuration: {}",
                DEPLOYABLE.as_str(),
                central_api::config::ALL.join(", ")
            );
            return ExitCode::FAILURE;
        }
    };
    let outcome = Box::pin(run(&config)).await;
    match outcome {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{}: stopped: {error}", DEPLOYABLE.as_str());
            ExitCode::FAILURE
        }
    }
}

/// Builds the real adapters, proves each one answers, binds one listener and
/// serves it until the drain completes.
#[allow(
    clippy::too_many_lines,
    reason = "the composition root deliberately keeps every login, start-up dependency, service and drain binding visible in one audit surface"
)]
async fn run(config: &Config) -> Result<(), CentralApiRunError> {
    // Before any client is opened: the declared capabilities and the configured
    // resources must agree.
    aex_central_http::capability::admit(&manifest(), &config.resolved())?;
    tracing::info!(
        target: "aex::diagnostics",
        event_name = "process.started",
        deployable = DEPLOYABLE.as_str(),
        plane = config.http.plane.as_str(),
        region = config.http.region.as_str(),
        "process started"
    );

    let aws = aws_config::load_defaults(aws_config::BehaviorVersion::latest()).await;
    let rds = aws_sdk_rdsdata::Client::new(&aws);
    let secrets = aws_sdk_secretsmanager::Client::new(&aws);

    // --- one login, one cluster ------------------------------------------------
    //
    // The same connection `central-control-api`, `central-identity-api` and
    // `finance-api` each open: the cluster's managed login, reaching every
    // central schema. Per-schema `PostgreSQL` roles are deferred rather than
    // lost — `references/backlog.md` carries the row. Nothing here is what
    // keeps one tenant out of another's rows; that is the application's
    // organization scoping and it is unchanged.
    let cluster = ResourceArn::parse(&config.aurora_cluster_arn)
        .map_err(|error| CentralApiRunError::Dependency("aurora", error.to_string()))?;
    let database = DatabaseName::parse(&config.database)
        .map_err(|error| CentralApiRunError::Dependency("aurora", error.to_string()))?;
    let aurora = login(&rds, &cluster, &database, &config.aurora_secret_arn)?;

    // --- probe the login before the listener binds ------------------------------
    probe_login(&aurora, "aurora").await?;

    // --- the two peppers, each through the directory that holds its versions ---
    let control_directory = Arc::new(aex_central_aws::DataApiPepperDirectory::new(
        aurora.clone(),
        aex_central_aws::PepperStatements {
            active: aex_control_aurora::sql::ACTIVE_CONTROL_PEPPER,
            by_version: aex_control_aurora::sql::CONTROL_PEPPER_BY_VERSION,
            verification_set: Some(aex_control_aurora::sql::LIVE_CONTROL_PEPPERS),
        },
    ));
    let api_peppers = Arc::new(aex_central_aws::SecretsManagerPepperKeystore::new(
        secrets.clone(),
        config.api_key_pepper_secret_id.clone(),
        Arc::clone(&control_directory) as Arc<dyn aex_central_aws::PepperDirectory>,
    ));
    api_peppers
        .probe(PepperPurpose::ApiKey)
        .await
        .map_err(|error| CentralApiRunError::Dependency("api-key-pepper", error.to_string()))?;

    let identity_peppers = Arc::new(aex_central_aws::SecretsManagerPepperKeystore::new(
        secrets.clone(),
        config.identity_pepper_secret_id.clone(),
        Arc::new(aex_central_aws::DataApiPepperDirectory::new(
            aurora.clone(),
            aex_central_aws::PepperStatements {
                active: aex_identity_aurora::sql::ACTIVE_IDENTITY_PEPPER,
                by_version: aex_identity_aurora::sql::IDENTITY_PEPPER_BY_VERSION,
                verification_set: None,
            },
        )),
    ));
    identity_peppers
        .probe(PepperPurpose::Identity)
        .await
        .map_err(|error| CentralApiRunError::Dependency("identity-pepper", error.to_string()))?;

    // The registered Google OAuth client is a startup dependency: serving
    // without it would make the browser sign-in exchange fail for this process.
    let google =
        central_api::identity_oauth::load_oauth_client(&secrets, &config.google_oauth_secret_id)
            .await
            .map_err(|reason| CentralApiRunError::Dependency("google-oauth-client", reason))?;
    // The handshake is bounded by the same deadline the request it serves is, so
    // a provider that stops answering can never outlive its own request.
    let handshake = Arc::new(
        central_api::identity_oauth::HttpProviderHandshake::new(
            google,
            config.sign_in_redirect_uri.clone(),
            config.http.request_deadline,
        )
        .map_err(|error| CentralApiRunError::Dependency("provider-handshake", error.to_string()))?,
    );

    let cursor_peppers = Arc::new(aex_central_aws::SecretsManagerPepperKeystore::new(
        secrets,
        config.cursor_secret_id.clone(),
        control_directory,
    ));
    let cursor_keys = cursor_peppers
        .verification_set(PepperPurpose::Cursor)
        .await
        .map_err(|error| CentralApiRunError::Dependency("cursor-secret", error.to_string()))?;
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
        .map_err(|error| CentralApiRunError::Dependency("cursor-secret", error.to_string()))?,
    );

    // --- the three services ----------------------------------------------------
    let clock: Arc<dyn aex_identity_app::ports::Clock> = Arc::new(aex_central_aws::SystemClock);

    let control_store = Arc::new(aex_control_aurora::AuroraControlStore::new(aurora.clone()));
    let control = Arc::new(central_api::control::ControlService::new(
        control_store.clone() as Arc<dyn central_api::control::Store>,
        Arc::clone(&api_peppers) as Arc<dyn PepperKeystore>,
        Arc::clone(&clock),
        Arc::new(aex_central_aws::Uuid7Factory),
        Arc::new(aex_central_aws::OsSecretRng),
        Arc::clone(&cursor_secret),
        config.http.region,
        config.api_urls.clone(),
    ));

    let auth = Arc::new(central_api::identity::AuthService::new(
        Arc::new(aex_identity_aurora::AuroraIdentityStore::new(
            aurora.clone(),
            Arc::clone(&identity_peppers) as Arc<dyn PepperKeystore>,
        )),
        control_store.clone() as Arc<dyn aex_control_app::PersonalAccountProvisioner>,
        Arc::clone(&identity_peppers) as Arc<dyn PepperKeystore>,
        Arc::clone(&clock),
        Arc::new(aex_central_aws::Uuid7Factory),
        Arc::new(aex_central_aws::OsSecretRng),
        handshake,
    ));

    let billing_authority = Arc::new(central_api::billing::aurora::AuroraBillingAuthority::new(
        Arc::new(aurora.clone()),
        central_api::billing::REQUIRED_ROLE.to_owned(),
    ));
    // The prelaunch composition intentionally uses the cluster's one managed
    // login. `finance-api`'s per-role membership probe is correct for its
    // standalone role-scoped login, but requiring it here contradicts that
    // topology and makes the first shared-login task unable to start. The
    // common Aurora login has already answered its real connection probe above.
    let billing = Arc::new(central_api::billing::service::BillingService::new(
        Arc::clone(&billing_authority),
        Arc::new(central_api::billing::gateway::CommandEdgeGateway::new(
            aws_sdk_lambda::Client::new(&aws),
            config.stripe_command_edge_arn.clone(),
        )),
        config.page_limit,
        aex_wire::types::HttpsUrl::parse(DEFAULT_RETURN_URL).map_err(|error| {
            CentralApiRunError::Dependency("default-return-url", error.to_string())
        })?,
    ));

    // --- the edge, verifying credentials in this process -----------------------
    //
    // `with_in_process_authentication` is exclusive with the gateway path by
    // enum. That exclusivity is the whole safety property behind a load
    // balancer: a stack that both verified a credential *and* honoured an
    // ambient context map would be one whose verification a caller skips by
    // asserting the principal it wants.
    let authenticator = Arc::new(CredentialAdmission::new(
        aex_control_aurora::AuroraAuthorizationReader::new(aurora.clone()),
        PurposedPeppers::new(
            Arc::clone(&identity_peppers) as Arc<dyn PepperKeystore>,
            Arc::clone(&api_peppers) as Arc<dyn PepperKeystore>,
        ),
        config.context_lifetime_ms,
    ));
    let edge = EdgeStack::new(
        config.http.clone(),
        Arc::new(aex_central_http::target::ControlStoreTargets::new(
            control_store as Arc<dyn aex_control_app::ports::ControlStore>,
        )),
        clock,
        cursor_secret,
    )
    .with_in_process_authentication(authenticator);

    // One flag for the whole process, read by both probes. It is raised before
    // the listener is asked to stop, so `/internal/readyz` answers `503` while
    // the ALB is still deregistering this target and the requests it already
    // accepted finish.
    let draining = Arc::new(AtomicBool::new(false));
    let readiness = readiness(Probes::READY).with_drain_signal(Arc::clone(&draining));
    let router = app(control, auth, billing, edge, readiness);

    let address = SocketAddr::from((Ipv6Addr::UNSPECIFIED, config.port));
    let listener = tokio::net::TcpListener::bind(address)
        .await
        .map_err(|error| CentralApiRunError::Listener(format!("cannot bind {address}: {error}")))?;
    eprintln!(
        "{}: listening on {address} routes={}",
        DEPLOYABLE.as_str(),
        DEPLOYABLE.routes().len()
    );

    let deadline = config.drain_deadline();
    let (shutdown_tx, mut shutdown_rx) = tokio::sync::watch::channel(false);
    let server = axum::serve(listener, router)
        .with_graceful_shutdown(async move {
            while !*shutdown_rx.borrow() && shutdown_rx.changed().await.is_ok() {}
        })
        .into_future();
    tokio::pin!(server);
    tokio::select! {
        outcome = &mut server => outcome.map_err(|error| CentralApiRunError::Listener(error.to_string())),
        () = wait_for_termination() => {
            // Readiness flips before the listener is asked to stop: the ALB
            // deregisters this target on the next `/internal/readyz` poll while
            // in-flight requests run to completion. The deadline is bounded at
            // configuration time by `aex_regional_http::drain`, so it fires
            // before `SIGKILL` rather than being an arithmetic certainty to
            // overrun — an overrun here is therefore a real signal.
            draining.store(true, Ordering::Release);
            let _ = shutdown_tx.send(true);
            eprintln!("{}: draining, deadline {} ms", DEPLOYABLE.as_str(), deadline.as_millis());
            match tokio::time::timeout(deadline, &mut server).await {
                Ok(outcome) => outcome.map_err(|error| CentralApiRunError::Listener(error.to_string())),
                Err(_) => Err(CentralApiRunError::Listener(format!(
                    "drain deadline of {} ms expired",
                    deadline.as_millis()
                ))),
            }
        }
    }
}

/// One Data API client for one login secret against the shared cluster.
fn login(
    rds: &aws_sdk_rdsdata::Client,
    cluster: &ResourceArn,
    database: &DatabaseName,
    secret_arn: &str,
) -> Result<DataApiClient, CentralApiRunError> {
    let secret = SecretArn::parse(secret_arn)
        .map_err(|error| CentralApiRunError::Dependency("aurora", error.to_string()))?;
    let data_api = DataApiConfig::new(cluster.clone(), secret, database.clone());
    Ok(DataApiClient::new(
        Arc::new(AwsTransport::new(rds.clone(), &data_api)),
        data_api,
    ))
}

/// The cheapest statement that proves a login resolves and its role exists.
async fn probe_login(client: &DataApiClient, name: &'static str) -> Result<(), CentralApiRunError> {
    client
        .query::<Ok1>(Statement::new(aex_control_aurora::sql::READINESS_PROBE))
        .await
        .map(|_| ())
        .map_err(|error| CentralApiRunError::Dependency(name, error.to_string()))
}

/// The one-column `SELECT 1` the readiness probes issue.
struct Ok1;

impl aex_rds_data::Row for Ok1 {
    fn from_record(record: &aex_rds_data::Record<'_>) -> Result<Self, aex_rds_data::DecodeError> {
        record.expect_arity(1)?;
        record.i64(0)?;
        Ok(Self)
    }
}

/// Waits for the container runtime's stop signal.
async fn wait_for_termination() {
    #[cfg(unix)]
    {
        let Ok(mut terminate) =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        else {
            return std::future::pending().await;
        };
        tokio::select! {
            _ = terminate.recv() => {}
            _ = tokio::signal::ctrl_c() => {}
        }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
}
