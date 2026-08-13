//! `finance-api` composition root (Rust Lambda ZIP).
//!
//! This binary parses typed configuration, builds the real adapters, wires them
//! into the generated `central:billing` server trait, runs one bounded
//! readiness probe through its own database role, and serves. There is no
//! business logic here.

use std::sync::Arc;

use aex_rds_data::{AwsTransport, DataApiClient};
use aex_wire::dispatch::RequestLimits;
use aex_wire::types::HttpsUrl;
use finance_api::aurora::AuroraBillingAuthority;
use finance_api::authority::BillingAuthority as _;
use finance_api::billing::BillingService;
use finance_api::config::Config;
use finance_api::download::S3StatementDownloads;
use finance_api::edge::UnresolvedPrincipalEdge;
use finance_api::gateway::CommandEdgeGateway;
use finance_api::health::Readiness;
use finance_api::mount::{AppState, app};

/// The identity this deployable reports in every record.
const DEPLOYABLE: &str = "finance-api";

/// Where a hosted provider page returns a caller that named no URL.
///
/// A constant rather than a variable: it is a public product URL, not a
/// resource identity, and letting an environment override it would let a
/// misconfiguration redirect a paying customer somewhere else.
const DEFAULT_RETURN_URL: &str = "https://aex.dev/dashboard/billing";

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

/// Builds every adapter and serves until the runtime stops.
async fn run(config: Config) -> Result<(), lambda_http::Error> {
    let aws = aws_config::load_defaults(aws_config::BehaviorVersion::latest()).await;

    let transport = AwsTransport::new(aws_sdk_rdsdata::Client::new(&aws), &config.data_api);
    let client = Arc::new(DataApiClient::new(
        Arc::new(transport),
        config.data_api.clone(),
    ));
    let authority = Arc::new(AuroraBillingAuthority::new(
        Arc::clone(&client),
        config.database_role.clone(),
    ));
    let gateway = Arc::new(CommandEdgeGateway::new(
        aws_sdk_lambda::Client::new(&aws),
        config.command_edge_arn.clone(),
    ));
    let downloads = Arc::new(S3StatementDownloads::new(
        aws_sdk_s3::Client::new(&aws),
        config.statement_bucket.clone(),
    ));

    let default_return_url = HttpsUrl::parse(DEFAULT_RETURN_URL)
        .map_err(|error| lambda_http::Error::from(error.to_string()))?;
    let api = Arc::new(BillingService::new(
        Arc::clone(&authority),
        gateway,
        downloads,
        config.page_limit,
        i64::try_from(config.download_grant_ttl.as_millis()).unwrap_or(i64::MAX),
        default_return_url,
    ));

    // Readiness is a grant fact. The probe runs once at init and the gate stays
    // closed if this deployable cannot prove its own role, so a misconfigured
    // secret produces an unready instance rather than a money surface that
    // fails at its first write.
    let readiness = Arc::new(Readiness::pending());
    match authority.probe_role().await {
        Ok(()) => readiness.hold(),
        Err(error) => readiness.release(error.to_string()),
    }

    let state = AppState {
        edge: Arc::new(UnresolvedPrincipalEdge::new(RequestLimits::DEFAULT)),
        api,
        readiness,
    };
    lambda_http::run(app(state)).await
}
