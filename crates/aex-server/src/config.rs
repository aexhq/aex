use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::{collections::BTreeSet, net::SocketAddr, path::PathBuf};

#[derive(Clone, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub listen: SocketAddr,
    pub operator_listen: SocketAddr,
    pub data_dir: PathBuf,
    pub brain_url: String,
    pub agentloops: BTreeSet<String>,
    pub models: BTreeSet<String>,
    pub limits: Limits,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attachments: Option<crate::attachments::Config>,
}

#[derive(Clone, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Limits {
    pub accounts: u32,
    pub keys_per_account: u32,
    pub sessions_per_account: u32,
    pub hosts_per_account: u32,
    pub claims_per_account: u32,
    pub active_turns_per_account: u32,
    pub streams_per_account: usize,
    pub requests: usize,
    pub requests_per_account: usize,
    pub request_bytes: usize,
    pub response_bytes: usize,
    pub upstream_timeout_secs: u64,
    pub retained_bytes_per_account: u64,
    pub turn_reserve_bytes: u64,
    pub minimum_free_disk_bytes: u64,
    pub retention_secs: u64,
    pub usage_max_age_secs: u64,
    pub artifacts_per_account: u32,
    pub artifact_bytes_per_account: u64,
}

impl Config {
    pub fn validate(&self) -> anyhow::Result<()> {
        anyhow::ensure!(
            self.operator_listen.ip().is_loopback(),
            "operator listener must be loopback"
        );
        let url = url::Url::parse(&self.brain_url)?;
        anyhow::ensure!(
            matches!(url.scheme(), "http" | "https")
                && url.host_str().is_some()
                && url.username().is_empty()
                && url.password().is_none()
                && url.query().is_none()
                && url.fragment().is_none()
                && url.path() == "/",
            "brain_url must be an HTTP origin"
        );
        anyhow::ensure!(
            self.agentloops.iter().all(|id| id.len() == 64
                && id
                    .bytes()
                    .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))),
            "agentloops must contain approved SHA-256 addresses"
        );
        anyhow::ensure!(
            !self.models.is_empty()
                && self.models.iter().all(|m| m
                    .split_once('/')
                    .is_some_and(|(p, n)| !p.is_empty() && !n.is_empty())),
            "models must contain provider/model entries"
        );
        for (name, value) in serde_json::to_value(&self.limits)?.as_object().unwrap() {
            anyhow::ensure!(
                value
                    .as_u64()
                    .is_some_and(|v| v > 0 && v <= i64::MAX as u64),
                "{name} must be positive and fit database integers"
            );
        }
        anyhow::ensure!(
            self.limits.turn_reserve_bytes <= self.limits.retained_bytes_per_account,
            "turn reserve exceeds account storage"
        );
        if let Some(config) = &self.attachments {
            anyhow::ensure!(
                self.limits.requests >= 2 && self.limits.requests_per_account >= 2,
                "attachments require request capacity for provider downloads"
            );
            config.validate(self.limits.request_bytes)?;
        }
        Ok(())
    }
}
