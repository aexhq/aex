pub mod account;
pub mod admission;
pub mod artifacts;
pub mod attachments;
pub mod billing;
pub mod brain;
pub mod config;
pub mod environments;
pub mod error;
pub mod hosts;
pub mod http;
pub mod identity;
pub mod model_usage;
pub mod operator;
pub mod payments;
pub mod sessions;
pub mod store;
pub mod turns;

use std::{collections::HashMap, sync::Arc};
use tokio::sync::{Mutex, Semaphore, watch};

#[derive(Clone)]
pub struct App {
    pub operator_lock: Arc<Mutex<()>>,
    pub accepting: Arc<std::sync::atomic::AtomicBool>,
    pub config: Arc<config::Config>,
    pub store: store::Store,
    pub attachment_storage: Option<Arc<dyn attachments::storage::Storage>>,
    pub payments: Option<Arc<payments::Stripe>>,
    pub environment_token: Option<String>,
    pub attachment_uploads: Arc<tokio::sync::RwLock<()>>,
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
        let environment_token = if let Some(environments) = &config.environments {
            let token = std::env::var("AEX_ENVIRONMENT_TOKEN")?;
            anyhow::ensure!(
                token.len() >= 32
                    && token != brain_token
                    && token != operator_token
                    && token != site_token,
                "Environment credential must be distinct and contain at least 32 characters"
            );
            environments::initialize(&store, environments).await?;
            Some(token)
        } else {
            None
        };
        if let Some(billing) = &config.billing {
            billing::initialize(&store, billing).await?;
        }
        let payments = config
            .billing
            .as_ref()
            .and_then(|b| b.payments.as_ref())
            .map(payments::Stripe::from_env)
            .transpose()?
            .map(Arc::new);
        let brain = brain::Brain::new(&config, brain_token)?;
        let attachment_storage = config
            .attachments
            .as_ref()
            .map(attachments::storage::S3::new)
            .transpose()?
            .map(|storage| Arc::new(storage) as Arc<dyn attachments::storage::Storage>);
        Ok(Self {
            operator_lock: Arc::default(),
            accepting: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            requests: Arc::new(Semaphore::new(config.limits.requests)),
            account_requests: Arc::default(),
            config: Arc::new(config),
            store,
            attachment_storage,
            payments,
            environment_token,
            attachment_uploads: Arc::default(),
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
