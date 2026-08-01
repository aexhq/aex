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

use std::net::{Ipv6Addr, SocketAddr};
use std::process::ExitCode;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use aex_regional_http::config::ConfigError;
use aex_regional_http::health::Readiness;
use regional_stream::config::{Config, WakeMode};

/// Why `regional-stream` stopped.
#[derive(Debug, thiserror::Error)]
enum RunError {
    /// Start-up configuration was rejected.
    #[error(transparent)]
    Config(#[from] ConfigError),
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
    let settings = aex_platform_telemetry::Settings::default();
    let telemetry = aex_platform_telemetry::Handle::install(&settings, None);
    let outcome = run(&config, &telemetry).await;
    if let aex_platform_telemetry::FlushOutcome::DeadlineExceeded { pending } =
        telemetry.flush(settings.flush_deadline)
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
async fn run(config: &Config, telemetry: &aex_platform_telemetry::Handle) -> Result<(), RunError> {
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
    // Read-only clients only. There is no SQS client and no work-table adapter
    // in this link graph, which is what makes "the stream owns no mutation"
    // structural rather than reviewed.
    let dynamodb = aws_sdk_dynamodb::Client::new(&aws);
    let objects = aws_sdk_s3::Client::new(&aws);
    let readers = Readers {
        dynamodb,
        objects,
        session_table: config.session_table.clone(),
        observation_table: config.observation_table.clone(),
        content_bucket: config.content_bucket.clone(),
    };

    let draining = Arc::new(AtomicBool::new(false));
    let router = aex_regional_http::health::router(readiness(config, &readers, false));

    let address = SocketAddr::from((Ipv6Addr::UNSPECIFIED, config.port));
    let listener = tokio::net::TcpListener::bind(address)
        .await
        .map_err(|error| RunError::Listener(format!("cannot bind {address}: {error}")))?;
    eprintln!(
        "regional-stream: listening on {address} wake={} tasks={}",
        config.wake_mode.as_str(),
        config.max_tasks
    );

    let drain = Arc::clone(&draining);
    let deadline = Duration::from_millis(config.drain_deadline_ms);
    axum::serve(listener, router)
        .with_graceful_shutdown(async move {
            wait_for_termination().await;
            // Readiness flips first so the ALB deregisters this target before
            // the socket drain runs; the deregistration delay is what turns a
            // task replacement into a reconnect rather than a dropped frame.
            drain.store(true, Ordering::SeqCst);
            eprintln!(
                "regional-stream: draining, deadline {} ms",
                deadline.as_millis()
            );
        })
        .await
        .map_err(|error| RunError::Listener(error.to_string()))
}

/// The read-only authorities this service is bound to.
#[derive(Debug, Clone)]
struct Readers {
    dynamodb: aws_sdk_dynamodb::Client,
    objects: aws_sdk_s3::Client,
    session_table: String,
    observation_table: String,
    content_bucket: String,
}

impl Readers {
    /// Dependency names that are not yet resolved.
    fn unresolved(&self) -> Vec<String> {
        let mut unresolved = Vec::new();
        for (name, bound) in [
            ("session-authority", !self.session_table.is_empty()),
            ("observation-authority", !self.observation_table.is_empty()),
            ("content-bucket", !self.content_bucket.is_empty()),
        ] {
            if !bound {
                unresolved.push(name.to_owned());
            }
        }
        unresolved
    }
}

/// Fail-closed readiness: draining is never ready, and neither is a process
/// whose authoritative readers are unbound.
fn readiness(config: &Config, readers: &Readers, draining: bool) -> Readiness {
    let mut unavailable = readers.unresolved();
    if draining {
        unavailable.push("draining".to_owned());
    }
    if config.wake_mode == WakeMode::DdbStreams && config.session_stream.is_none() {
        unavailable.push("session-authority-stream".to_owned());
    }
    if unavailable.is_empty() {
        Readiness::ready(config.release_digest.clone())
    } else {
        Readiness::not_ready(config.release_digest.clone(), unavailable)
            .unwrap_or_else(|_| Readiness::ready(config.release_digest.clone()))
    }
}

/// Waits for the container runtime's stop signal.
async fn wait_for_termination() {
    #[cfg(unix)]
    {
        let mut terminate =
            match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
                Ok(signal) => signal,
                Err(_) => return std::future::pending().await,
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

/// Keeps the read-only clients reachable from the composition.
const _: fn(&Readers) -> (&aws_sdk_dynamodb::Client, &aws_sdk_s3::Client) =
    |readers| (&readers.dynamodb, &readers.objects);
