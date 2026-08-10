//! `session-stream-api` composition root (Rust Fargate OCI).
//!
//! Exclusive responsibility: the finite session, run, operation, registry,
//! upload, content-metadata, approval, secret-metadata and usage routes, **and**
//! the NDJSON replay and live-socket routes. One process, one listener, one
//! drain flag, two assertion audiences.
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
use aex_observation_domain::keys::ScopeKey;
use aex_regional_http::authz::{ParameterStore, RegionalProjection, SecretStore, TrustError};
use aex_regional_http::capability::{CompositionError, Declares, StreamSocket, WorkClaim};
use aex_regional_http::config::RegionalHttpConfigError;
use aex_regional_http::edge::{EdgeBinding, RegionalEdge, SystemClock};
use aex_regional_http::health::{Readiness, ReadinessError};
use aex_regional_http::mount::{MountError, mount_unary};
use aex_session_dynamodb::stream_keys::SessionReadState;
use aex_wire::dispatch::RequestLimits;
use aex_wire::error::{ErrorCode, WireError};
use regional_observation_api::api::{ObservationService, StreamPolicy, StreamRevalidator};
use regional_observation_api::counters::{
    CounterResource, PUBLISH_INTERVAL, ReadCounter, ReadCounters,
};
use regional_observation_api::reader::ObservationReader;
use regional_observation_api::wake::WakeHub;
use session_stream_api::capability::Composition;
use session_stream_api::config::{Config, WakeMode};
use session_stream_api::session::handlers::{Dispatcher, Shared};
use session_stream_api::stream::mount::{AppState, Edge};
use session_stream_api::stream::wakes::{AuthorityStream, StreamSpec};
use session_stream_api::stream::{QuotaLimits, QuotaManager};

/// The deployable name every diagnostic and telemetry record carries.
const DEPLOYABLE: &str = session_stream_api::config::DEPLOYABLE;

/// The audience the finite unary half accepts.
///
/// A credential admitted at the stream must never admit a unary request, and the
/// reverse. Merging the two deployables did **not** merge their audiences:
/// inventing a combined one would hand every stream credential the finite API's
/// write surface. The audience used to live in the assertion envelope and now
/// lives on the key authorization row, checked at stage 5 of admission and again
/// on every stream revalidation, so the two edges below remain two genuinely
/// different trust boundaries that happen to share a process.
const SESSION_AUDIENCE: AssertionAudience = AssertionAudience::RegionalSession;

/// The audience the long-lived NDJSON half accepts.
const STREAM_AUDIENCE: AssertionAudience = AssertionAudience::RegionalStream;

/// The production regional edge shape, once per audience.
type SessionEdge = RegionalEdge<
    RegionalProjection<aex_session_dynamodb::projection::ProjectionReader>,
    SystemClock,
>;

#[derive(Clone)]
struct EdgeRevalidator {
    edge: Arc<Edge>,
    dynamodb: aws_sdk_dynamodb::Client,
    session_table: String,
    counters: Arc<ReadCounters>,
}

#[async_trait::async_trait]
impl StreamRevalidator for EdgeRevalidator {
    async fn revalidate(
        &self,
        authorization: &aex_regional_http::context::RegionalAuthorization,
        scope: &ScopeKey,
    ) -> Result<(), aex_wire::error::WireError> {
        self.edge.revalidate(authorization).await?;
        let ScopeKey::Session(session) = scope else {
            return Ok(());
        };
        let (pk, sk) = aex_session_dynamodb::stream_keys::head(*session);
        self.counters.record(ReadCounter::SessionHead);
        let response = self
            .dynamodb
            .get_item()
            .table_name(&self.session_table)
            .key("pk", aws_sdk_dynamodb::types::AttributeValue::S(pk))
            .key(
                "sk",
                aws_sdk_dynamodb::types::AttributeValue::S(sk.to_owned()),
            )
            .consistent_read(true)
            .send()
            .await
            .map_err(|_| WireError::new(ErrorCode::ObservabilityUnavailable))?;
        let item = response
            .item
            .ok_or_else(|| WireError::new(ErrorCode::SessionDeleted))?;
        match aex_session_dynamodb::stream_keys::session_read_state(
            &item,
            authorization.workspace_id,
        ) {
            Ok(SessionReadState::Active) => Ok(()),
            Ok(SessionReadState::Deleting) => Err(WireError::new(ErrorCode::SessionDeleting)),
            Ok(SessionReadState::Deleted) => Err(WireError::new(ErrorCode::SessionDeleted)),
            Err(attribute) => Err(WireError::new(ErrorCode::ObservabilityUnavailable)
                .with_message(format!("session head has invalid `{attribute}`"))),
        }
    }
}

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
    // Telemetry first: a configuration refusal must reach the wire, or a
    // crash-looping deployment is visible only to whoever tails stderr. The
    // long-lived profile replaces the Lambda default, which flushes on an
    // invocation boundary this process does not have. The pump refusing to
    // spawn is the one failure that stays stderr-only, because there is no
    // installed exporter to carry it yet.
    let telemetry = match aex_platform_telemetry::LongLivedTelemetry::install() {
        Ok(telemetry) => telemetry,
        Err(error) => {
            eprintln!("{DEPLOYABLE}: refusing to start: {error}");
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
                .with(aex_telemetry_schema::generated::AEX_DEPLOYABLE, DEPLOYABLE),
            );
            let _ = telemetry.shutdown();
            eprintln!("{DEPLOYABLE}: refusing to start: {error}");
            return ExitCode::FAILURE;
        }
    };
    let outcome = Box::pin(run(&config, telemetry.handle())).await;
    if let aex_platform_telemetry::FlushOutcome::DeadlineExceeded { pending } = telemetry.shutdown()
    {
        eprintln!("{DEPLOYABLE}: telemetry flush left {pending} record(s) undelivered");
    }
    match outcome {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{DEPLOYABLE}: stopped: {error}");
            ExitCode::FAILURE
        }
    }
}

/// Builds the real adapters, assembles both routers, binds one listener and
/// serves it until the drain completes.
#[allow(
    clippy::too_many_lines,
    reason = "the composition root deliberately keeps every probed authority, edge, wake reader, quota and drain binding visible in one audit surface"
)]
async fn run(
    config: &Config,
    telemetry: &aex_platform_telemetry::Handle,
) -> Result<(), SessionStreamApiRunError> {
    // Before any client is opened: the declared capabilities and the configured
    // resources must agree. This is the fail-closed half of the guarantee the
    // stream's `AEX_WORK_TABLE` refusal used to carry on its own.
    session_stream_api::capability::admit(config)?;

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
    let secrets = SecretStore::new(aws_sdk_secretsmanager::Client::new(&aws));

    // One flag for the whole process, read by readiness and by both halves. It
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
    // it can issue no continuation a later request could redeem. One pepper ring
    // and one cursor ring serve both halves — they were always the same two
    // references read twice by two tasks.
    let peppers = secrets.pepper_ring(&config.credential_pepper_ref).await?;
    // One ring, shared by both halves. The unary side signs continuations with
    // it and the stream side resumes them, so reading the parameter twice would
    // duplicate the secret material and, mid-rotation, could give one half a ring
    // the other cannot verify against.
    let cursor_keys = Arc::new(
        parameters
            .cursor_key_ring(&config.cursor_signing_key_ref)
            .await?,
    );

    // --- the stream half's authorities, probed before bind --------------------
    let counters = Arc::new(ReadCounters::default());
    let reader = ObservationReader::new(
        dynamodb.clone(),
        objects.clone(),
        config.observation_table.clone(),
        config.session_table.clone(),
        config.content_bucket.clone(),
        config.observation_index_settle_ms,
        Arc::clone(&counters),
    );
    Box::pin(reader.probe())
        .await
        .map_err(|error| SessionStreamApiRunError::Probe(error.to_string()))?;
    dynamodb
        .describe_table()
        .table_name(&config.session_table)
        .send()
        .await
        .map_err(|error| {
            SessionStreamApiRunError::Probe(format!("`session-authority` is not readable: {error}"))
        })?;

    // Aggregate deltas only: the read path increments atomics, and this is the
    // one task that turns them into records.
    tokio::spawn(regional_observation_api::counters::publish(
        Arc::clone(&counters),
        telemetry.clone(),
        CounterResource {
            plane: config.plane.as_str().to_owned(),
            region: config.region.as_str().to_owned(),
            deployable: DEPLOYABLE,
        },
        PUBLISH_INTERVAL,
        Arc::clone(&draining),
    ));

    // The one production reader of the projection frontier. Nothing read it
    // before, so the bound every cluster relies on — how long a pause, a
    // revocation or a limit change takes to reach this region — was unmeasured.
    // It publishes; it refuses nothing.
    tokio::spawn(session_stream_api::frontier::publish(
        Arc::new(aex_session_dynamodb::projection::ProjectionReader::new(
            dynamodb.clone(),
            stores.authz_projection_table.clone(),
        )),
        telemetry.clone(),
        session_stream_api::frontier::FrontierResource {
            plane: config.plane.as_str().to_owned(),
            region: config.region.as_str().to_owned(),
            deployable: DEPLOYABLE,
        },
        session_stream_api::frontier::MEASURE_INTERVAL,
        Arc::clone(&draining),
    ));

    let wake_hub = WakeHub::default();
    let _wake_readers = if config.wake_mode == WakeMode::DdbStreams {
        let session = config.session_stream.as_ref().ok_or_else(|| {
            SessionStreamApiRunError::Probe(
                "ddb_streams mode omitted the session stream ARN".to_owned(),
            )
        })?;
        let observation = config.observation_stream.as_ref().ok_or_else(|| {
            SessionStreamApiRunError::Probe(
                "ddb_streams mode omitted the observation stream ARN".to_owned(),
            )
        })?;
        let endpoint = config
            .dynamodb_streams_endpoint_url
            .as_ref()
            .ok_or_else(|| {
                SessionStreamApiRunError::Probe(
                    "ddb_streams mode omitted the private DynamoDB Streams endpoint".to_owned(),
                )
            })?;
        let streams = aws_sdk_dynamodbstreams::Client::from_conf(
            aws_sdk_dynamodbstreams::config::Builder::from(&aws)
                .endpoint_url(endpoint.clone())
                .build(),
        );
        Some(
            session_stream_api::stream::wakes::start(
                streams,
                dynamodb.clone(),
                config.session_table.clone(),
                vec![
                    StreamSpec {
                        arn: session.value.clone(),
                        authority: AuthorityStream::Session,
                    },
                    StreamSpec {
                        arn: observation.value.clone(),
                        authority: AuthorityStream::Observation,
                    },
                ],
                wake_hub.clone(),
                // The published stream counters are the readers' health
                // surface: stderr alone cannot be alarmed on, and a degraded
                // reader silently demotes every consumer to fallback polling.
                Arc::clone(&counters),
            )
            .await
            .map_err(|error| SessionStreamApiRunError::Probe(error.to_string()))?,
        )
    } else {
        None
    };

    // --- two edges, one per audience -----------------------------------------
    let session_edge = build_edge(config, &dynamodb, peppers.clone(), SESSION_AUDIENCE);
    let stream_edge = Arc::new(build_edge(config, &dynamodb, peppers, STREAM_AUDIENCE));

    // --- the session half's router -------------------------------------------
    let dispatcher = Dispatcher::new(Arc::new(Shared {
        custody: Arc::new(stores.custody.clone()),
        custody_table: stores.custody.table().to_owned(),
        registry: Arc::new(stores.registry.clone()),
        sessions: Arc::new(aex_session_dynamodb::store::SessionReads::new(
            dynamodb.clone(),
            stores.session_table.clone(),
        )),
        // The cold workspace surface, over the same projection table the edge
        // authorizes against. Two ports over one reader rather than one wide
        // port: a limit read must not be able to reach for a placement, and the
        // admission path must not gain a profile read it never makes.
        workspace: Arc::new(aex_session_dynamodb::projection::ProjectionReader::new(
            dynamodb.clone(),
            stores.authz_projection_table.clone(),
        )),
        placements: Arc::new(aex_session_dynamodb::projection::ProjectionReader::new(
            dynamodb.clone(),
            stores.authz_projection_table.clone(),
        )),
        api_url: config.regional_api_url.clone(),
        operations: Arc::new(aex_session_dynamodb::store::OperationStore::new(
            dynamodb.clone(),
            stores.session_table.clone(),
        )),
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
    }));
    let mounted = mount_unary(Arc::new(dispatcher), Arc::new(session_edge), limits(config))?;

    // --- the stream half's router --------------------------------------------
    let frames =
        (config.connection_buffer_bytes / aex_regional_http::stream::MAX_FRAME_BYTES).max(1);
    let stream_policy = StreamPolicy {
        buffered_frames: frames,
        write_stall: Duration::from_millis(config.write_stall_ms),
        draining: Arc::clone(&draining),
        wakes: (config.wake_mode == WakeMode::DdbStreams).then_some(wake_hub),
        ..StreamPolicy::new(Arc::new(EdgeRevalidator {
            edge: Arc::clone(&stream_edge),
            dynamodb: dynamodb.clone(),
            session_table: config.session_table.clone(),
            counters: Arc::clone(&counters),
        }))
    };
    let service = ObservationService::new(
        reader,
        config.observation_budget,
        aex_observation_domain::limits::METRIC_AGGREGATE_SCAN,
        cursor_keys,
        config.region,
        stream_policy,
    );
    let quotas = QuotaManager::new(QuotaLimits {
        total: quota(config.max_connections)?,
        session: quota(config.max_connections_session)?,
        observation: quota(config.max_connections_observation)?,
        per_workspace: quota(config.max_connections_per_workspace)?,
    })
    .map_err(|error| SessionStreamApiRunError::Probe(error.to_string()))?;
    let streams = session_stream_api::stream::mount::router(
        &<Composition as Declares<StreamSocket>>::grant(),
        Arc::new(AppState {
            edge: stream_edge,
            service: Arc::new(service),
            limits: RequestLimits::DEFAULT,
            quotas,
            draining: Arc::clone(&draining),
        }),
    );

    // --- one router ----------------------------------------------------------
    //
    // `Router::merge` rather than a new `RouteOwner` variant. The two halves'
    // URL prefixes (`/api/sessions`, `/api/workspace`, `/api/operations`,
    // `/api/billing`, `/api/streams`) are already disjoint and must stay so; a
    // combined owner would mean regenerating `api/generated` to rename
    // `servingArtifact`, turning a deployment change into a contract change.
    //
    // Health is merged exactly once. The stream half used to hand-roll the same
    // two paths, which on axum 0.8 is a `Router::merge` panic at start-up rather
    // than a duplicate endpoint.
    let router = mounted
        .router
        .merge(streams)
        .merge(aex_regional_http::health::router(readiness.clone()))
        .merge(aex_regional_http::release_health::router(readiness));

    let address = SocketAddr::from((Ipv6Addr::UNSPECIFIED, config.port));
    let listener = tokio::net::TcpListener::bind(address)
        .await
        .map_err(|error| {
            SessionStreamApiRunError::Listener(format!("cannot bind {address}: {error}"))
        })?;
    eprintln!(
        "{DEPLOYABLE}: listening on {address} unary_routes={} wake={} tasks={}",
        mounted.routes.len(),
        config.wake_mode.as_str(),
        config.max_tasks
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

fn quota(value: u64) -> Result<u32, SessionStreamApiRunError> {
    u32::try_from(value).map_err(|_| {
        SessionStreamApiRunError::Probe(format!("connection quota `{value}` exceeds `u32`"))
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
