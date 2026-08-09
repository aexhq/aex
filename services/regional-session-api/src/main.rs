//! `regional-session-api` composition root (Rust Fargate OCI).
//!
//! Exclusive responsibility: the finite session, run, operation, registry,
//! upload, content-metadata, approval, secret-metadata and usage routes.
//!
//! The binary is a composition root only: it validates configuration, resolves
//! its key material, builds the real adapters, assembles the router from the
//! generated route table, and serves it on a bound listener. Behaviour lives in
//! the library crates it composes.
//!
//! Every start-up failure stops the process. A regional edge that cannot verify
//! an assertion, or cannot sign a continuation, must not serve: the alternative
//! is a listener that answers a permanent failure on every route it advertises.
//!
//! The process binds its listener only after configuration and key material are
//! admitted, serves `/internal/healthz` and `/internal/readyz` (the ALB
//! target-group check), and on `SIGTERM` flips readiness to `503` first so the
//! load balancer deregisters before the drain deadline runs.

use std::future::IntoFuture as _;
use std::net::{Ipv6Addr, SocketAddr};
use std::process::ExitCode;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use aex_internal_contracts::assertion::AssertionAudience;
use aex_regional_http::assertion::AuthFailure;
use aex_regional_http::authz::{
    LambdaAssertionSource, ParameterStore, RegionalProjection, TrustError,
};
use aex_regional_http::config::RegionalHttpConfigError;
use aex_regional_http::edge::{EdgeBinding, RegionalEdge, SystemClock};
use aex_regional_http::health::{Readiness, ReadinessError};
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
enum RegionalSessionApiRunError {
    /// Start-up configuration was rejected.
    #[error(transparent)]
    Config(#[from] RegionalHttpConfigError),
    /// Start-up key material was rejected.
    #[error(transparent)]
    Trust(#[from] TrustError),
    /// The edge could not be composed over its resolved inputs.
    #[error("the request edge could not be composed: {0}")]
    Edge(AuthFailure),
    /// The served route set could not be mounted.
    #[error(transparent)]
    Mount(#[from] MountError),
    /// The readiness projection could not be built from the resolved stores.
    #[error(transparent)]
    Readiness(#[from] ReadinessError),
    /// The listener could not be bound, served or drained.
    #[error("the session listener stopped: {0}")]
    Listener(String),
}

#[tokio::main]
async fn main() -> ExitCode {
    // Telemetry first: a configuration refusal must reach the wire, or a
    // crash-looping deployment is visible only to whoever tails stderr. The
    // long-lived profile replaces the Lambda default, which flushes on an
    // invocation boundary this process does not have. The pump refusing to
    // spawn is the one failure that stays stderr-only, because there is no
    // installed exporter to carry it yet.
    let telemetry = match aex_platform_telemetry::LongLivedTelemetry::install() {
        Ok(telemetry) => telemetry,
        Err(error) => {
            eprintln!("regional-session-api: refusing to start: {error}");
            return ExitCode::FAILURE;
        }
    };
    let config = match Config::from_env() {
        Ok(config) => config,
        Err(error) => {
            telemetry.handle().emit(
                aex_platform_telemetry::Record::event(
                    aex_telemetry_schema::generated::EVENT_AEX_PROCESS_CONFIGURATION_REJECTED,
                )
                .with(
                    aex_telemetry_schema::generated::AEX_DEPLOYABLE,
                    "regional-session-api",
                ),
            );
            let _ = telemetry.shutdown();
            eprintln!("regional-session-api: refusing to start: {error}");
            return ExitCode::FAILURE;
        }
    };
    let outcome = Box::pin(run(&config, telemetry.handle())).await;
    if let aex_platform_telemetry::FlushOutcome::DeadlineExceeded { pending } = telemetry.shutdown()
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

/// Builds the real adapters, assembles the router, binds the listener and
/// serves it until the drain completes.
async fn run(
    config: &Config,
    telemetry: &aex_platform_telemetry::Handle,
) -> Result<(), RegionalSessionApiRunError> {
    telemetry.emit(
        aex_platform_telemetry::Record::event(
            aex_telemetry_schema::generated::EVENT_AEX_PROCESS_STARTED,
        )
        .with(
            aex_telemetry_schema::generated::AEX_PLANE,
            config.plane.as_str().to_owned(),
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

    // One flag, read by both readiness surfaces. It is raised before the
    // listener is asked to stop, so `/internal/readyz` answers `503` while the
    // ALB is still deregistering this target and in-flight requests finish.
    let draining = Arc::new(AtomicBool::new(false));
    let stores = regional_session_api::Stores::build(config, &dynamodb, &objects);
    let readiness = match stores.unresolved() {
        unresolved if unresolved.is_empty() => Readiness::ready(config.release_digest.clone()),
        unresolved => Readiness::not_ready(config.release_digest.clone(), unresolved)?,
    }
    .with_drain_signal(Arc::clone(&draining));

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
        custody_table: stores.custody.table().to_owned(),
        registry: Arc::new(stores.registry.clone()),
        sessions: Arc::new(aex_session_dynamodb::store::SessionReads::new(
            dynamodb.clone(),
            stores.session_table.clone(),
        )),
        operations: Arc::new(aex_session_dynamodb::store::OperationStore::new(
            dynamodb.clone(),
            stores.session_table.clone(),
        )),
        cursor_keys: Arc::new(cursor_keys),
    }));
    let mounted = mount_unary(Arc::new(dispatcher), Arc::new(edge), limits(config))?;

    // The health surfaces are merged rather than layered: they are not generated
    // routes and must answer without an assertion. The public release identity
    // is mounted only by this deployable; the internal paths remain private.
    let router = mounted
        .router
        .merge(aex_regional_http::health::router(readiness.clone()))
        .merge(aex_regional_http::release_health::router(readiness));

    let address = SocketAddr::from((Ipv6Addr::UNSPECIFIED, config.port));
    let listener = tokio::net::TcpListener::bind(address)
        .await
        .map_err(|error| {
            RegionalSessionApiRunError::Listener(format!("cannot bind {address}: {error}"))
        })?;
    eprintln!(
        "regional-session-api: listening on {address} routes={}",
        mounted.routes.len()
    );

    let deadline = Duration::from_millis(config.drain_deadline_ms);
    let (shutdown_tx, mut shutdown_rx) = tokio::sync::watch::channel(false);
    let server = axum::serve(listener, router)
        .with_graceful_shutdown(async move {
            while !*shutdown_rx.borrow() && shutdown_rx.changed().await.is_ok() {}
        })
        .into_future();
    tokio::pin!(server);
    tokio::select! {
        outcome = &mut server => outcome.map_err(|error| RegionalSessionApiRunError::Listener(error.to_string())),
        () = wait_for_termination() => {
            // Readiness flips before the listener is asked to stop: new requests
            // get 503 and the ALB deregisters this target while the requests
            // already accepted run to completion inside the drain deadline.
            draining.store(true, Ordering::Release);
            let _ = shutdown_tx.send(true);
            eprintln!("regional-session-api: draining, deadline {} ms", deadline.as_millis());
            match tokio::time::timeout(deadline, &mut server).await {
                Ok(outcome) => outcome.map_err(|error| RegionalSessionApiRunError::Listener(error.to_string())),
                Err(_) => Err(RegionalSessionApiRunError::Listener(format!(
                    "drain deadline of {} ms expired",
                    deadline.as_millis()
                ))),
            }
        }
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

/// Builds the shared regional edge over its three resolved inputs.
type Edge = RegionalEdge<
    LambdaAssertionSource,
    RegionalProjection<aex_session_dynamodb::projection::ProjectionReader>,
    SystemClock,
>;

fn build_edge(
    config: &Config,
    aws: &aws_config::SdkConfig,
    dynamodb: &aws_sdk_dynamodb::Client,
    anchors: aex_identity_domain::assertion::VerificationKeySet,
) -> Result<Edge, RegionalSessionApiRunError> {
    let projection = aex_session_dynamodb::projection::ProjectionReader::new(
        dynamodb.clone(),
        config.authz_projection_table.clone(),
    );
    RegionalEdge::new(
        LambdaAssertionSource::new(
            aws_sdk_lambda::Client::new(aws),
            config.authz_function.value.clone(),
            AUDIENCE,
            config.region,
        ),
        anchors,
        RegionalProjection::new(projection, config.region),
        SystemClock,
        EdgeBinding {
            plane: config.plane,
            audience: AUDIENCE,
            region: config.region,
            cache_budget_bytes: config.assertion_cache_bytes,
        },
    )
    .map_err(RegionalSessionApiRunError::Edge)
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
