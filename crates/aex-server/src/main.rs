use aex_server::{App, config::Config, operator::Operation};
use clap::{Parser, Subcommand};
use std::path::PathBuf;

#[derive(Parser)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}
#[derive(Subcommand)]
enum Command {
    Serve {
        #[arg(long)]
        config: PathBuf,
    },
    Operate {
        #[arg(long, default_value = "http://127.0.0.1:8082")]
        url: String,
        #[arg(long)]
        request: String,
    },
    Contract {
        #[arg(long)]
        output: PathBuf,
    },
}
fn secret(name: &str) -> anyhow::Result<String> {
    std::env::var(name).map_err(|_| anyhow::anyhow!("{name} is required"))
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .json()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .with_writer(std::io::stderr)
        .init();
    match Cli::parse().command {
        Command::Contract { output } => {
            std::fs::create_dir_all(&output)?;
            std::fs::write(
                output.join("config.schema.json"),
                format!(
                    "{}\n",
                    serde_json::to_string_pretty(&schemars::schema_for!(Config))?
                ),
            )?;
            std::fs::write(
                output.join("operator.schema.json"),
                format!(
                    "{}\n",
                    serde_json::to_string_pretty(&schemars::schema_for!(Operation))?
                ),
            )?;
        }
        Command::Operate { url, request } => {
            let origin = url::Url::parse(&url)?;
            anyhow::ensure!(
                origin.scheme() == "http"
                    && origin.host_str().is_some_and(|h| h
                        .parse::<std::net::IpAddr>()
                        .is_ok_and(|ip| ip.is_loopback())),
                "operator URL must be loopback HTTP; use an authenticated tunnel for remote access"
            );
            let request: Operation = serde_json::from_str(&request)?;
            let response = reqwest::Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .timeout(std::time::Duration::from_secs(60))
                .build()?
                .post(format!("{}/operate", url.trim_end_matches('/')))
                .bearer_auth(secret("AEX_OPERATOR_TOKEN")?)
                .json(&request)
                .send()
                .await?;
            anyhow::ensure!(
                response.status().is_success(),
                "operator request failed: {}",
                response.status()
            );
            println!("{}", response.text().await?);
        }
        Command::Serve { config } => {
            let config: Config = serde_json::from_slice(&std::fs::read(config)?)?;
            config.validate()?;
            std::fs::create_dir_all(&config.data_dir)?;
            let lock = std::fs::OpenOptions::new()
                .read(true)
                .write(true)
                .create(true)
                .truncate(false)
                .open(config.data_dir.join(".lock"))?;
            lock.try_lock()
                .map_err(|_| anyhow::anyhow!("Aex data directory already has a writer"))?;
            let listener = tokio::net::TcpListener::bind(config.listen).await?;
            let operator = tokio::net::TcpListener::bind(config.operator_listen).await?;
            let app = App::open(
                config,
                secret("AEX_BRAIN_TOKEN")?,
                secret("AEX_OPERATOR_TOKEN")?,
            )
            .await?;
            let stop = tokio_util::sync::CancellationToken::new();
            let public = axum::serve(listener, aex_server::http::router(app.clone()))
                .with_graceful_shutdown(stop.clone().cancelled_owned());
            let admin = axum::serve(operator, aex_server::operator::router(app))
                .with_graceful_shutdown(stop.clone().cancelled_owned());
            let shutdown = async {
                shutdown_signal().await;
                stop.cancel();
            };
            tokio::select! {
                result = async { tokio::try_join!(async { public.await }, async { admin.await }) } => { result?; },
                _ = shutdown => {},
            }
            drop(lock);
        }
    }
    Ok(())
}
async fn shutdown_signal() {
    #[cfg(unix)]
    {
        let mut term = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("install SIGTERM handler");
        tokio::select! { _ = term.recv() => {}, _ = tokio::signal::ctrl_c() => {} }
    }
    #[cfg(not(unix))]
    tokio::signal::ctrl_c()
        .await
        .expect("install Ctrl-C handler");
}
