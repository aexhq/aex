//! Total, fail-fast start-up configuration for `finance-api`.
//!
//! Nothing here has a default. A resource identifier, a database role, a plane
//! or a region that a process can guess is a process that silently binds itself
//! to the wrong plane, and for a money authority that is the worst available
//! failure mode. Every variable is read once, validated once, and named in the
//! refusal when it is absent or unusable.

use std::time::Duration;

use aex_rds_data::config::{DataApiConfig, DatabaseName, ResourceArn, SecretArn};

/// The configuration namespace `release/units.toml` registers for this unit.
pub const NAMESPACE: &str = "AEX_FINANCE_API_";

/// The deployment plane.
pub const PLANE_VAR: &str = "AEX_FINANCE_API_PLANE";
/// The bound AWS region.
pub const REGION_VAR: &str = "AEX_FINANCE_API_REGION";
/// The Aurora cluster holding the `finance` schema.
pub const CLUSTER_ARN_VAR: &str = "AEX_FINANCE_API_AURORA_CLUSTER_ARN";
/// The Secrets Manager secret naming this deployable's own login role.
pub const SECRET_ARN_VAR: &str = "AEX_FINANCE_API_AURORA_SECRET_ARN";
/// The logical database inside the cluster.
pub const DATABASE_NAME_VAR: &str = "AEX_FINANCE_API_DATABASE_NAME";
/// The `PostgreSQL` role this deployable connects as.
pub const DATABASE_ROLE_VAR: &str = "AEX_FINANCE_API_DATABASE_ROLE";
/// The `stripe-command-edge` function this deployable may invoke.
pub const COMMAND_EDGE_ARN_VAR: &str = "AEX_FINANCE_API_STRIPE_COMMAND_EDGE_ARN";
/// The pricing context a new reservation pins.
pub const PRICING_VERSION_VAR: &str = "AEX_FINANCE_API_DEFAULT_PRICING_VERSION";
/// The bucket issued statement artifacts live in.
pub const STATEMENT_BUCKET_VAR: &str = "AEX_FINANCE_API_STATEMENT_BUCKET";
/// The application deadline for one Aurora transaction.
pub const TX_DEADLINE_VAR: &str = "AEX_FINANCE_API_TX_DEADLINE_MS";
/// The largest page a list route will answer with.
pub const PAGE_LIMIT_VAR: &str = "AEX_FINANCE_API_PAGE_LIMIT";
/// How long a minted statement download grant verifies for.
pub const DOWNLOAD_GRANT_TTL_VAR: &str = "AEX_FINANCE_API_DOWNLOAD_GRANT_TTL_MS";

/// Every variable this deployable requires, in declaration order.
pub const REQUIRED_VARS: [&str; 12] = [
    PLANE_VAR,
    REGION_VAR,
    CLUSTER_ARN_VAR,
    SECRET_ARN_VAR,
    DATABASE_NAME_VAR,
    DATABASE_ROLE_VAR,
    COMMAND_EDGE_ARN_VAR,
    PRICING_VERSION_VAR,
    STATEMENT_BUCKET_VAR,
    TX_DEADLINE_VAR,
    PAGE_LIMIT_VAR,
    DOWNLOAD_GRANT_TTL_VAR,
];

/// The only role `finance-api` may connect as.
pub const REQUIRED_ROLE: &str = "aex_finance_api";

/// The planes this deployable may be bound to.
pub const PLANES: [&str; 2] = ["dev", "prd"];

/// OD-17 pins a download grant and its signature to the same five minutes.
pub const MAX_DOWNLOAD_GRANT_TTL_MS: u64 = 300_000;

/// Validated start-up configuration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    /// Deployment plane.
    pub plane: String,
    /// Bound AWS region.
    pub region: String,
    /// The validated Data `API` addressing for the finance schema.
    pub data_api: DataApiConfig,
    /// The role the login secret authenticates as.
    pub database_role: String,
    /// The Stripe command edge function ARN.
    pub command_edge_arn: String,
    /// The pricing context bound at admission.
    pub default_pricing_version: String,
    /// The statement artifact bucket.
    pub statement_bucket: String,
    /// The largest page a list route answers with.
    pub page_limit: u32,
    /// How long a statement download grant verifies for.
    pub download_grant_ttl: Duration,
}

/// Why `finance-api` refused to start.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ConfigError {
    /// A required variable was absent or blank.
    #[error("required environment variable `{name}` is missing")]
    Missing {
        /// The variable that must be supplied.
        name: &'static str,
    },
    /// A required variable was present but unusable.
    #[error("environment variable `{name}` is invalid: {reason}")]
    Invalid {
        /// The variable that was rejected.
        name: &'static str,
        /// Why it was rejected.
        reason: String,
    },
}

impl Config {
    /// Reads and validates the configuration from the process environment.
    ///
    /// # Errors
    ///
    /// Returns [`ConfigError::Missing`] naming the first absent or blank
    /// variable, and [`ConfigError::Invalid`] naming the first variable whose
    /// value does not parse or is outside its permitted set.
    pub fn from_env() -> Result<Self, ConfigError> {
        Self::from_lookup(|name| std::env::var(name).ok())
    }

    /// Reads and validates the configuration from an arbitrary lookup.
    ///
    /// Tests use this directly: `std::env::set_var` is `unsafe` in edition 2024
    /// and this workspace forbids `unsafe` code.
    ///
    /// # Errors
    ///
    /// Identical to [`Config::from_env`].
    pub fn from_lookup<F>(lookup: F) -> Result<Self, ConfigError>
    where
        F: Fn(&str) -> Option<String>,
    {
        let plane = one_of(&lookup, PLANE_VAR, &PLANES)?;
        let region = required(&lookup, REGION_VAR)?;
        let cluster_arn = ResourceArn::parse(&required(&lookup, CLUSTER_ARN_VAR)?)
            .map_err(|error| invalid(CLUSTER_ARN_VAR, &error))?;
        let secret_arn = SecretArn::parse(&required(&lookup, SECRET_ARN_VAR)?)
            .map_err(|error| invalid(SECRET_ARN_VAR, &error))?;
        let database = DatabaseName::parse(&required(&lookup, DATABASE_NAME_VAR)?)
            .map_err(|error| invalid(DATABASE_NAME_VAR, &error))?;
        let database_role = required(&lookup, DATABASE_ROLE_VAR)?;
        if database_role != REQUIRED_ROLE {
            return Err(ConfigError::Invalid {
                name: DATABASE_ROLE_VAR,
                reason: format!("expected `{REQUIRED_ROLE}`, got `{database_role}`"),
            });
        }
        let command_edge_arn = required(&lookup, COMMAND_EDGE_ARN_VAR)?;
        if !command_edge_arn.starts_with("arn:") {
            return Err(ConfigError::Invalid {
                name: COMMAND_EDGE_ARN_VAR,
                reason: format!("expected a function ARN, got `{command_edge_arn}`"),
            });
        }
        let default_pricing_version = required(&lookup, PRICING_VERSION_VAR)?;
        let statement_bucket = required(&lookup, STATEMENT_BUCKET_VAR)?;
        let transaction_deadline_ms = positive(&lookup, TX_DEADLINE_VAR)?;
        let page_limit = positive(&lookup, PAGE_LIMIT_VAR)?;
        let page_limit = u32::try_from(page_limit).map_err(|_| ConfigError::Invalid {
            name: PAGE_LIMIT_VAR,
            reason: format!("expected 1..=1000, got `{page_limit}`"),
        })?;
        if page_limit > 1000 {
            return Err(ConfigError::Invalid {
                name: PAGE_LIMIT_VAR,
                reason: format!("the route table bounds `limit` at 1000, got `{page_limit}`"),
            });
        }
        let download_grant_ttl_ms = positive(&lookup, DOWNLOAD_GRANT_TTL_VAR)?;
        if download_grant_ttl_ms > MAX_DOWNLOAD_GRANT_TTL_MS {
            return Err(ConfigError::Invalid {
                name: DOWNLOAD_GRANT_TTL_VAR,
                reason: format!(
                    "OD-17 bounds a grant and its signature at {MAX_DOWNLOAD_GRANT_TTL_MS} ms, \
                     got `{download_grant_ttl_ms}`"
                ),
            });
        }

        let mut data_api = DataApiConfig::new(cluster_arn, secret_arn, database);
        data_api.transaction_deadline = Duration::from_millis(transaction_deadline_ms);
        data_api.statement_deadline = Duration::from_millis(transaction_deadline_ms);
        data_api
            .validate()
            .map_err(|error| invalid(TX_DEADLINE_VAR, &error))?;

        Ok(Self {
            plane,
            region,
            data_api,
            database_role,
            command_edge_arn,
            default_pricing_version,
            statement_bucket,
            page_limit,
            download_grant_ttl: Duration::from_millis(download_grant_ttl_ms),
        })
    }
}

/// Reads a variable, treating blank as absent.
fn required<F>(lookup: &F, name: &'static str) -> Result<String, ConfigError>
where
    F: Fn(&str) -> Option<String>,
{
    match lookup(name) {
        Some(value) if !value.trim().is_empty() => Ok(value.trim().to_owned()),
        _ => Err(ConfigError::Missing { name }),
    }
}

/// Reads a variable that must be one of a closed set.
fn one_of<F>(lookup: &F, name: &'static str, permitted: &[&str]) -> Result<String, ConfigError>
where
    F: Fn(&str) -> Option<String>,
{
    let value = required(lookup, name)?;
    if permitted.contains(&value.as_str()) {
        Ok(value)
    } else {
        Err(ConfigError::Invalid {
            name,
            reason: format!("expected one of {permitted:?}, got `{value}`"),
        })
    }
}

/// Reads a variable that must be a positive integer.
fn positive<F>(lookup: &F, name: &'static str) -> Result<u64, ConfigError>
where
    F: Fn(&str) -> Option<String>,
{
    let raw = required(lookup, name)?;
    let parsed = raw.parse::<u64>().map_err(|error| ConfigError::Invalid {
        name,
        reason: format!("expected a positive integer, got `{raw}`: {error}"),
    })?;
    if parsed == 0 {
        return Err(ConfigError::Invalid {
            name,
            reason: "expected a positive integer, got `0`".to_owned(),
        });
    }
    Ok(parsed)
}

/// Renders a peer validation failure as this deployable's own refusal.
fn invalid(name: &'static str, error: &impl std::fmt::Display) -> ConfigError {
    ConfigError::Invalid {
        name,
        reason: error.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::{
        CLUSTER_ARN_VAR, Config, ConfigError, DATABASE_ROLE_VAR, DOWNLOAD_GRANT_TTL_VAR, NAMESPACE,
        PAGE_LIMIT_VAR, PLANE_VAR, REQUIRED_VARS,
    };

    pub(crate) fn complete() -> BTreeMap<&'static str, String> {
        BTreeMap::from([
            (super::PLANE_VAR, "dev".to_owned()),
            (super::REGION_VAR, "eu-west-1".to_owned()),
            (
                super::CLUSTER_ARN_VAR,
                "arn:aws:rds:eu-west-1:000000000000:cluster:aex-central".to_owned(),
            ),
            (
                super::SECRET_ARN_VAR,
                "arn:aws:secretsmanager:eu-west-1:000000000000:secret:aex/finance-api".to_owned(),
            ),
            (super::DATABASE_NAME_VAR, "aex".to_owned()),
            (super::DATABASE_ROLE_VAR, "aex_finance_api".to_owned()),
            (
                super::COMMAND_EDGE_ARN_VAR,
                "arn:aws:lambda:eu-west-1:000000000000:function:aex-dev-stripe-command-edge"
                    .to_owned(),
            ),
            (super::PRICING_VERSION_VAR, "synthetic-zero-v1".to_owned()),
            (super::STATEMENT_BUCKET_VAR, "aex-dev-statements".to_owned()),
            (super::TX_DEADLINE_VAR, "8000".to_owned()),
            (super::PAGE_LIMIT_VAR, "100".to_owned()),
            (super::DOWNLOAD_GRANT_TTL_VAR, "300000".to_owned()),
        ])
    }

    fn read(vars: &BTreeMap<&'static str, String>) -> Result<Config, ConfigError> {
        Config::from_lookup(|name| vars.get(name).cloned())
    }

    #[test]
    fn every_required_variable_is_namespaced_and_declared() {
        let vars = complete();
        assert_eq!(REQUIRED_VARS.len(), vars.len());
        for name in REQUIRED_VARS {
            assert!(name.starts_with(NAMESPACE), "{name} is outside {NAMESPACE}");
            assert!(vars.contains_key(name), "{name} has no fixture value");
        }
    }

    #[test]
    fn a_complete_environment_is_accepted() {
        let config = read(&complete()).expect("the complete environment is accepted");
        assert_eq!(config.plane, "dev");
        assert_eq!(config.page_limit, 100);
        assert_eq!(config.data_api.database.as_str(), "aex");
    }

    #[test]
    fn each_missing_variable_is_named_in_the_refusal() {
        for name in REQUIRED_VARS {
            let mut vars = complete();
            vars.remove(name);
            assert_eq!(
                read(&vars),
                Err(ConfigError::Missing { name }),
                "removing {name}"
            );
        }
    }

    #[test]
    fn a_blank_variable_is_missing_rather_than_empty() {
        let mut vars = complete();
        vars.insert(CLUSTER_ARN_VAR, "   ".to_owned());
        assert_eq!(
            read(&vars),
            Err(ConfigError::Missing {
                name: CLUSTER_ARN_VAR
            })
        );
    }

    #[test]
    fn a_resource_identifier_that_is_not_an_arn_is_refused() {
        let mut vars = complete();
        vars.insert(CLUSTER_ARN_VAR, "aex-central".to_owned());
        let error = read(&vars).expect_err("a bare cluster name is not an ARN");
        assert!(
            matches!(error, ConfigError::Invalid { name, .. } if name == CLUSTER_ARN_VAR),
            "{error:?}"
        );
    }

    #[test]
    fn the_wrong_database_role_is_refused_before_a_connection_exists() {
        let mut vars = complete();
        vars.insert(DATABASE_ROLE_VAR, "aex_finance_settlement".to_owned());
        let error = read(&vars).expect_err("finance-api may only be aex_finance_api");
        assert!(
            matches!(error, ConfigError::Invalid { name, .. } if name == DATABASE_ROLE_VAR),
            "{error:?}"
        );
    }

    #[test]
    fn an_unknown_plane_is_refused() {
        let mut vars = complete();
        vars.insert(PLANE_VAR, "staging".to_owned());
        let error = read(&vars).expect_err("an unknown plane is refused");
        assert!(
            matches!(error, ConfigError::Invalid { name, .. } if name == PLANE_VAR),
            "{error:?}"
        );
    }

    #[test]
    fn a_page_limit_above_the_route_table_bound_is_refused() {
        let mut vars = complete();
        vars.insert(PAGE_LIMIT_VAR, "1001".to_owned());
        let error = read(&vars).expect_err("the route table bounds `limit` at 1000");
        assert!(
            matches!(error, ConfigError::Invalid { name, .. } if name == PAGE_LIMIT_VAR),
            "{error:?}"
        );
    }

    #[test]
    fn a_grant_that_outlives_its_signature_is_refused() {
        let mut vars = complete();
        vars.insert(DOWNLOAD_GRANT_TTL_VAR, "300001".to_owned());
        let error = read(&vars).expect_err("OD-17 bounds the grant at five minutes");
        assert!(
            matches!(error, ConfigError::Invalid { name, .. } if name == DOWNLOAD_GRANT_TTL_VAR),
            "{error:?}"
        );
    }

    #[test]
    fn a_zero_deadline_is_refused() {
        let mut vars = complete();
        vars.insert(super::TX_DEADLINE_VAR, "0".to_owned());
        let error = read(&vars).expect_err("a zero transaction deadline is refused");
        assert!(
            matches!(error, ConfigError::Invalid { name, .. } if name == super::TX_DEADLINE_VAR),
            "{error:?}"
        );
    }
}
