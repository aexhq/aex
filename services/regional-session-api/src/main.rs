//! `regional-session-api` composition root (Rust Lambda ZIP).
//!
//! Exclusive responsibility: the finite session, run, operation, registry,
//! upload, content-metadata, approval, secret-metadata and usage routes.
//!
//! The binary is a composition root only: it validates configuration, resolves
//! its key material, builds the real adapters, assembles the router from the
//! generated route table, and hands it to `lambda_http`. Behaviour lives in the
//! library crates it composes.
//!
//! Every start-up failure stops the process. A regional edge that cannot verify
//! an assertion, or cannot sign a continuation, must not serve: the alternative
//! is a listener that answers a permanent failure on every route it advertises.

use std::process::ExitCode;
use std::sync::Arc;

use aex_internal_contracts::assertion::AssertionAudience;
use aex_regional_http::assertion::AuthFailure;
use aex_regional_http::authz::{
    Ed25519Anchors, LambdaAssertionSource, ParameterStore, RegionalProjection, TrustError,
};
use aex_regional_http::config::ConfigError;
use aex_regional_http::edge::{EdgeBinding, RegionalEdge, SystemClock};
use aex_regional_http::health::Readiness;
use aex_regional_http::mount::{MountError, mount_unary};
use aex_wire::dispatch::RequestLimits;
use regional_session_api::config::Config;
use regional_session_api::handlers::{Dispatcher, Shared};

/// The one audience this deployable accepts.
///
/// An assertion minted for the secret edge must never admit a request here, and
/// the reverse. Asking `central-authz` for the right audience is cheaper than
/// checking afterwards, and the verifier checks it again regardless.
const AUDIENCE: AssertionAudience = AssertionAudience::RegionalSession;

/// Why `regional-session-api` stopped.
#[derive(Debug, thiserror::Error)]
enum RunError {
    /// Start-up configuration was rejected.
    #[error(transparent)]
    Config(#[from] ConfigError),
    /// Start-up key material was rejected.
    #[error(transparent)]
    Trust(#[from] TrustError),
    /// The edge could not be composed over its resolved inputs.
    #[error("the request edge could not be composed: {0}")]
    Edge(AuthFailure),
    /// The served route set could not be mounted.
    #[error(transparent)]
    Mount(#[from] MountError),
    /// The `HTTP` runtime stopped.
    #[error("the lambda runtime stopped: {0}")]
    Runtime(String),
}

#[tokio::main]
async fn main() -> ExitCode {
    let config = match Config::from_env() {
        Ok(config) => config,
        Err(error) => {
            eprintln!("regional-session-api: refusing to start: {error}");
            return ExitCode::FAILURE;
        }
    };
    let settings = aex_platform_telemetry::Settings::default();
    let telemetry = aex_platform_telemetry::Handle::install(&settings, None);
    let outcome = run(&config, &telemetry).await;
    if let aex_platform_telemetry::FlushOutcome::DeadlineExceeded { pending } =
        telemetry.flush(settings.flush_deadline)
    {
        eprintln!("regional-session-api: telemetry flush left {pending} record(s) undelivered");
    }
    match outcome {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("regional-session-api: stopped: {error}");
            ExitCode::FAILURE
        }
    }
}

/// Builds the real adapters, assembles the router and serves it.
async fn run(config: &Config, telemetry: &aex_platform_telemetry::Handle) -> Result<(), RunError> {
    telemetry.emit(
        aex_platform_telemetry::Record::event(
            aex_telemetry_schema::generated::EVENT_AEX_PROCESS_STARTED,
        )
        .with(
            aex_telemetry_schema::generated::AEX_PLANE,
            config.plane.clone(),
        )
        .with(
            aex_telemetry_schema::generated::AEX_REGION,
            config.region.as_str().to_owned(),
        ),
    );

    let aws = aws_config::load_defaults(aws_config::BehaviorVersion::latest()).await;
    let dynamodb = aws_sdk_dynamodb::Client::new(&aws);
    let objects = aws_sdk_s3::Client::new(&aws);
    let parameters = ParameterStore::new(aws_sdk_ssm::Client::new(&aws));

    let stores = regional_session_api::Stores::build(config, &dynamodb, &objects);
    let readiness = match stores.unresolved() {
        unresolved if unresolved.is_empty() => Readiness::ready(config.release_digest.clone()),
        unresolved => Readiness::not_ready(config.release_digest.clone(), unresolved)
            .map_err(|error| RunError::Runtime(error.to_string()))?,
    };

    // Both reads happen once, here, before the listener binds. Neither is on a
    // request path and neither has a fallback: an unreadable trust anchor set
    // means this process can verify nothing, and an unreadable signing ring
    // means it can issue no continuation a later request could redeem.
    let anchors = parameters
        .trust_anchors(&config.authz_verify_keys_param)
        .await?;
    let cursor_keys = parameters
        .cursor_key_ring(&config.cursor_signing_key_ref)
        .await?;

    let edge = build_edge(config, &aws, &dynamodb, anchors)?;
    let dispatcher = Dispatcher::new(Arc::new(Shared {
        custody: Arc::new(stores.custody.clone()),
        registry: Arc::new(stores.registry.clone()),
        cursor_keys: Arc::new(cursor_keys),
    }));
    let mounted = mount_unary(Arc::new(dispatcher), Arc::new(edge), limits(config))?;

    // The health surface is merged rather than layered: `/internal/healthz` and
    // `/internal/readyz` are not generated routes and must answer without an
    // assertion, which is exactly why they are not in the mounted partition.
    lambda_http::run(
        mounted
            .router
            .merge(aex_regional_http::health::router(readiness)),
    )
    .await
    .map_err(|error| RunError::Runtime(error.to_string()))
}

/// Builds the shared regional edge over its four resolved inputs.
type Edge = RegionalEdge<
    LambdaAssertionSource,
    Ed25519Anchors,
    RegionalProjection<aex_session_dynamodb::projection::ProjectionReader>,
    SystemClock,
>;

fn build_edge(
    config: &Config,
    aws: &aws_config::SdkConfig,
    dynamodb: &aws_sdk_dynamodb::Client,
    anchors: Ed25519Anchors,
) -> Result<Edge, RunError> {
    RegionalEdge::new(
        LambdaAssertionSource::new(
            aws_sdk_lambda::Client::new(aws),
            config.authz_function.value.clone(),
            AUDIENCE,
            config.region,
        ),
        anchors,
        RegionalProjection::new(
            aex_session_dynamodb::projection::ProjectionReader::new(
                dynamodb.clone(),
                config.authz_projection_table.clone(),
            ),
            config.region,
        ),
        SystemClock,
        EdgeBinding {
            audience: AUDIENCE,
            region: config.region,
            cache_budget_bytes: config.assertion_cache_bytes,
            limits: config.limits(),
        },
    )
    .map_err(RunError::Edge)
}

/// The decode bounds the generated dispatchers enforce.
///
/// This deployable owns no OTLP route, so its OTLP bound is the contract default
/// and is unreachable rather than configurable: a second knob nothing reads is a
/// knob that will eventually be set wrong.
fn limits(config: &Config) -> RequestLimits {
    RequestLimits {
        max_json_body_bytes: config.max_json_body_bytes,
        max_otlp_body_bytes: RequestLimits::DEFAULT_OTLP_BODY_BYTES,
    }
}
