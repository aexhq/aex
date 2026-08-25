//! The Aex control plane: identity, prepaid billing, session authority, rated usage.
//!
//! One process in front of one brain. It owns accounts, API keys, the prepaid ledger and the
//! meters; it serves the control API (`contracts/control/v1`) and every `session/v1` path
//! verbatim as an authorizing, admitting, metering proxy. The brain's journal is the billing
//! record: compute time is folded from each session's event log (turn intervals, with the
//! pre-suspend idle window absorbed), and storage comes from durable Brain `storage.usage`
//! transitions integrated exactly in byte-milliseconds.
//!
//! Substrate posture matches the brain: SQLite on local disk by default (one file, zero
//! config), payments faked by default with a loud banner; `AEX_PAYMENTS=stripe` +
//! `STRIPE_SECRET_KEY` is the configured production path.

pub mod admission;
pub mod api;
pub mod brain;
pub mod customer_environment;
pub mod identity;
pub mod outbound;
pub mod output;
pub mod payments;
pub mod rating;
pub mod store;
pub mod sweep;
pub mod web;

use std::net::SocketAddr;
use std::path::PathBuf;

/// Control-plane failure. `code()` maps onto the UNION of two vocabularies: the
/// `contracts/control/v1` `ControlErrorCode` enum, plus the three session-vocabulary codes
/// (`output_schema_error`, `file_too_large`, `storage_quota_exceeded`) that only
/// session-scoped routes produce (output validation, uploads, storage quota). A test below
/// pins every variant to one of the two sets so a new code cannot drift off-contract
/// silently; the full per-route typed split is recorded in the audit backlog.
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
            Error::Invalid(_) | Error::Unprocessable(_) => "invalid_request",
            Error::OutputSchema(_) => "output_schema_error",
            Error::Unauthorized => "unauthorized",
            Error::Forbidden(_) => "forbidden",
            Error::NotFound => "not_found",
            Error::Conflict(_) => "conflict",
            Error::InsufficientBalance(_) => "insufficient_balance",
            Error::RateLimited(_) => "rate_limited",
            Error::PayloadTooLarge(_) => "file_too_large",
            Error::StorageQuota(_) => "storage_quota_exceeded",
            Error::Payment(_) => "payment_error",
            Error::Upstream(_) => "upstream_error",
            Error::Internal(_) => "internal",
        }
    }

    pub fn status(&self) -> u16 {
        match self {
            Error::Invalid(_) | Error::OutputSchema(_) => 400,
            Error::Unprocessable(_) => 422,
            Error::Unauthorized => 401,
            Error::Forbidden(_) => 403,
            Error::NotFound => 404,
            Error::Conflict(_) => 409,
            Error::InsufficientBalance(_) => 402,
            Error::RateLimited(_) => 429,
            Error::PayloadTooLarge(_) => 413,
            Error::StorageQuota(_) => 409,
            Error::Payment(_) => 502,
            Error::Upstream(_) => 502,
            Error::Internal(_) => 500,
        }
    }
}

#[cfg(test)]
mod error_code_contract {
    /// Every code `Error::code()` can emit is either in the control contract's enum or in
    /// the named session-vocabulary set. Adding a variant forces a conscious classification.
    #[test]
    fn every_error_code_is_classified() {
        let schemas: serde_json::Value =
            serde_json::from_str(include_str!("../../../contracts/control/v1/schemas.json"))
                .expect("control schemas parse");
        let control: Vec<String> = schemas["$defs"]["ControlErrorCode"]["enum"]
            .as_array()
            .expect("ControlErrorCode enum")
            .iter()
            .map(|v| v.as_str().expect("code string").to_owned())
            .collect();
        let session_only = [
            "output_schema_error",
            "file_too_large",
            "storage_quota_exceeded",
        ];
        let all = [
            super::Error::Invalid(String::new()).code(),
            super::Error::Unprocessable(String::new()).code(),
            super::Error::OutputSchema(String::new()).code(),
            super::Error::Unauthorized.code(),
            super::Error::Forbidden(String::new()).code(),
            super::Error::NotFound.code(),
            super::Error::Conflict(String::new()).code(),
            super::Error::InsufficientBalance(String::new()).code(),
            super::Error::RateLimited(String::new()).code(),
            super::Error::PayloadTooLarge(String::new()).code(),
            super::Error::StorageQuota(String::new()).code(),
            super::Error::Payment(String::new()).code(),
            super::Error::Upstream(String::new()).code(),
            super::Error::Internal(String::new()).code(),
        ];
        for code in all {
            assert!(
                control.iter().any(|c| c == code) || session_only.contains(&code),
                "error code {code:?} is in neither the control contract nor the session set"
            );
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
    // Billing responses must never render a plausible 1970 lie for an out-of-range value.
    chrono::DateTime::from_timestamp_millis(ms)
        .unwrap_or_else(|| panic!("timestamp {ms}ms is outside the representable range"))
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

#[derive(Debug, Clone, Copy)]
pub struct StorageLimits {
    pub max_object_bytes: u64,
    pub max_session_bytes: u64,
}

/// Hosted alpha's public, authoritative per-session published-plus-reserved storage ceiling.
pub const MAX_HOSTED_SESSION_STORAGE_BYTES: u64 = 10 * 1024 * 1024 * 1024;

impl Default for StorageLimits {
    fn default() -> Self {
        Self {
            max_object_bytes: 512 * 1024 * 1024,
            max_session_bytes: MAX_HOSTED_SESSION_STORAGE_BYTES,
        }
    }
}

/// Server configuration. Fail fast: `AEX_BRAIN_TOKEN` has no default, malformed numbers are
/// errors, `AEX_PAYMENTS=stripe` without a key is an error.
pub struct Config {
    pub listen: SocketAddr,
    pub internal_listen: SocketAddr,
    pub brain_url: String,
    pub brain_token: String,
    pub db_path: PathBuf,
    pub payments: PaymentsMode,
    pub topup_success_url: String,
    pub topup_cancel_url: String,
    pub card: rating::RateCard,
    pub operator_token_hash: Option<String>,
    pub external_executor_token_hash: Option<String>,
    pub customer_environment_gateway: Option<customer_environment::CustomerEnvironmentGateway>,
    pub serper_api_key: Option<String>,
    pub max_concurrent_sessions: i64,
    pub max_retained_root_sessions: i64,
    pub session_creates_per_hour: i64,
    pub max_concurrent_create_bodies: usize,
    pub max_concurrent_message_bodies: usize,
    pub max_concurrent_inline_session_bodies: usize,
    pub sweep_seconds: u64,
    pub discovery_overlap_ms: i64,
    pub discovery_session_limit: usize,
    pub discovery_partitions: u16,
    pub storage_limits: StorageLimits,
    pub admission: admission::AdmissionConfig,
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
        let internal_listen: SocketAddr =
            num("AEX_CONTROL_INTERNAL_LISTEN", "127.0.0.1:8601".parse()?)?;
        if !internal_listen.ip().is_loopback() {
            anyhow::bail!(
                "AEX_CONTROL_INTERNAL_LISTEN must use a loopback address; got {internal_listen}"
            );
        }
        let customer_environment_gateway = match std::env::var(
            "AEX_CUSTOMER_ENVIRONMENT_GATEWAY_TOKEN",
        ) {
            Ok(token) => {
                let trusted = std::env::var("AEX_CUSTOMER_ENVIRONMENT_TRUSTED_PROXY_CIDRS").map_err(|_| {
                    anyhow::anyhow!(
                        "AEX_CUSTOMER_ENVIRONMENT_GATEWAY_TOKEN needs AEX_CUSTOMER_ENVIRONMENT_TRUSTED_PROXY_CIDRS"
                    )
                })?;
                let blocked = std::env::var("AEX_MANAGED_ENVIRONMENT_NAT_CIDRS").map_err(|_| {
                    anyhow::anyhow!(
                        "AEX_CUSTOMER_ENVIRONMENT_GATEWAY_TOKEN needs AEX_MANAGED_ENVIRONMENT_NAT_CIDRS"
                    )
                })?;
                Some(
                    customer_environment::CustomerEnvironmentGateway::new(
                        &token,
                        customer_environment::parse_cidrs(
                            "AEX_CUSTOMER_ENVIRONMENT_TRUSTED_PROXY_CIDRS",
                            &trusted,
                        )?,
                        customer_environment::parse_cidrs(
                            "AEX_MANAGED_ENVIRONMENT_NAT_CIDRS",
                            &blocked,
                        )?,
                    )
                    .map_err(|error| anyhow::anyhow!("customer-environment gateway: {error}"))?,
                )
            }
            Err(_) => None,
        };
        let sweep_seconds = num("AEX_SWEEP_SECONDS", 30u64)?;
        if !(1..=3_600).contains(&sweep_seconds) {
            anyhow::bail!("AEX_SWEEP_SECONDS must be between 1 and 3600");
        }
        let discovery_overlap_seconds = num(
            "AEX_DISCOVERY_OVERLAP_SECONDS",
            sweep_seconds.saturating_mul(2).max(120),
        )?;
        let discovery_overlap_ms = i64::try_from(discovery_overlap_seconds)
            .map_err(|_| anyhow::anyhow!("AEX_DISCOVERY_OVERLAP_SECONDS is too large"))?
            .checked_mul(1_000)
            .ok_or_else(|| anyhow::anyhow!("AEX_DISCOVERY_OVERLAP_SECONDS is too large"))?;
        if discovery_overlap_ms <= 0 {
            anyhow::bail!("AEX_DISCOVERY_OVERLAP_SECONDS must be positive");
        }
        let discovery_session_limit = num("AEX_DISCOVERY_SESSION_LIMIT", 100_000usize)?;
        if !(1..=1_000_000).contains(&discovery_session_limit) {
            anyhow::bail!("AEX_DISCOVERY_SESSION_LIMIT must be between 1 and 1000000");
        }
        let discovery_partitions = num("AEX_DISCOVERY_PARTITIONS", 1u16)?;
        if !(1..=256).contains(&discovery_partitions) {
            anyhow::bail!("AEX_DISCOVERY_PARTITIONS must be between 1 and 256");
        }
        let storage_limits = StorageLimits {
            max_object_bytes: num(
                "AEX_STORAGE_MAX_OBJECT_BYTES",
                StorageLimits::default().max_object_bytes,
            )?,
            max_session_bytes: num(
                "AEX_STORAGE_MAX_SESSION_BYTES",
                StorageLimits::default().max_session_bytes,
            )?,
        };
        if storage_limits.max_object_bytes == 0
            || storage_limits.max_session_bytes < storage_limits.max_object_bytes
            || storage_limits.max_session_bytes > MAX_HOSTED_SESSION_STORAGE_BYTES
        {
            anyhow::bail!(
                "AEX_STORAGE_MAX_SESSION_BYTES must be at least the positive AEX_STORAGE_MAX_OBJECT_BYTES and at most 10 GiB"
            );
        }
        let default_admission_seconds = sweep_seconds.saturating_mul(2).max(60);
        let admission_stale_seconds =
            num("AEX_ADMISSION_STALE_SECONDS", default_admission_seconds)?;
        let admission_reservation_seconds = num(
            "AEX_ADMISSION_RESERVATION_SECONDS",
            default_admission_seconds,
        )?;
        let admission = admission::AdmissionConfig {
            action_exposure_microusd: num("AEX_ADMISSION_ACTION_EXPOSURE_MICROUSD", 100_000)?,
            account_exposure_microusd: num("AEX_ADMISSION_ACCOUNT_EXPOSURE_MICROUSD", 1_000_000)?,
            low_balance_microusd: num("AEX_ADMISSION_LOW_BALANCE_MICROUSD", 1_000_000)?,
            stale_after_ms: i64::try_from(admission_stale_seconds)
                .map_err(|_| anyhow::anyhow!("AEX_ADMISSION_STALE_SECONDS is too large"))?
                .checked_mul(1_000)
                .ok_or_else(|| anyhow::anyhow!("AEX_ADMISSION_STALE_SECONDS is too large"))?,
            reservation_ttl_ms: i64::try_from(admission_reservation_seconds)
                .map_err(|_| anyhow::anyhow!("AEX_ADMISSION_RESERVATION_SECONDS is too large"))?
                .checked_mul(1_000)
                .ok_or_else(|| anyhow::anyhow!("AEX_ADMISSION_RESERVATION_SECONDS is too large"))?,
            max_cached_accounts: num("AEX_ADMISSION_MAX_CACHED_ACCOUNTS", 10_000usize)?,
        }
        .validate()?;
        let max_concurrent_create_bodies = num("AEX_MAX_CONCURRENT_CREATE_BODIES", 4usize)?;
        if !(1..=64).contains(&max_concurrent_create_bodies) {
            anyhow::bail!("AEX_MAX_CONCURRENT_CREATE_BODIES must be between 1 and 64");
        }
        let max_concurrent_message_bodies = num("AEX_MAX_CONCURRENT_MESSAGE_BODIES", 256usize)?;
        if !(1..=4_096).contains(&max_concurrent_message_bodies) {
            anyhow::bail!("AEX_MAX_CONCURRENT_MESSAGE_BODIES must be between 1 and 4096");
        }
        let max_concurrent_inline_session_bodies =
            num("AEX_MAX_CONCURRENT_INLINE_SESSION_BODIES", 64usize)?;
        if !(1..=1_024).contains(&max_concurrent_inline_session_bodies) {
            anyhow::bail!("AEX_MAX_CONCURRENT_INLINE_SESSION_BODIES must be between 1 and 1024");
        }
        let max_concurrent_sessions = num("AEX_LIMIT_CONCURRENT_SESSIONS", 10)?;
        let max_retained_root_sessions = num("AEX_LIMIT_RETAINED_ROOT_SESSIONS", 100)?;
        let session_creates_per_hour = num("AEX_LIMIT_SESSION_CREATES_PER_HOUR", 30)?;
        if !(1..=1_000_000).contains(&max_concurrent_sessions)
            || max_retained_root_sessions < max_concurrent_sessions
            || max_retained_root_sessions > 1_000_000
            || !(1..=1_000_000).contains(&session_creates_per_hour)
        {
            anyhow::bail!(
                "root concurrent, retained, and hourly-create limits must be between 1 and 1000000, with retained at least concurrent"
            );
        }
        Ok(Config {
            listen: num("AEX_CONTROL_LISTEN", "127.0.0.1:8600".parse()?)?,
            internal_listen,
            brain_url: std::env::var("AEX_BRAIN_URL")
                .unwrap_or_else(|_| "http://127.0.0.1:8700".into()),
            brain_token: std::env::var("AEX_BRAIN_TOKEN")
                .map_err(|_| anyhow::anyhow!("AEX_BRAIN_TOKEN is not set"))?,
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
            external_executor_token_hash: std::env::var("AEX_EXTERNAL_TOOL_EXECUTOR_TOKEN")
                .ok()
                .filter(|token| !token.is_empty())
                .map(|token| identity::hash_secret(&token)),
            customer_environment_gateway,
            serper_api_key: std::env::var("SERPER_API_KEY")
                .ok()
                .filter(|key| !key.is_empty()),
            max_concurrent_sessions,
            max_retained_root_sessions,
            session_creates_per_hour,
            max_concurrent_create_bodies,
            max_concurrent_message_bodies,
            max_concurrent_inline_session_bodies,
            sweep_seconds,
            discovery_overlap_ms,
            discovery_session_limit,
            discovery_partitions,
            storage_limits,
            admission,
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
