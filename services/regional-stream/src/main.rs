//! `regional-stream` composition root (Rust Fargate OCI).
//!
//! Exclusive responsibility: the NDJSON replay and live-socket routes. It owns
//! no mutation of any kind — no queue client, no work table, no write action —
//! and its configuration refuses every binding that would give it one.
//!
//! The process binds its listener only after configuration is admitted, serves
//! `/internal/healthz` and `/internal/readyz` (the ALB target-group check), and
//! on `SIGTERM` flips readiness to `503` first so the load balancer deregisters
//! before the drain deadline runs.

use std::future::IntoFuture as _;
use std::net::{Ipv6Addr, SocketAddr};
use std::process::ExitCode;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use aex_internal_contracts::assertion::AssertionAudience;
use aex_observation_domain::keys::ScopeKey;
use aex_regional_http::assertion::AuthFailure;
use aex_regional_http::authz::{
    LambdaAssertionSource, ParameterStore, RegionalProjection, TrustError,
};
use aex_regional_http::config::RegionalHttpConfigError;
use aex_regional_http::edge::{EdgeBinding, RegionalEdge, SystemClock};
use aex_session_dynamodb::stream_keys::SessionReadState;
use aex_wire::dispatch::RequestLimits;
use aex_wire::error::{ErrorCode, WireError};
use regional_observation_api::api::{ObservationService, StreamPolicy, StreamRevalidator};
use regional_observation_api::counters::{
    CounterResource, PUBLISH_INTERVAL, ReadCounter, ReadCounters,
};
use regional_observation_api::reader::ObservationReader;
use regional_observation_api::wake::WakeHub;
use regional_stream::config::{Config, WakeMode};
use regional_stream::mount::{AppState, Edge};
use regional_stream::wakes::{AuthorityStream, StreamSpec};
use regional_stream::{QuotaLimits, QuotaManager};

const AUDIENCE: AssertionAudience = AssertionAudience::RegionalStream;

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

/// Why `regional-stream` stopped.
#[derive(Debug, thiserror::Error)]
enum RegionalStreamRunError {
    /// Start-up configuration was rejected.
    #[error(transparent)]
    Config(#[from] RegionalHttpConfigError),
    /// Cold-start trust material was unreadable or malformed.
    #[error(transparent)]
    Trust(#[from] TrustError),
    /// The authenticated regional edge could not be composed.
    #[error("the request edge could not be composed: {0}")]
    Edge(AuthFailure),
    /// A required authority could not be proven before bind.
    #[error("a start-up probe failed: {0}")]
    Probe(String),
    /// The listener could not be bound or served.
    #[error("the stream listener stopped: {0}")]
    Listener(String),
}

#[tokio::main]
async fn main() -> ExitCode {
    let config = match Config::from_env() {
        Ok(config) => config,
        Err(error) => {
            eprintln!("regional-stream: refusing to start: {error}");
            return ExitCode::FAILURE;
        }
    };
    let telemetry = match aex_platform_telemetry::LongLivedTelemetry::install() {
        Ok(telemetry) => telemetry,
        Err(error) => {
            eprintln!("regional-stream: refusing to start: {error}");
            return ExitCode::FAILURE;
        }
    };
    let outcome = Box::pin(run(&config, telemetry.handle())).await;
    if let aex_platform_telemetry::FlushOutcome::DeadlineExceeded { pending } = telemetry.shutdown()
    {
        eprintln!("regional-stream: telemetry flush left {pending} record(s) undelivered");
    }
    match outcome {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("regional-stream: stopped: {error}");
            ExitCode::FAILURE
        }
    }
}

/// Builds the authoritative readers, binds the listener and serves until drain.
#[allow(
    clippy::too_many_lines,
    reason = "the composition root deliberately keeps every probed authority, wake reader, quota and drain binding visible in one audit surface"
)]
async fn run(config: &Config, telemetry: &aex_platform_telemetry::Handle) -> Result<(), RegionalStreamRunError> {
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

    let counters = Arc::new(ReadCounters::default());
    let reader = ObservationReader::new(
        dynamodb.clone(),
        objects,
        config.observation_table.clone(),
        config.session_table.clone(),
        config.content_bucket.clone(),
        config.observation_index_settle_ms,
        Arc::clone(&counters),
    );
    Box::pin(reader.probe())
        .await
        .map_err(|error| RegionalStreamRunError::Probe(error.to_string()))?;
    dynamodb
        .describe_table()
        .table_name(&config.session_table)
        .send()
        .await
        .map_err(|error| {
            RegionalStreamRunError::Probe(format!("`session-authority` is not readable: {error}"))
        })?;

    let anchors = parameters
        .trust_anchors(&config.authz_verify_keys_param)
        .await?;
    let cursor_ring = parameters
        .cursor_key_ring(&config.cursor_signing_key_ref)
        .await?;

    let draining = Arc::new(AtomicBool::new(false));
    // Aggregate deltas only: the read path increments atomics, and this is the
    // one task that turns them into records.
    tokio::spawn(regional_observation_api::counters::publish(
        Arc::clone(&counters),
        telemetry.clone(),
        CounterResource {
            plane: config.plane.as_str().to_owned(),
            region: config.region.as_str().to_owned(),
            deployable: "regional-stream",
        },
        PUBLISH_INTERVAL,
        Arc::clone(&draining),
    ));
    let wake_hub = WakeHub::default();
    let _wake_readers = if config.wake_mode == WakeMode::DdbStreams {
        let session = config.session_stream.as_ref().ok_or_else(|| {
            RegionalStreamRunError::Probe("ddb_streams mode omitted the session stream ARN".to_owned())
        })?;
        let observation = config.observation_stream.as_ref().ok_or_else(|| {
            RegionalStreamRunError::Probe("ddb_streams mode omitted the observation stream ARN".to_owned())
        })?;
        let endpoint = config
            .dynamodb_streams_endpoint_url
            .as_ref()
            .ok_or_else(|| {
                RegionalStreamRunError::Probe(
                    "ddb_streams mode omitted the private DynamoDB Streams endpoint".to_owned(),
                )
            })?;
        let streams = aws_sdk_dynamodbstreams::Client::from_conf(
            aws_sdk_dynamodbstreams::config::Builder::from(&aws)
                .endpoint_url(endpoint.clone())
                .build(),
        );
        Some(
            regional_stream::wakes::start(
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
            )
            .await
            .map_err(|error| RegionalStreamRunError::Probe(error.to_string()))?,
        )
    } else {
        None
    };
    let edge = Arc::new(build_edge(config, &aws, &dynamodb, anchors)?);
    let frames =
        (config.connection_buffer_bytes / aex_regional_http::stream::MAX_FRAME_BYTES).max(1);
    let stream_policy = StreamPolicy {
        buffered_frames: frames,
        write_stall: Duration::from_millis(config.write_stall_ms),
        draining: Arc::clone(&draining),
        wakes: (config.wake_mode == WakeMode::DdbStreams).then_some(wake_hub),
        ..StreamPolicy::new(Arc::new(EdgeRevalidator {
            edge: Arc::clone(&edge),
            dynamodb: dynamodb.clone(),
            session_table: config.session_table.clone(),
            counters: Arc::clone(&counters),
        }))
    };
    let service = ObservationService::new(
        reader,
        config.observation_budget,
        aex_observation_domain::limits::METRIC_AGGREGATE_SCAN,
        cursor_ring,
        config.region,
        stream_policy,
    );
    let quotas = QuotaManager::new(QuotaLimits {
        total: quota(config.max_connections)?,
        session: quota(config.max_connections_session)?,
        observation: quota(config.max_connections_observation)?,
        per_workspace: quota(config.max_connections_per_workspace)?,
    })
    .map_err(|error| RegionalStreamRunError::Probe(error.to_string()))?;
    let router = regional_stream::mount::router(Arc::new(AppState {
        edge,
        service: Arc::new(service),
        limits: RequestLimits::DEFAULT,
        quotas,
        draining: Arc::clone(&draining),
        release_digest: config.release_digest.clone(),
    }));

    let address = SocketAddr::from((Ipv6Addr::UNSPECIFIED, config.port));
    let listener = tokio::net::TcpListener::bind(address)
        .await
        .map_err(|error| RegionalStreamRunError::Listener(format!("cannot bind {address}: {error}")))?;
    eprintln!(
        "regional-stream: listening on {address} wake={} tasks={}",
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
        outcome = &mut server => outcome.map_err(|error| RegionalStreamRunError::Listener(error.to_string())),
        () = wait_for_termination() => {
            // Readiness and producer drain flip before the listener is asked to
            // stop. Existing streams emit `rotate` with their last sent cursor;
            // new connections receive 503 while the ALB deregisters the task.
            draining.store(true, Ordering::Release);
            let _ = shutdown_tx.send(true);
            eprintln!("regional-stream: draining, deadline {} ms", deadline.as_millis());
            match tokio::time::timeout(deadline, &mut server).await {
                Ok(outcome) => outcome.map_err(|error| RegionalStreamRunError::Listener(error.to_string())),
                Err(_) => Err(RegionalStreamRunError::Listener(format!(
                    "drain deadline of {} ms expired",
                    deadline.as_millis()
                ))),
            }
        }
    }
}

fn quota(value: u64) -> Result<u32, RegionalStreamRunError> {
    u32::try_from(value)
        .map_err(|_| RegionalStreamRunError::Probe(format!("connection quota `{value}` exceeds `u32`")))
}

fn build_edge(
    config: &Config,
    aws: &aws_config::SdkConfig,
    dynamodb: &aws_sdk_dynamodb::Client,
    anchors: aex_identity_domain::assertion::VerificationKeySet,
) -> Result<Edge, RegionalStreamRunError> {
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
    .map_err(RegionalStreamRunError::Edge)
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
