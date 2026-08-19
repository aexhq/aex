//! The Aex control-plane server.
//!
//! Env: AEX_BRAIN_TOKEN required (the brain's AEX_API_TOKEN); AEX_BRAIN_URL (default
//! http://127.0.0.1:8700); AEX_CONTROL_LISTEN (default 127.0.0.1:8600); AEX_CONTROL_DB
//! (default ./aex-control-data/control.db); AEX_PAYMENTS fake|stripe (default fake — loud
//! banner; stripe needs STRIPE_SECRET_KEY); AEX_TOPUP_SUCCESS_URL / AEX_TOPUP_CANCEL_URL;
//! AEX_RATE_* card overrides; AEX_LIMIT_CONCURRENT_SESSIONS / AEX_LIMIT_SESSION_CREATES_PER_HOUR;
//! AEX_OPERATOR_TOKEN (enables waitlist administration); AEX_SWEEP_SECONDS (default 30).

use std::sync::Arc;

use aex_control::api::{AppState, router};
use aex_control::brain::BrainClient;
use aex_control::payments::{FakePayments, Payments, StripePayments, StripeWebhook};
use aex_control::store::Db;
use aex_control::sweep::run_sweeper;
use aex_control::{Config, PaymentsMode};

fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "aex_control=info".into()),
        )
        .init();
    let cfg = Config::from_env()?;
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?
        .block_on(run(cfg))
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
    let brain = BrainClient::new(cfg.brain_url.clone(), cfg.brain_token.clone());
    tracing::info!(
        "brain: {} · db: {} · card: 1gb {}/h running",
        cfg.brain_url,
        cfg.db_path.display(),
        aex_control::usd_display(cfg.card.hourly_microusd("1gb")),
    );

    tokio::spawn(run_sweeper(
        db.clone(),
        brain.clone(),
        cfg.card.clone(),
        cfg.sweep_seconds,
    ));

    let state = AppState {
        db,
        brain,
        payments,
        stripe_webhook,
        card: cfg.card.clone(),
        operator_token_hash: cfg.operator_token_hash,
        default_limits: (cfg.max_concurrent_sessions, cfg.session_creates_per_hour),
    };
    let listener = tokio::net::TcpListener::bind(cfg.listen).await?;
    tracing::info!("control plane listening on {}", cfg.listen);
    axum::serve(listener, router(state))
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await?;
    Ok(())
}
