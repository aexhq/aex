//! Aex's hosted identity boundary and authenticated proxy for Brain.

use std::{net::SocketAddr, path::PathBuf};

pub mod api;
pub mod billing;
pub mod brain;
pub mod identity;
pub mod payments;
pub mod rating;
pub mod store;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{0}")]
    Invalid(String),
    #[error("{0}")]
    Unprocessable(String),
    #[error("{0}")]
    OutputSchema(String),
    #[error("unauthorized")]
    Unauthorized,
    #[error("{0}")]
    Forbidden(String),
    #[error("not found")]
    NotFound,
    #[error("{0}")]
    Conflict(String),
    #[error("{0}")]
    InsufficientBalance(String),
    #[error("{0}")]
    RateLimited(String),
    #[error("{0}")]
    PayloadTooLarge(String),
    #[error("{0}")]
    StorageQuota(String),
    #[error("payment provider: {0}")]
    Payment(String),
    #[error("brain: {0}")]
    Upstream(String),
    #[error("{0}")]
    Internal(String),
}

pub type Result<T> = std::result::Result<T, Error>;

impl Error {
    pub fn code(&self) -> &'static str {
        match self {
            Self::Invalid(_) | Self::Unprocessable(_) => "invalid_request",
            Self::OutputSchema(_) => "output_schema_error",
            Self::Unauthorized => "unauthorized",
            Self::Forbidden(_) => "forbidden",
            Self::NotFound => "not_found",
            Self::Conflict(_) => "conflict",
            Self::InsufficientBalance(_) => "insufficient_balance",
            Self::RateLimited(_) => "rate_limited",
            Self::PayloadTooLarge(_) => "payload_too_large",
            Self::StorageQuota(_) => "storage_quota_exceeded",
            Self::Payment(_) => "payment_error",
            Self::Upstream(_) => "upstream_error",
            Self::Internal(_) => "internal",
        }
    }

    pub fn status(&self) -> u16 {
        match self {
            Self::Invalid(_) | Self::OutputSchema(_) => 400,
            Self::Unprocessable(_) => 422,
            Self::Unauthorized => 401,
            Self::Forbidden(_) => 403,
            Self::NotFound => 404,
            Self::Conflict(_) | Self::StorageQuota(_) => 409,
            Self::InsufficientBalance(_) => 402,
            Self::RateLimited(_) => 429,
            Self::PayloadTooLarge(_) => 413,
            Self::Payment(_) | Self::Upstream(_) => 502,
            Self::Internal(_) => 500,
        }
    }
}

pub const MAX_HOSTED_SESSION_STORAGE_BYTES: u64 = 10 * 1024 * 1024 * 1024;

pub enum PaymentsMode {
    Fake,
    Stripe {
        secret_key: String,
        webhook_secret: String,
    },
}

pub fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

pub fn rfc3339(ms: i64) -> String {
    chrono::DateTime::from_timestamp_millis(ms)
        .unwrap_or_else(|| panic!("timestamp {ms}ms is outside the representable range"))
        .to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

pub fn parse_rfc3339_ms(value: &str) -> Option<i64> {
    chrono::DateTime::parse_from_rfc3339(value)
        .ok()
        .map(|time| time.timestamp_millis())
}

pub fn usd_display(microusd: i64) -> String {
    let sign = if microusd < 0 { "-" } else { "" };
    let absolute = microusd.unsigned_abs();
    format!(
        "{sign}{}.{:02}",
        absolute / 1_000_000,
        (absolute % 1_000_000) / 10_000
    )
}

pub struct Config {
    pub listen: SocketAddr,
    pub brain_url: String,
    pub brain_token: String,
    pub db_path: PathBuf,
    pub payments: PaymentsMode,
    pub topup_success_url: String,
    pub topup_cancel_url: String,
    pub operator_token_hash: Option<String>,
    pub max_concurrent_sessions: i64,
    pub session_creates_per_hour: i64,
}

impl Config {
    pub fn from_env() -> anyhow::Result<Self> {
        fn positive_limit(name: &str, default: i64) -> anyhow::Result<i64> {
            let value = std::env::var(name).map_or(Ok(default), |value| {
                value
                    .parse::<i64>()
                    .map_err(|error| anyhow::anyhow!("{name}: {error}"))
            })?;
            anyhow::ensure!((1..=1_000_000).contains(&value), "{name} is out of range");
            Ok(value)
        }
        let listen = std::env::var("AEX_CONTROL_LISTEN")
            .unwrap_or_else(|_| "127.0.0.1:8600".into())
            .parse()
            .map_err(|error| anyhow::anyhow!("AEX_CONTROL_LISTEN: {error}"))?;
        let brain_token = std::env::var("AEX_BRAIN_TOKEN")
            .map_err(|_| anyhow::anyhow!("AEX_BRAIN_TOKEN is not set"))?;
        anyhow::ensure!(!brain_token.is_empty(), "AEX_BRAIN_TOKEN cannot be empty");
        let payments = match std::env::var("AEX_PAYMENTS").as_deref() {
            Ok("fake") => PaymentsMode::Fake,
            Ok("stripe") => PaymentsMode::Stripe {
                secret_key: std::env::var("STRIPE_SECRET_KEY")
                    .map_err(|_| anyhow::anyhow!("AEX_PAYMENTS=stripe needs STRIPE_SECRET_KEY"))?,
                webhook_secret: std::env::var("STRIPE_WEBHOOK_SECRET").map_err(|_| {
                    anyhow::anyhow!("AEX_PAYMENTS=stripe needs STRIPE_WEBHOOK_SECRET")
                })?,
            },
            Ok(value) => anyhow::bail!("AEX_PAYMENTS={value}: expected fake or stripe"),
            Err(_) => anyhow::bail!("AEX_PAYMENTS is not set (expected fake or stripe)"),
        };
        let operator_token_hash = match std::env::var("AEX_OPERATOR_TOKEN") {
            Ok(token)
                if token.starts_with("aex_ad_")
                    && (40..=64).contains(&(token.len() - "aex_ad_".len()))
                    && token["aex_ad_".len()..]
                        .bytes()
                        .all(|byte| byte.is_ascii_alphanumeric()) =>
            {
                Some(identity::hash_secret(&token))
            }
            Ok(_) => anyhow::bail!(
                "AEX_OPERATOR_TOKEN must be aex_ad_ followed by 40 to 64 alphanumeric characters"
            ),
            Err(_) => None,
        };
        Ok(Self {
            listen,
            brain_url: std::env::var("AEX_BRAIN_URL")
                .unwrap_or_else(|_| "http://127.0.0.1:8700".into()),
            brain_token,
            db_path: std::env::var("AEX_CONTROL_DB")
                .unwrap_or_else(|_| "./aex-control-data/control.db".into())
                .into(),
            payments,
            topup_success_url: std::env::var("AEX_TOPUP_SUCCESS_URL")
                .unwrap_or_else(|_| "https://aex.dev/topup/success".into()),
            topup_cancel_url: std::env::var("AEX_TOPUP_CANCEL_URL")
                .unwrap_or_else(|_| "https://aex.dev/topup/cancelled".into()),
            operator_token_hash,
            max_concurrent_sessions: positive_limit("AEX_LIMIT_CONCURRENT_SESSIONS", 10)?,
            session_creates_per_hour: positive_limit("AEX_LIMIT_SESSION_CREATES_PER_HOUR", 30)?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn money_and_time_render_deterministically() {
        assert_eq!(usd_display(9_989_194), "9.98");
        let ms = 1_787_046_300_000;
        assert_eq!(parse_rfc3339_ms(&rfc3339(ms)), Some(ms));
    }
}
