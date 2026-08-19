//! The aex control plane: identity, prepaid billing, session authority, rated usage.
//!
//! One process in front of one brain. It owns accounts, API keys, the prepaid ledger and the
//! meters; it serves the control API (`contracts/control/v1`) and every `session/v1` path
//! verbatim as an authorizing, admitting, metering proxy. The brain's journal is the billing
//! record: compute time is folded from each session's event log (turn intervals — the
//! pre-suspend idle window is absorbed, ARCHITECTURE-v1 D4), storage from the brain-reported
//! byte meters integrated over wall time.
//!
//! Substrate posture matches the brain: SQLite on local disk by default (one file, zero
//! config), payments faked by default with a loud banner; `AEX_PAYMENTS=stripe` +
//! `STRIPE_SECRET_KEY` is the configured production path.

pub mod api;
pub mod brain;
pub mod identity;
pub mod payments;
pub mod rating;
pub mod store;
pub mod sweep;

use std::net::SocketAddr;
use std::path::PathBuf;

/// Control-plane failure, mapped 1:1 onto `contracts/control/v1` `ControlErrorCode`.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{0}")]
    Invalid(String),
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
            Error::Invalid(_) => "invalid_request",
            Error::Unauthorized => "unauthorized",
            Error::Forbidden(_) => "forbidden",
            Error::NotFound => "not_found",
            Error::Conflict(_) => "conflict",
            Error::InsufficientBalance(_) => "insufficient_balance",
            Error::RateLimited(_) => "rate_limited",
            Error::Payment(_) => "payment_error",
            Error::Upstream(_) => "upstream_error",
            Error::Internal(_) => "internal",
        }
    }

    pub fn status(&self) -> u16 {
        match self {
            Error::Invalid(_) => 400,
            Error::Unauthorized => 401,
            Error::Forbidden(_) => 403,
            Error::NotFound => 404,
            Error::Conflict(_) => 409,
            Error::InsufficientBalance(_) => 402,
            Error::RateLimited(_) => 429,
            Error::Payment(_) => 502,
            Error::Upstream(_) => 502,
            Error::Internal(_) => 500,
        }
    }
}

/// Epoch milliseconds now.
pub fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

/// Epoch milliseconds -> RFC 3339 UTC with milliseconds ("2026-08-18T09:45:00.000Z") — billing
/// folds on event timestamps, so the sub-second part is load-bearing.
pub fn rfc3339(ms: i64) -> String {
    chrono::DateTime::from_timestamp_millis(ms)
        .unwrap_or_default()
        .to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

/// RFC 3339 -> epoch milliseconds; None if unparseable.
pub fn parse_rfc3339_ms(s: &str) -> Option<i64> {
    chrono::DateTime::parse_from_rfc3339(s)
        .ok()
        .map(|t| t.timestamp_millis())
}

/// Micro-USD -> display dollars, whole cents, rounded toward zero ("9.98", "-0.07").
pub fn usd_display(microusd: i64) -> String {
    let sign = if microusd < 0 { "-" } else { "" };
    let abs = microusd.unsigned_abs();
    format!(
        "{sign}{}.{:02}",
        abs / 1_000_000,
        (abs % 1_000_000) / 10_000
    )
}

/// How top-ups are paid.
pub enum PaymentsMode {
    /// Auto-paid, no money moves. The local default; the server banners it loudly.
    Fake,
    /// Stripe Checkout with this secret key.
    Stripe {
        secret_key: String,
        webhook_secret: String,
    },
}

/// Server configuration. Fail fast: `AEX_BRAIN_TOKEN` has no default, malformed numbers are
/// errors, `AEX_PAYMENTS=stripe` without a key is an error.
pub struct Config {
    pub listen: SocketAddr,
    pub brain_url: String,
    pub brain_token: String,
    pub db_path: PathBuf,
    pub payments: PaymentsMode,
    pub topup_success_url: String,
    pub topup_cancel_url: String,
    pub card: rating::RateCard,
    pub operator_token_hash: Option<String>,
    pub max_concurrent_sessions: i64,
    pub session_creates_per_hour: i64,
    pub sweep_seconds: u64,
}

impl Config {
    pub fn from_env() -> anyhow::Result<Self> {
        fn num<T: std::str::FromStr>(name: &str, default: T) -> anyhow::Result<T>
        where
            T::Err: std::fmt::Display,
        {
            match std::env::var(name) {
                Ok(v) => v.parse().map_err(|e| anyhow::anyhow!("{name}={v}: {e}")),
                Err(_) => Ok(default),
            }
        }
        let payments = match std::env::var("AEX_PAYMENTS").as_deref() {
            Err(_) | Ok("fake") => PaymentsMode::Fake,
            Ok("stripe") => PaymentsMode::Stripe {
                secret_key: std::env::var("STRIPE_SECRET_KEY")
                    .map_err(|_| anyhow::anyhow!("AEX_PAYMENTS=stripe needs STRIPE_SECRET_KEY"))?,
                webhook_secret: std::env::var("STRIPE_WEBHOOK_SECRET").map_err(|_| {
                    anyhow::anyhow!("AEX_PAYMENTS=stripe needs STRIPE_WEBHOOK_SECRET")
                })?,
            },
            Ok(other) => anyhow::bail!("AEX_PAYMENTS={other}: expected fake or stripe"),
        };
        let operator_token_hash = match std::env::var("AEX_OPERATOR_TOKEN") {
            Ok(token)
                if token.starts_with("aex_ad_")
                    && token.len() >= "aex_ad_".len() + 40
                    && token.len() <= "aex_ad_".len() + 64
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
        Ok(Config {
            listen: num("AEX_CONTROL_LISTEN", "127.0.0.1:8600".parse()?)?,
            brain_url: std::env::var("AEX_BRAIN_URL")
                .unwrap_or_else(|_| "http://127.0.0.1:8700".into()),
            brain_token: std::env::var("AEX_BRAIN_TOKEN").map_err(|_| {
                anyhow::anyhow!("AEX_BRAIN_TOKEN is not set (the brain's AEX_API_TOKEN)")
            })?,
            db_path: std::env::var("AEX_CONTROL_DB")
                .unwrap_or_else(|_| "./aex-control-data/control.db".into())
                .into(),
            payments,
            topup_success_url: std::env::var("AEX_TOPUP_SUCCESS_URL")
                .unwrap_or_else(|_| "https://aex.dev/topup/success".into()),
            topup_cancel_url: std::env::var("AEX_TOPUP_CANCEL_URL")
                .unwrap_or_else(|_| "https://aex.dev/topup/cancelled".into()),
            card: rating::RateCard::from_env()?,
            operator_token_hash,
            max_concurrent_sessions: num("AEX_LIMIT_CONCURRENT_SESSIONS", 10)?,
            session_creates_per_hour: num("AEX_LIMIT_SESSION_CREATES_PER_HOUR", 30)?,
            sweep_seconds: num("AEX_SWEEP_SECONDS", 30)?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn usd_display_rounds_toward_zero_both_signs() {
        assert_eq!(usd_display(9_989_194), "9.98");
        assert_eq!(usd_display(-70_000), "-0.07");
        assert_eq!(usd_display(0), "0.00");
        assert_eq!(usd_display(10_000_000), "10.00");
        assert_eq!(usd_display(-9_999), "-0.00");
    }

    #[test]
    fn rfc3339_round_trips_ms() {
        let ms = 1_787_046_300_000; // 2026-08-18T09:45:00Z
        assert_eq!(rfc3339(ms), "2026-08-18T09:45:00.000Z");
        assert_eq!(parse_rfc3339_ms(&rfc3339(ms)), Some(ms));
        assert_eq!(
            parse_rfc3339_ms("2026-08-18T09:45:00.250Z"),
            Some(1_787_046_300_250)
        );
        assert_eq!(parse_rfc3339_ms("nope"), None);
    }
}
