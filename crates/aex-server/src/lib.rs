pub mod account;
pub mod admission;
pub mod artifacts;
pub mod brain;
pub mod config;
pub mod error;
pub mod hosts;
pub mod http;
pub mod identity;
pub mod operator;
pub mod sessions;
pub mod store;

use std::{collections::HashMap, sync::Arc};
use tokio::sync::{Mutex, Semaphore, watch};

#[derive(Clone)]
pub struct App {
    pub operator_lock: Arc<Mutex<()>>,
    pub accepting: Arc<std::sync::atomic::AtomicBool>,
    pub config: Arc<config::Config>,
    pub store: store::Store,
    pub brain: brain::Brain,
    pub changed: watch::Sender<u64>,
    pub requests: Arc<Semaphore>,
    pub account_requests: Arc<Mutex<HashMap<String, Arc<Semaphore>>>>,
    pub streams: Arc<Mutex<HashMap<String, Arc<Semaphore>>>>,
    pub host_registration: Arc<Mutex<()>>,
    pub artifact_admission: Arc<Mutex<()>>,
    pub operator_verifier: String,
    pub site_verifier: String,
}
impl App {
    pub async fn open(
        config: config::Config,
        brain_token: String,
        operator_token: String,
        database_url: String,
        site_token: String,
    ) -> anyhow::Result<Self> {
        config.validate()?;
        anyhow::ensure!(
            brain_token.len() >= 32 && operator_token.len() >= 32 && site_token.len() >= 32,
            "internal credentials require at least 32 characters"
        );
        anyhow::ensure!(
            brain_token != operator_token
                && site_token != brain_token
                && site_token != operator_token,
            "internal credentials must differ"
        );
        std::fs::create_dir_all(&config.data_dir)?;
        let store = store::Store::open(&database_url).await?;
        let brain = brain::Brain::new(&config, brain_token)?;
        Ok(Self {
            operator_lock: Arc::default(),
            accepting: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            requests: Arc::new(Semaphore::new(config.limits.requests)),
            account_requests: Arc::default(),
            config: Arc::new(config),
            store,
            brain,
            changed: watch::channel(0).0,
            streams: Arc::default(),
            host_registration: Arc::default(),
            artifact_admission: Arc::default(),
            operator_verifier: identity::digest(operator_token.as_bytes()),
            site_verifier: identity::digest(site_token.as_bytes()),
        })
    }
}
