//! Aex's downstream Brain composition.
//!
//! Brain owns the engine and protocols, Hands implements the selected isolation adapter, and this
//! binary supplies only Aex's hosted capability set and outer configuration mapping.

use std::sync::Arc;

use brain::adapter::ToolExecutor;
use brain::api::{AppState, serve};
use brain::external::HttpExternalToolExecutor;
use brain::session::BrainConfig;
use brain_aws::AwsPersistenceConfig;
use hand_brain_aws::LambdaFactory;

const CAPABILITIES: [&str; 3] = ["aex.output.v1", "aex.web.search.v1", "aex.web.fetch.v1"];

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| {
                "brain=info,brain_aws=info,hand_brain_aws=info,aex_brain=info".into()
            }),
        )
        .init();

    let token = required("AEX_BRAIN_TOKEN")?;
    let executor_token = required("AEX_EXTERNAL_TOOL_EXECUTOR_TOKEN")?;
    let executor_url = std::env::var("AEX_CONTROL_INTERNAL_URL")
        .unwrap_or_else(|_| "http://127.0.0.1:8601/internal/v1/tools/call".into());
    validate_executor_url(&executor_url)?;
    let address = std::env::var("AEX_BRAIN_LISTEN")
        .unwrap_or_else(|_| "127.0.0.1:8700".into())
        .parse()?;

    // This Aex-owned binary registers one exact capability set. Generic BRAIN_* executor settings
    // cannot widen or redirect it accidentally.
    let config = BrainConfig {
        external_executor_url: None,
        external_executor_token: None,
        external_executor_capabilities: Default::default(),
        ..BrainConfig::default()
    };
    let external: Arc<dyn ToolExecutor> = Arc::new(HttpExternalToolExecutor::new(
        executor_url,
        Some(executor_token),
        config.external_call_timeout,
        CAPABILITIES.map(str::to_owned),
    ));

    let persistence = AwsPersistenceConfig::from_env().map_err(anyhow::Error::msg)?;
    let hands = Arc::new(
        LambdaFactory::from_env()
            .await
            .map_err(anyhow::Error::msg)?,
    );
    hands.verify().await.map_err(anyhow::Error::msg)?;
    let brain = brain_aws::compose(config, persistence, hands, Some(external))
        .await
        .map_err(anyhow::Error::msg)?;
    tracing::info!(capabilities = ?CAPABILITIES, "Aex hosted Brain composition ready");
    serve(AppState { brain, token }, address).await
}

fn required(name: &str) -> anyhow::Result<String> {
    let value = std::env::var(name).map_err(|_| anyhow::anyhow!("{name} is not set"))?;
    if value.trim().is_empty() {
        anyhow::bail!("{name} cannot be empty");
    }
    Ok(value)
}

fn validate_executor_url(value: &str) -> anyhow::Result<()> {
    let url = reqwest::Url::parse(value)
        .map_err(|error| anyhow::anyhow!("AEX_CONTROL_INTERNAL_URL: {error}"))?;
    if !matches!(url.scheme(), "http" | "https") {
        anyhow::bail!("AEX_CONTROL_INTERNAL_URL must use HTTP or HTTPS");
    }
    if !url.username().is_empty() || url.password().is_some() {
        anyhow::bail!("AEX_CONTROL_INTERNAL_URL must not contain credentials");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hosted_capability_set_is_exact_and_stable() {
        assert_eq!(
            CAPABILITIES,
            ["aex.output.v1", "aex.web.search.v1", "aex.web.fetch.v1"]
        );
        assert!(validate_executor_url("http://127.0.0.1:8601/internal/v1/tools/call").is_ok());
        assert!(validate_executor_url("file:///tmp/executor").is_err());
        assert!(validate_executor_url("http://user:secret@localhost/call").is_err());
    }
}
