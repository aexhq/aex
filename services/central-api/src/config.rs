//! Total, fail-fast start-up configuration for `central-api`.
//!
//! Nothing here has a default. A value that identifies a plane, a region, a
//! cluster or a bound is supplied explicitly, because a defaulted one binds a
//! process to the wrong plane without saying so — and this process holds the
//! control, identity and finance surfaces at once, so the blast radius of a
//! silent misbinding is the whole central plane.
//!
//! # One login, one cluster, one process
//!
//! The three deployables this one merges each connect to Aurora through the
//! cluster's RDS-managed master secret, and so does this one: a single
//! [`AURORA_SECRET_ARN`] against a single cluster, exactly the shape
//! `central-control-api`, `central-identity-api` and `finance-api` already
//! have. The merged service is therefore no different from its parts in this
//! respect.
//!
//! Per-schema `PostgreSQL` logins — a control role that cannot read finance
//! tables and so on — are a deferred capability, not a lost one; see the
//! narrowed-login row in `references/backlog.md`. Nothing here bears on
//! *tenant* isolation, which is enforced in the application and is unaffected.
//!
//! # What is deliberately absent
//!
//! * A `ROLE` variable per schema. The Lambda deployables each carry one and
//!   compare it to a constant, which proves only that the operator typed the
//!   constant: the role a connection actually assumes comes from the secret,
//!   not from the environment. This prelaunch composition deliberately uses
//!   the one cluster login and therefore does not run a per-role membership
//!   probe at start-up.
//!   Two `VERCEL_*` values used to be excluded here on the grounds that the
//!   browser ceremony exchange was not a route this process mounts. It is one
//!   now: this composition performs the provider authorization-code exchange
//!   itself, so it requires Google's registered OAuth client
//!   ([`GOOGLE_OAUTH_SECRET_ID`]) and the one [`SIGN_IN_REDIRECT_URI`] it is
//!   registered against, and excludes nothing
//!   on that ground.

use std::collections::BTreeMap;
use std::time::Duration;

use aex_central_http::authorizer::MAX_CONTEXT_LIFETIME_MS;
use aex_central_http::config::{CentralServiceId, HttpConfig};
use aex_regional_http::drain::{MAX_DRAIN_DEADLINE_MS, MIN_DRAIN_DEADLINE_MS};
use aex_wire::types::{HttpsUrl, Region};

/// The deployable this crate is.
pub const DEPLOYABLE: CentralServiceId = CentralServiceId::CentralApi;

/// The configuration namespace `release/units.toml` registers for this unit.
pub const NAMESPACE: &str = "AEX_CENTRAL_API_";

/// The plane's account id, for the ARN binding check.
pub const ACCOUNT_ID: &str = "AEX_CENTRAL_API_ACCOUNT_ID";
/// Public workspace API base URLs, `region=https-url` comma-separated.
pub const API_URLS: &str = "AEX_CENTRAL_API_API_URLS";
/// The API-key credential pepper secret.
pub const API_KEY_PEPPER_SECRET_ID: &str = "AEX_CENTRAL_API_API_KEY_PEPPER_SECRET_ID";
/// The Aurora cluster holding every central schema.
pub const AURORA_CLUSTER_ARN: &str = "AEX_CENTRAL_API_AURORA_CLUSTER_ARN";
/// The credentials secret this deployable connects with.
pub const AURORA_SECRET_ARN: &str = "AEX_CENTRAL_API_AURORA_SECRET_ARN";
/// How long one minted admission context is honoured for.
pub const CONTEXT_LIFETIME_MS: &str = "AEX_CENTRAL_API_CONTEXT_LIFETIME_MS";
/// The cursor signing secret.
pub const CURSOR_SECRET_ID: &str = "AEX_CENTRAL_API_CURSOR_SECRET_ID";
/// The logical database inside the cluster.
pub const DATABASE: &str = "AEX_CENTRAL_API_DATABASE";
/// How long a drain may run before the listener is abandoned.
pub const DRAIN_DEADLINE_MS: &str = "AEX_CENTRAL_API_DRAIN_DEADLINE_MS";
/// The identity credential pepper secret.
pub const IDENTITY_PEPPER_SECRET_ID: &str = "AEX_CENTRAL_API_IDENTITY_PEPPER_SECRET_ID";
/// The largest request body this composition accepts.
pub const MAX_BODY_BYTES: &str = "AEX_CENTRAL_API_MAX_BODY_BYTES";
/// The largest page a list route will answer with.
pub const PAGE_LIMIT: &str = "AEX_CENTRAL_API_PAGE_LIMIT";
/// The deployment plane.
pub const PLANE: &str = "AEX_CENTRAL_API_PLANE";
/// The `TCP` port the one listener binds.
pub const PORT: &str = "AEX_CENTRAL_API_PORT";
/// The bound region.
pub const REGION: &str = "AEX_CENTRAL_API_REGION";
/// How long one request may take before the edge gives up.
pub const REQUEST_DEADLINE_MS: &str = "AEX_CENTRAL_API_REQUEST_DEADLINE_MS";
/// The secret holding Google's registered OAuth client.
///
/// A JSON object with `clientId` and `clientSecret`, bound to
/// [`aex_central_http::capability::SignInHandshake`].
pub const GOOGLE_OAUTH_SECRET_ID: &str = "AEX_CENTRAL_API_GOOGLE_OAUTH_SECRET_ID";
/// The secret holding GitHub's registered OAuth client.
pub const GITHUB_OAUTH_SECRET_ID: &str = "AEX_CENTRAL_API_GITHUB_OAUTH_SECRET_ID";
/// Where a provider sends the browser back after a person authorizes.
///
/// Configured rather than accepted from the request body: a caller that could
/// choose it could aim a redeemed code at any URI the provider has registered.
pub const SIGN_IN_REDIRECT_URI: &str = "AEX_CENTRAL_API_SIGN_IN_REDIRECT_URI";
/// The `stripe-command-edge` function this deployable may invoke.
pub const STRIPE_COMMAND_EDGE_ARN: &str = "AEX_CENTRAL_API_STRIPE_COMMAND_EDGE_ARN";

/// Every variable this deployable requires, for the totality test and for the
/// refusal message a crash-looping deployment prints.
pub const ALL: &[&str] = &[
    ACCOUNT_ID,
    API_KEY_PEPPER_SECRET_ID,
    API_URLS,
    AURORA_CLUSTER_ARN,
    AURORA_SECRET_ARN,
    CONTEXT_LIFETIME_MS,
    CURSOR_SECRET_ID,
    DATABASE,
    DRAIN_DEADLINE_MS,
    GOOGLE_OAUTH_SECRET_ID,
    GITHUB_OAUTH_SECRET_ID,
    IDENTITY_PEPPER_SECRET_ID,
    MAX_BODY_BYTES,
    PAGE_LIMIT,
    PLANE,
    PORT,
    REGION,
    REQUEST_DEADLINE_MS,
    SIGN_IN_REDIRECT_URI,
    STRIPE_COMMAND_EDGE_ARN,
];

/// Why `central-api` refused to start.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CentralApiConfigError {
    /// A required variable was absent or blank.
    #[error("required environment variable `{0}` is missing")]
    Missing(&'static str),
    /// A required variable was present but unusable.
    #[error("environment variable `{name}` is invalid: {reason}")]
    Invalid {
        /// Which variable.
        name: &'static str,
        /// Why it was refused.
        reason: String,
    },
}

/// Validated start-up configuration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    /// The shared `HTTP` composition configuration.
    pub http: HttpConfig,
    /// The plane's account id.
    pub account_id: String,
    /// The one Aurora cluster this deployable connects to.
    pub aurora_cluster_arn: String,
    /// The logical database inside it.
    pub database: String,
    /// The credentials secret this deployable connects with.
    pub aurora_secret_arn: String,
    /// The API-key pepper secret.
    pub api_key_pepper_secret_id: String,
    /// The identity pepper secret.
    pub identity_pepper_secret_id: String,
    /// The secret holding Google's registered OAuth client.
    pub google_oauth_secret_id: String,
    /// The secret holding GitHub's registered OAuth client.
    pub github_oauth_secret_id: String,
    /// Where a provider sends the browser back after a person authorizes.
    pub sign_in_redirect_uri: String,
    /// The cursor signing secret.
    pub cursor_secret_id: String,
    /// Public workspace API base URL for every configured region.
    pub api_urls: BTreeMap<Region, HttpsUrl>,
    /// The Stripe command edge function ARN.
    pub stripe_command_edge_arn: String,
    /// The largest page a list route answers with.
    pub page_limit: u32,
    /// How long one minted admission context is honoured for.
    pub context_lifetime_ms: u64,
    /// The `TCP` port the one listener binds.
    pub port: u16,
    /// How long a drain may run before the listener is abandoned.
    pub drain_deadline_ms: u64,
}

impl Config {
    /// Reads and validates the configuration from the process environment.
    ///
    /// # Errors
    ///
    /// Returns [`CentralApiConfigError`] naming the first variable it refused.
    pub fn from_env() -> Result<Self, CentralApiConfigError> {
        Self::from_lookup(|name| std::env::var(name).ok())
    }

    /// Reads and validates from an arbitrary lookup.
    ///
    /// Tests use this directly: `std::env::set_var` is `unsafe` in edition 2024
    /// and this workspace forbids `unsafe` code.
    ///
    /// # Errors
    ///
    /// Identical to [`Config::from_env`].
    #[allow(
        clippy::too_many_lines,
        reason = "one reader for one process; splitting it would hide which variables are read together"
    )]
    pub fn from_lookup<F>(lookup: F) -> Result<Self, CentralApiConfigError>
    where
        F: Fn(&str) -> Option<String>,
    {
        let http = HttpConfig::resolve(
            &required(&lookup, PLANE)?,
            &required(&lookup, REGION)?,
            DEPLOYABLE.as_str(),
            integer(&lookup, MAX_BODY_BYTES)?,
            integer(&lookup, REQUEST_DEADLINE_MS)?,
        )
        .map_err(|reason| CentralApiConfigError::Invalid {
            name: PLANE,
            reason: reason.to_string(),
        })?;

        let api_urls = api_urls(&required(&lookup, API_URLS)?)?;
        let page_limit = bounded(&lookup, PAGE_LIMIT, 1, 1_000)?;
        let context_lifetime_ms = bounded(
            &lookup,
            CONTEXT_LIFETIME_MS,
            1,
            MAX_CONTEXT_LIFETIME_MS.unsigned_abs(),
        )?;
        let raw_port = bounded(&lookup, PORT, 1, 65_535)?;
        let drain_deadline_ms = bounded(
            &lookup,
            DRAIN_DEADLINE_MS,
            MIN_DRAIN_DEADLINE_MS,
            MAX_DRAIN_DEADLINE_MS,
        )?;

        Ok(Self {
            http,
            account_id: required(&lookup, ACCOUNT_ID)?,
            aurora_cluster_arn: required(&lookup, AURORA_CLUSTER_ARN)?,
            database: required(&lookup, DATABASE)?,
            aurora_secret_arn: required(&lookup, AURORA_SECRET_ARN)?,
            api_key_pepper_secret_id: required(&lookup, API_KEY_PEPPER_SECRET_ID)?,
            identity_pepper_secret_id: required(&lookup, IDENTITY_PEPPER_SECRET_ID)?,
            google_oauth_secret_id: required(&lookup, GOOGLE_OAUTH_SECRET_ID)?,
            github_oauth_secret_id: required(&lookup, GITHUB_OAUTH_SECRET_ID)?,
            sign_in_redirect_uri: https_url(&lookup, SIGN_IN_REDIRECT_URI)?,
            cursor_secret_id: required(&lookup, CURSOR_SECRET_ID)?,
            api_urls,
            stripe_command_edge_arn: required(&lookup, STRIPE_COMMAND_EDGE_ARN)?,
            page_limit: u32::try_from(page_limit).unwrap_or(u32::MAX),
            context_lifetime_ms,
            port: u16::try_from(raw_port).map_err(|_| CentralApiConfigError::Invalid {
                name: PORT,
                reason: format!("`{raw_port}` is not a TCP port"),
            })?,
            drain_deadline_ms,
        })
    }

    /// The resolved values the composition check runs over.
    ///
    /// Only the keys the manifest binds. `admit` refuses a value it was not
    /// told about, so this map and
    /// [`crate::manifest`] are two halves of one statement.
    #[must_use]
    pub fn resolved(&self) -> aex_central_http::capability::ResolvedConfig {
        aex_central_http::capability::ResolvedConfig {
            deployable: DEPLOYABLE.as_str().to_owned(),
            plane: self.http.plane,
            region: self.http.region,
            account_id: self.account_id.clone(),
            values: BTreeMap::from([
                (
                    AURORA_CLUSTER_ARN.to_owned(),
                    self.aurora_cluster_arn.clone(),
                ),
                (
                    API_KEY_PEPPER_SECRET_ID.to_owned(),
                    self.api_key_pepper_secret_id.clone(),
                ),
                (
                    GOOGLE_OAUTH_SECRET_ID.to_owned(),
                    self.google_oauth_secret_id.clone(),
                ),
                (
                    GITHUB_OAUTH_SECRET_ID.to_owned(),
                    self.github_oauth_secret_id.clone(),
                ),
                (
                    IDENTITY_PEPPER_SECRET_ID.to_owned(),
                    self.identity_pepper_secret_id.clone(),
                ),
                (AURORA_SECRET_ARN.to_owned(), self.aurora_secret_arn.clone()),
                (
                    STRIPE_COMMAND_EDGE_ARN.to_owned(),
                    self.stripe_command_edge_arn.clone(),
                ),
            ]),
        }
    }

    /// The drain deadline as a duration.
    #[must_use]
    pub const fn drain_deadline(&self) -> Duration {
        Duration::from_millis(self.drain_deadline_ms)
    }
}

fn required<F>(lookup: &F, name: &'static str) -> Result<String, CentralApiConfigError>
where
    F: Fn(&str) -> Option<String>,
{
    match lookup(name) {
        Some(value) if !value.trim().is_empty() => Ok(value),
        _ => Err(CentralApiConfigError::Missing(name)),
    }
}

fn integer<F>(lookup: &F, name: &'static str) -> Result<u64, CentralApiConfigError>
where
    F: Fn(&str) -> Option<String>,
{
    let raw = required(lookup, name)?;
    raw.parse::<u64>()
        .map_err(|_| CentralApiConfigError::Invalid {
            name,
            reason: format!("expected an integer, got `{raw}`"),
        })
}

/// Reads a value that must be an `https` URL.
///
/// Validated at start-up rather than at the first sign-in: a redirect URI that
/// does not parse is a configuration fact, and discovering it when a person
/// clicks a provider button costs an outage nobody can attribute.
fn https_url<F>(lookup: &F, name: &'static str) -> Result<String, CentralApiConfigError>
where
    F: Fn(&str) -> Option<String>,
{
    let raw = required(lookup, name)?;
    HttpsUrl::parse(&raw)
        .map(|url| url.as_str().to_owned())
        .map_err(|error| CentralApiConfigError::Invalid {
            name,
            reason: error.to_string(),
        })
}

fn bounded<F>(
    lookup: &F,
    name: &'static str,
    min: u64,
    max: u64,
) -> Result<u64, CentralApiConfigError>
where
    F: Fn(&str) -> Option<String>,
{
    let value = integer(lookup, name)?;
    if value < min || value > max {
        return Err(CentralApiConfigError::Invalid {
            name,
            reason: format!("{value} is outside {min}..={max}"),
        });
    }
    Ok(value)
}

fn api_urls(raw: &str) -> Result<BTreeMap<Region, HttpsUrl>, CentralApiConfigError> {
    let mut map = BTreeMap::new();
    for entry in raw.split(',').filter(|entry| !entry.trim().is_empty()) {
        let (region, url) =
            entry
                .split_once('=')
                .ok_or_else(|| CentralApiConfigError::Invalid {
                    name: API_URLS,
                    reason: "expected `region=https-url` entries".to_owned(),
                })?;
        let region =
            Region::from_name(region.trim()).ok_or_else(|| CentralApiConfigError::Invalid {
                name: API_URLS,
                reason: format!("`{region}` is not a launch region"),
            })?;
        let url = HttpsUrl::parse(url.trim()).map_err(|error| CentralApiConfigError::Invalid {
            name: API_URLS,
            reason: error.to_string(),
        })?;
        if map.insert(region, url).is_some() {
            return Err(CentralApiConfigError::Invalid {
                name: API_URLS,
                reason: format!("`{}` is repeated", region.as_str()),
            });
        }
    }
    if map.is_empty() {
        return Err(CentralApiConfigError::Invalid {
            name: API_URLS,
            reason: "at least one configured region is required".to_owned(),
        });
    }
    Ok(map)
}
