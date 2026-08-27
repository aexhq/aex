use aex_control::{
    Config, PaymentsMode,
    api::{AppState, router},
    brain::BrainClient,
    payments::{FakePayments, Payments, StripePayments, StripeWebhook},
    store::Db,
};
use std::sync::Arc;

fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "aex_control=info".into()),
        )
        .init();
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    runtime.block_on(run(Config::from_env()?))
}

async fn run(config: Config) -> anyhow::Result<()> {
    let (payments, stripe_webhook): (Arc<dyn Payments>, Option<StripeWebhook>) =
        match config.payments {
            PaymentsMode::Fake => {
                tracing::warn!("fake payments are enabled; no money moves");
                (Arc::new(FakePayments), None)
            }
            PaymentsMode::Stripe {
                secret_key,
                webhook_secret,
            } => (
                Arc::new(StripePayments::new(
                    secret_key,
                    config.topup_success_url.clone(),
                    config.topup_cancel_url.clone(),
                )),
                Some(StripeWebhook::new(webhook_secret)),
            ),
        };
    let state = AppState {
        db: Db::open(&config.db_path)?,
        brain: BrainClient::new(&config.brain_url, config.brain_token)?,
        payments,
        stripe_webhook,
        operator_token_hash: config.operator_token_hash,
        default_limits: (
            config.max_concurrent_sessions,
            config.session_creates_per_hour,
        ),
    };
    let listener = tokio::net::TcpListener::bind(config.listen).await?;
    tracing::info!(listen = %config.listen, brain = %config.brain_url, "Aex control plane ready");
    axum::serve(listener, router(state))
        .with_graceful_shutdown(async move {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await?;
    Ok(())
}
