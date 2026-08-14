//! `session-stream-api` composition root (Rust Fargate OCI).
//!
//! Exclusive responsibility: the finite session, run, operation, registry,
//! upload, content-metadata, approval, secret-metadata and usage routes.
//!
//! The binary is a composition root only: it validates configuration, admits its
//! capability manifest, resolves its key material, builds the real adapters,
//! assembles both routers from the generated route table, and serves them on one
//! bound listener. Behaviour lives in the library crates it composes.
//!
//! Every start-up failure stops the process. A regional edge that cannot verify
//! an assertion, or cannot sign a continuation, must not serve: the alternative
//! is a listener that answers a permanent failure on every route it advertises.
//!
//! The process binds its listener only after configuration and key material are
//! admitted, serves `/internal/healthz` and `/internal/readyz` (the ALB
//! target-group check) **once**, from the shared health router, and on `SIGTERM`
//! flips readiness to `503` first so the load balancer deregisters before the
//! drain deadline runs.

use std::future::IntoFuture as _;
use std::net::{Ipv6Addr, SocketAddr};
use std::process::ExitCode;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use aex_internal_contracts::assertion::AssertionAudience;
use aex_regional_http::authz::{ParameterStore, RegionalProjection, SecretStore, TrustError};
use aex_regional_http::capability::{CompositionError, Declares, WorkClaim};
use aex_regional_http::config::RegionalHttpConfigError;
use aex_regional_http::edge::{EdgeBinding, RegionalEdge, SystemClock};
use aex_regional_http::health::{Readiness, ReadinessError};
use aex_regional_http::mount::{MountError, mount_unary};
use aex_wire::dispatch::RequestLimits;
use session_stream_api::capability::Composition;
use session_stream_api::config::Config;
use session_stream_api::session::handlers::{Dispatcher, Shared};
use session_stream_api::session::provider_key::SessionProviderKeys;

/// The deployable name every diagnostic record carries.
const DEPLOYABLE: &str = session_stream_api::config::DEPLOYABLE;

/// The audience the finite API accepts.
const SESSION_AUDIENCE: AssertionAudience = AssertionAudience::RegionalSession;

/// The production regional edge shape, once per audience.
type SessionEdge = RegionalEdge<
    RegionalProjection<aex_session_dynamodb::projection::ProjectionReader>,
    SystemClock,
>;

/// Why `session-stream-api` stopped.
#[derive(Debug, thiserror::Error)]
enum SessionStreamApiRunError {
    /// Start-up configuration was rejected.
    #[error(transparent)]
    Config(#[from] RegionalHttpConfigError),
    /// The declared capability manifest did not admit the resolved configuration.
    #[error(transparent)]
    Composition(#[from] CompositionError),
    /// Start-up key material was rejected.
    #[error(transparent)]
    Trust(#[from] TrustError),
    /// The served unary route set could not be mounted.
    #[error(transparent)]
    Mount(#[from] MountError),
    /// The readiness projection could not be built from the resolved stores.
    #[error(transparent)]
    Readiness(#[from] ReadinessError),
    /// A required authority could not be proven before bind.
    #[error("a start-up probe failed: {0}")]
    Probe(String),
    /// The listener could not be bound, served or drained.
    #[error("the listener stopped: {0}")]
    Listener(String),
}

#[tokio::main]
async fn main() -> ExitCode {
    if let Err(error) = aex_platform_diagnostics::install_json() {
        eprintln!("{DEPLOYABLE}: could not install diagnostics: {error}");
        return ExitCode::FAILURE;
    }
    let config = match Config::from_env() {
        Ok(config) => config,
        Err(error) => {
            tracing::error!(
                target: "aex::diagnostics",
                event = "process_configuration_rejected",
                deployable = DEPLOYABLE,
                error = %error,
            );
            eprintln!("{DEPLOYABLE}: refusing to start: {error}");
            return ExitCode::FAILURE;
        }
    };
    let outcome = Box::pin(run(&config)).await;
    match outcome {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{DEPLOYABLE}: stopped: {error}");
            ExitCode::FAILURE
        }
    }
}

/// Builds the real adapters, assembles the router, binds one listener and
/// serves it until the drain completes.
#[allow(
    clippy::too_many_lines,
    reason = "the composition root deliberately keeps every probed authority, edge and drain binding visible in one audit surface"
)]
async fn run(config: &Config) -> Result<(), SessionStreamApiRunError> {
    // Before any client is opened: the declared capabilities and the configured
    // resources must agree before any authority client is opened.
    session_stream_api::capability::admit(config)?;
    let catalog = Arc::new(session_stream_api::release_catalog::ReleaseCatalog);

    tracing::info!(
        target: "aex::diagnostics",
        event = "process_started",
        deployable = DEPLOYABLE,
        plane = config.plane.as_str(),
        region = %config.region,
    );

    let aws = aws_config::load_defaults(aws_config::BehaviorVersion::latest()).await;
    let dynamodb = aws_sdk_dynamodb::Client::new(&aws);
    let kms = aws_sdk_kms::Client::new(&aws);
    let objects = aws_sdk_s3::Client::new(&aws);
    let parameters = ParameterStore::new(aws_sdk_ssm::Client::new(&aws));
    let secrets = SecretStore::new(aws_sdk_secretsmanager::Client::new(&aws));

    // One flag for the whole process, read by readiness. It
    // is raised before the listener is asked to stop, so `/internal/readyz`
    // answers `503` while the ALB is still deregistering this target and the
    // requests already accepted finish.
    let draining = Arc::new(AtomicBool::new(false));

    // The one write handle in the process, and the only place it can be built.
    let stores = session_stream_api::session::Stores::build(
        &<Composition as Declares<WorkClaim>>::grant(),
        config,
        &dynamodb,
        &objects,
    );
    let readiness = match stores.unresolved() {
        unresolved if unresolved.is_empty() => Readiness::ready(config.release_digest.clone()),
        unresolved => Readiness::not_ready(config.release_digest.clone(), unresolved)?,
    }
    .with_drain_signal(Arc::clone(&draining));

    // Both reads happen once, here, before the listener binds. Neither is on a
    // request path and neither has a fallback: an unreadable pepper ring means
    // this process can check no credential, and an unreadable signing ring means
    // it can issue no continuation a later request could redeem.
    let peppers = secrets.pepper_ring(&config.credential_pepper_ref).await?;
    // The finite API signs and verifies every continuation through this ring.
    let cursor_keys = Arc::new(
        parameters
            .cursor_key_ring(&config.cursor_signing_key_ref)
            .await?,
    );

    // The session authority must be readable before bind.
    dynamodb
        .describe_table()
        .table_name(&config.session_table)
        .send()
        .await
        .map_err(|error| {
            SessionStreamApiRunError::Probe(format!("`session-authority` is not readable: {error}"))
        })?;

    // The one production reader of the projection frontier. Nothing read it
    // before, so the bound every cluster relies on — how long a pause, a
    // revocation or a limit change takes to reach this region — was unmeasured.
    // It publishes; it refuses nothing.
    tokio::spawn(session_stream_api::frontier::publish(
        Arc::new(aex_session_dynamodb::projection::ProjectionReader::new(
            dynamodb.clone(),
            stores.authz_projection_table.clone(),
        )),
        session_stream_api::frontier::FrontierResource {
            plane: config.plane.as_str().to_owned(),
            region: config.region.as_str().to_owned(),
            deployable: DEPLOYABLE,
        },
        session_stream_api::frontier::MEASURE_INTERVAL,
        Arc::clone(&draining),
    ));

    let session_edge = build_edge(config, &dynamodb, peppers, SESSION_AUDIENCE);

    // --- the session half's router -------------------------------------------
    // Compiled capacity defaults. When the effective-limits authority lands they
    // override here, at the one check point, rather than growing a second one.
    let defaults = aex_capacity_dynamodb::defaults::canonical_defaults()
        .map_err(|error| SessionStreamApiRunError::Probe(error.to_string()))?;
    let scalar = |id: aex_wire::limits::LimitId| -> Result<u64, SessionStreamApiRunError> {
        defaults
            .value(id)
            .scalar()
            .and_then(|value| u64::try_from(value.get()).ok())
            .ok_or_else(|| {
                SessionStreamApiRunError::Probe(format!("`{}` has no scalar default", id.as_str()))
            })
    };
    let registry_entries = scalar(aex_wire::limits::LimitId::RegistryEntries)?;
    let registry_value_bytes = scalar(aex_wire::limits::LimitId::RegistryValueBytes)?;
    let live_files =
        session_stream_api::session::live_composition::production_live_files(&aws, config)
            .map_err(|error| SessionStreamApiRunError::Probe(error.to_string()))?;
    let provider_keys = Arc::new(SessionProviderKeys::new(
        Arc::new(stores.custody.clone()),
        stores.custody.table().to_owned(),
        Arc::new(aex_secret_aws::crypto::EnvelopeCrypto::new(
            Box::new(aex_secret_aws::keystore::KmsBranchKeys::new(
                kms.clone(),
                config.secret_kms_key.value.clone(),
            )),
            config.crypto_partition(),
        )),
        Arc::new(aex_secret_keystore_dynamodb::LazyBranchKeys::new(
            dynamodb.clone(),
            kms,
            aex_secret_keystore_dynamodb::KeyStoreBinding::new(
                config.session_table.clone(),
                config.secret_kms_key.value.clone(),
            ),
            config.plane.as_str(),
            config.region.as_str(),
        )),
        match config.plane {
            aex_identity_domain::assertion::Plane::Dev => aex_secret_domain::context::Plane::Dev,
            aex_identity_domain::assertion::Plane::Prd => aex_secret_domain::context::Plane::Prd,
        },
        config.region,
    ));
    let dispatcher = Dispatcher::new(Arc::new(Shared {
        catalog,
        deployment: config.deployment.clone(),
        live_files,
        provider_keys,
        registry: Arc::new(stores.registry.clone()),
        content: Arc::new(stores.content.clone()),
        // One presigner for the whole deployable (E D-10). The upload routes,
        // the registry mutation path and the registry download route all sign
        // through this adapter, so the expiry, the encryption context and the
        // bucket-owner assertion have exactly one place to be stated.
        content_objects: Arc::new(stores.content_objects.clone()),
        receipts: Arc::new(stores.registry.clone()),
        registry_table: stores.registry.table().to_owned(),
        work_table: stores.work.table().to_owned(),
        content_kms_key_id: stores.content_objects.binding().kms_key_id.clone(),
        content_encryption_context: content_encryption_context(config)?,
        registry_entries,
        registry_value_bytes,
        sessions: Arc::new(aex_session_dynamodb::store::SessionReads::new(
            dynamodb.clone(),
            stores.session_table.clone(),
        )),
        session_telemetry: stores.session_telemetry.clone(),
        // The cold workspace surface, over the same projection table the edge
        // authorizes against. Two ports over one reader rather than one wide
        // port: a limit read must not be able to reach for a placement, and the
        // admission path must not gain a profile read it never makes.
        workspace: Arc::new(aex_session_dynamodb::projection::ProjectionReader::new(
            dynamodb.clone(),
            stores.authz_projection_table.clone(),
        )),
        operations: Arc::new(aex_session_dynamodb::store::OperationStore::new(
            dynamodb.clone(),
            stores.session_table.clone(),
        )),
        operation_worker: Arc::new(
            session_stream_api::session::operation_worker::LambdaOperationWorkerInvoker::new(
                aws_sdk_lambda::Client::new(&aws),
                config.session_maintenance_worker.value.clone(),
            ),
        ),
        // The command path: an eventually consistent reader, the physical table
        // names its one transaction compiles against, and the client that
        // submits it. This is the whole of what composing `aex-session-app`
        // into this deployable costs — the crate had no consumer at all before
        // it, and its finished use cases were unreachable.
        commands: aex_session_dynamodb::app_authority::SessionCommandReads::new(
            dynamodb.clone(),
            stores.session_table.clone(),
        ),
        tables: regional_tables(&stores),
        authority: dynamodb.clone(),
        cursor_keys: Arc::clone(&cursor_keys),
        runtime_activity: stores.runtime_activity.clone(),
    }));
    let mounted = mount_unary(Arc::new(dispatcher), Arc::new(session_edge), limits(config))?;

    // Health is merged exactly once beside the generated unary router.
    let router = mounted
        .router
        .merge(aex_regional_http::health::router(readiness.clone()))
        .merge(aex_regional_http::release_health::router(readiness));

    let address = SocketAddr::from((Ipv6Addr::UNSPECIFIED, config.port));
    let listener = tokio::net::TcpListener::bind(address)
        .await
        .map_err(|error| {
            SessionStreamApiRunError::Listener(format!("cannot bind {address}: {error}"))
        })?;
    eprintln!(
        "{DEPLOYABLE}: listening on {address} unary_routes={}",
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
        outcome = &mut server => outcome.map_err(|error| SessionStreamApiRunError::Listener(error.to_string())),
        () = wait_for_termination() => {
            // Readiness and producer drain flip before the listener is asked to
            // stop: new requests see `503` on readiness and the ALB deregisters
            // this target, existing streams emit `rotate` with their last sent
            // cursor, and in-flight unary requests run to completion.
            //
            // The deadline is bounded at configuration time so it fires before
            // `SIGKILL`, and the stream write stall is bounded so the last
            // producer can actually observe the flag inside it. An overrun here
            // is therefore a real signal rather than an arithmetic certainty —
            // and it now ends one process holding both halves, which is why
            // those two bounds are checks instead of comments.
            draining.store(true, Ordering::Release);
            let _ = shutdown_tx.send(true);
            eprintln!("{DEPLOYABLE}: draining, deadline {} ms", deadline.as_millis());
            match tokio::time::timeout(deadline, &mut server).await {
                Ok(outcome) => outcome.map_err(|error| SessionStreamApiRunError::Listener(error.to_string())),
                Err(_) => Err(SessionStreamApiRunError::Listener(format!(
                    "drain deadline of {} ms expired",
                    deadline.as_millis()
                ))),
            }
        }
    }
}

/// The physical regional table names, taken from configuration rather than
/// composed.
///
/// `RegionalTables::composed` mirrors what the infrastructure stream
/// instantiates, but a deployable that was given explicit names must use them:
/// guessing a name that configuration already answered is how a process ends up
/// writing to a table nobody deployed.
fn regional_tables(
    stores: &session_stream_api::session::Stores,
) -> aex_session_dynamodb::plan::RegionalTables {
    aex_session_dynamodb::plan::RegionalTables {
        session_authority: stores.session_table.clone(),
        regional_work: stores.work.table().to_owned(),
        regional_content: stores.content.table().to_owned(),
        regional_registry: stores.registry.table().to_owned(),
        regional_secret_custody: stores.custody.table().to_owned(),
        // This deployable holds no decrypt key and therefore has no keystore
        // binding at all. The empty name is not a default: an action routed to
        // it fails request construction rather than reaching a table nobody
        // configured.
        regional_secret_keystore: String::new(),
        runtime_activity: stores.runtime_activity.table().to_owned(),
        regional_authz_projection: stores.authz_projection_table.clone(),
    }
}

/// The canonical encryption context every content object this deployable writes
/// is bound to.
///
/// S3 binds it to the SSE-KMS operation and `CloudTrail` records it, so a body
/// written under one plane, region or key domain cannot be read back under
/// another. It is composed once at start-up because it is constant for the
/// process.
fn content_encryption_context(config: &Config) -> Result<Vec<u8>, SessionStreamApiRunError> {
    let pairs = std::collections::BTreeMap::from([
        ("aex:domain", "regional-content".to_owned()),
        ("aex:plane", config.plane.as_str().to_owned()),
        ("aex:region", config.region.as_str().to_owned()),
    ]);
    aex_wire::to_jcs_bytes(&pairs).map_err(|error| {
        SessionStreamApiRunError::Probe(format!(
            "the content encryption context could not be encoded: {error}"
        ))
    })
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

/// Builds one regional edge for one audience.
///
/// Called twice, and the audience is the only thing that differs. Both edges
/// read the same projection table and hold the same pepper ring: what separates
/// them is the audience each requires the key row to name, which is why a
/// credential admitted at one is not admitted at the other.
///
/// There is no cache budget and no fallible construction any more. Nothing is
/// held between requests, so two edges in one process cost what one did.
fn build_edge(
    config: &Config,
    dynamodb: &aws_sdk_dynamodb::Client,
    peppers: aex_regional_http::credential::PepperRing,
    audience: AssertionAudience,
) -> SessionEdge {
    let projection = aex_session_dynamodb::projection::ProjectionReader::new(
        dynamodb.clone(),
        config.authz_projection_table.clone(),
    );
    RegionalEdge::new(
        peppers,
        RegionalProjection::new(projection, config.region),
        SystemClock,
        EdgeBinding {
            audience,
            region: config.region,
        },
    )
}

/// The decode bounds the generated unary dispatchers enforce.
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
