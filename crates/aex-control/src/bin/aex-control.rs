//! The Aex control-plane server.
//!
//! Env: AEX_BRAIN_TOKEN required; AEX_BRAIN_URL (default
//! http://127.0.0.1:8700); AEX_CONTROL_LISTEN (default 127.0.0.1:8600);
//! AEX_CONTROL_INTERNAL_LISTEN (loopback only, default 127.0.0.1:8601); AEX_CONTROL_DB
//! (default ./aex-control-data/control.db); AEX_PAYMENTS fake|stripe (default fake — loud
//! banner; stripe needs STRIPE_SECRET_KEY); AEX_TOPUP_SUCCESS_URL / AEX_TOPUP_CANCEL_URL;
//! AEX_RATE_* card overrides; AEX_LIMIT_CONCURRENT_SESSIONS (root sessions) /
//! AEX_LIMIT_SESSION_CREATES_PER_HOUR (root creates);
//! AEX_OPERATOR_TOKEN (enables waitlist administration); AEX_SWEEP_SECONDS (default 30);
//! AEX_DISCOVERY_OVERLAP_SECONDS (default max(2*sweep, 120));
//! AEX_DISCOVERY_SESSION_LIMIT (one-time bootstrap safety bound, default 100,000).
//! AEX_DISCOVERY_PARTITIONS (stable Brain changefeed partitions, default 1, maximum 256).
//! AEX_STORAGE_MAX_OBJECT_BYTES (default 512 MiB) and AEX_STORAGE_MAX_SESSION_BYTES
//! (default 10 GiB) bound durable session uploads.
//! AEX_MAX_CONCURRENT_CREATE_BODIES (default 4) bounds simultaneous 24 MiB request buffers.
//! AEX_MAX_CONCURRENT_MESSAGE_BODIES (default 256) and
//! AEX_MAX_CONCURRENT_INLINE_SESSION_BODIES (default 64) separately bound authenticated
//! 192 KiB prompt/message and 2 MiB inline-session request buffers.
//! AEX_EXTERNAL_TOOL_EXECUTOR_TOKEN authenticates the private Brain route; SERPER_API_KEY enables
//! the managed web_search Tool without entering Brain or the Hand.
//! AEX_CUSTOMER_ENVIRONMENT_GATEWAY_TOKEN, AEX_CUSTOMER_ENVIRONMENT_TRUSTED_PROXY_CIDRS and
//! AEX_MANAGED_ENVIRONMENT_NAT_CIDRS jointly enable the API Gateway WebSocket adapter.

use std::path::PathBuf;
use std::sync::Arc;

use aex_control::admission::Admission;
use aex_control::api::{AppState, internal_router, router};
use aex_control::brain::BrainClient;
use aex_control::payments::{FakePayments, Payments, StripePayments, StripeWebhook};
use aex_control::store::Db;
use aex_control::sweep::{run_deletion_worker, run_sweeper};
use aex_control::web::WebRuntime;
use aex_control::{Config, PaymentsMode};

fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "aex_control=info".into()),
        )
        .init();
    let command = match std::env::args().nth(1).as_deref() {
        None => Command::Serve,
        Some("reset-prelaunch-sessions") if std::env::args().nth(2).is_none() => {
            Command::ResetPrelaunchSessions
        }
        Some(argument) => anyhow::bail!("unknown command: {argument}"),
    };
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    match command {
        Command::Serve => runtime.block_on(run(Config::from_env()?)),
        Command::ResetPrelaunchSessions => runtime.block_on(reset_prelaunch_sessions()),
    }
}

enum Command {
    Serve,
    ResetPrelaunchSessions,
}

async fn reset_prelaunch_sessions() -> anyhow::Result<()> {
    let path = std::env::var_os("AEX_CONTROL_DB")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("./aex-control-data/control.db"));
    let removed = Db::open(&path)?.reset_prelaunch_sessions().await?;
    tracing::info!(removed, "discarded pre-launch control session state");
    Ok(())
}

async fn run(cfg: Config) -> anyhow::Result<()> {
    let (payments, stripe_webhook): (Arc<dyn Payments>, Option<StripeWebhook>) = match &cfg.payments
    {
        PaymentsMode::Fake => {
            tracing::warn!(
                "payments: FAKE — every top-up is instantly 'paid', no money moves. \
                 Local development only; set AEX_PAYMENTS=stripe and STRIPE_SECRET_KEY for real billing."
            );
            (Arc::new(FakePayments), None)
        }
        PaymentsMode::Stripe {
            secret_key,
            webhook_secret,
        } => {
            let mode = if secret_key.starts_with("sk_live_") {
                "LIVE"
            } else {
                "test"
            };
            tracing::info!("payments: stripe ({mode} mode)");
            (
                Arc::new(StripePayments::new(
                    secret_key.clone(),
                    cfg.topup_success_url.clone(),
                    cfg.topup_cancel_url.clone(),
                )),
                Some(StripeWebhook::new(webhook_secret.clone())),
            )
        }
    };
    let db = Db::open(&cfg.db_path)?;
    let brain = BrainClient::new(cfg.brain_url.clone(), cfg.brain_token.clone())
        .with_discovery_policy(cfg.discovery_overlap_ms, cfg.discovery_session_limit)
        .with_discovery_partitions(cfg.discovery_partitions);
    let admission = Admission::new(cfg.admission)?;
    tracing::info!(
        "brain: {} · db: {} · card: 1gb {}/h running",
        cfg.brain_url,
        cfg.db_path.display(),
        aex_control::usd_display(cfg.card.hourly_microusd("1gb")?),
    );

    tokio::spawn(run_sweeper(
        db.clone(),
        brain.clone(),
        cfg.card.clone(),
        cfg.sweep_seconds,
        admission.clone(),
    ));
    tokio::spawn(run_deletion_worker(
        db.clone(),
        brain.clone(),
        cfg.card.clone(),
    ));

    let state = AppState {
        db,
        brain,
        payments,
        stripe_webhook,
        card: cfg.card.clone(),
        operator_token_hash: cfg.operator_token_hash,
        external_executor_token_hash: cfg.external_executor_token_hash,
        customer_environment_gateway: cfg.customer_environment_gateway,
        web: WebRuntime::hosted(cfg.serper_api_key),
        admission,
        create_body_slots: Arc::new(tokio::sync::Semaphore::new(
            cfg.max_concurrent_create_bodies,
        )),
        message_body_slots: Arc::new(tokio::sync::Semaphore::new(
            cfg.max_concurrent_message_bodies,
        )),
        inline_session_body_slots: Arc::new(tokio::sync::Semaphore::new(
            cfg.max_concurrent_inline_session_bodies,
        )),
        default_limits: (cfg.max_concurrent_sessions, cfg.session_creates_per_hour),
        max_retained_root_sessions: cfg.max_retained_root_sessions,
        storage_limits: cfg.storage_limits,
    };
    let listener = tokio::net::TcpListener::bind(cfg.listen).await?;
    let internal_listener = tokio::net::TcpListener::bind(cfg.internal_listen).await?;
    let (catalog_source, catalog_generated, catalog_providers) =
        aex_control::model::catalog_provenance();
    tracing::info!(
        "model catalog: {catalog_providers} providers from {catalog_source} generated {catalog_generated}"
    );
    tracing::info!("control plane listening on {}", cfg.listen);
    tracing::info!("private tool executor listening on {}", cfg.internal_listen);
    let (shutdown, mut public_shutdown) = tokio::sync::watch::channel(false);
    let mut internal_shutdown = public_shutdown.clone();
    tokio::spawn(async move {
        let _ = tokio::signal::ctrl_c().await;
        let _ = shutdown.send(true);
    });
    tokio::try_join!(
        axum::serve(
            listener,
            router(state.clone()).into_make_service_with_connect_info::<std::net::SocketAddr>(),
        )
        .with_graceful_shutdown(async move {
            if !*public_shutdown.borrow() {
                let _ = public_shutdown.changed().await;
            }
        }),
        axum::serve(internal_listener, internal_router(state)).with_graceful_shutdown(async move {
            if !*internal_shutdown.borrow() {
                let _ = internal_shutdown.changed().await;
            }
        }),
    )?;
    Ok(())
}
