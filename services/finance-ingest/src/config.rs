//! Total, fail-fast start-up configuration for `finance-ingest`.
//!
//! This deployable holds no Stripe secret, no queue and no bucket. It has one
//! dependency — the finance schema — and one policy value it may not guess: the
//! pinned provider API version, which decides whether an event is parsed at all.

use std::time::Duration;

use aex_rds_data::config::{DataApiConfig, DatabaseName, ResourceArn, SecretArn};

/// The configuration namespace `release/units.toml` registers for this unit.
pub const NAMESPACE: &str = "AEX_FINANCE_INGEST_";

/// The deployment plane.
pub const PLANE_VAR: &str = "AEX_FINANCE_INGEST_PLANE";
/// The bound AWS region.
pub const REGION_VAR: &str = "AEX_FINANCE_INGEST_REGION";
/// The Aurora cluster holding the `finance` schema.
pub const CLUSTER_ARN_VAR: &str = "AEX_FINANCE_INGEST_AURORA_CLUSTER_ARN";
/// The Secrets Manager secret naming this deployable's own login role.
pub const SECRET_ARN_VAR: &str = "AEX_FINANCE_INGEST_AURORA_SECRET_ARN";
/// The logical database inside the cluster.
pub const DATABASE_NAME_VAR: &str = "AEX_FINANCE_INGEST_DATABASE_NAME";
/// The `PostgreSQL` role this deployable connects as.
pub const DATABASE_ROLE_VAR: &str = "AEX_FINANCE_INGEST_DATABASE_ROLE";
/// The provider API version every accepted event must declare.
pub const PINNED_API_VERSION_VAR: &str = "AEX_FINANCE_INGEST_PINNED_STRIPE_API_VERSION";
/// The application deadline for one Aurora transaction.
pub const TX_DEADLINE_VAR: &str = "AEX_FINANCE_INGEST_TX_DEADLINE_MS";

/// Every variable this deployable requires, in declaration order.
pub const REQUIRED_VARS: [&str; 8] = [
    PLANE_VAR,
    REGION_VAR,
    CLUSTER_ARN_VAR,
    SECRET_ARN_VAR,
    DATABASE_NAME_VAR,
    DATABASE_ROLE_VAR,
    PINNED_API_VERSION_VAR,
    TX_DEADLINE_VAR,
];

/// The only role `finance-ingest` may connect as.
pub const REQUIRED_ROLE: &str = "aex_finance_ingest";

/// The planes this deployable may be bound to.
pub const PLANES: [&str; 2] = ["dev", "prd"];

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
    /// The provider API version every accepted event must declare.
    pub pinned_api_version: String,
}

/// Why `finance-ingest` refused to start.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum FinanceIngestConfigError {
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
    /// Returns [`FinanceIngestConfigError::Missing`] naming the first absent or blank
    /// variable and [`FinanceIngestConfigError::Invalid`] naming the first unusable one.
    pub fn from_env() -> Result<Self, FinanceIngestConfigError> {
        Self::from_lookup(|name| std::env::var(name).ok())
    }

    /// Reads and validates the configuration from an arbitrary lookup.
    ///
    /// # Errors
    ///
    /// Identical to [`Config::from_env`].
    pub fn from_lookup<F>(lookup: F) -> Result<Self, FinanceIngestConfigError>
    where
        F: Fn(&str) -> Option<String>,
    {
        let plane = required(&lookup, PLANE_VAR)?;
        if !PLANES.contains(&plane.as_str()) {
            return Err(FinanceIngestConfigError::Invalid {
                name: PLANE_VAR,
                reason: format!("expected one of {PLANES:?}, got `{plane}`"),
            });
        }
        let region = required(&lookup, REGION_VAR)?;
        let cluster_arn = ResourceArn::parse(&required(&lookup, CLUSTER_ARN_VAR)?)
            .map_err(|error| invalid(CLUSTER_ARN_VAR, &error))?;
        let secret_arn = SecretArn::parse(&required(&lookup, SECRET_ARN_VAR)?)
            .map_err(|error| invalid(SECRET_ARN_VAR, &error))?;
        let database = DatabaseName::parse(&required(&lookup, DATABASE_NAME_VAR)?)
            .map_err(|error| invalid(DATABASE_NAME_VAR, &error))?;
        let database_role = required(&lookup, DATABASE_ROLE_VAR)?;
        if database_role != REQUIRED_ROLE {
            return Err(FinanceIngestConfigError::Invalid {
                name: DATABASE_ROLE_VAR,
                reason: format!("expected `{REQUIRED_ROLE}`, got `{database_role}`"),
            });
        }
        let pinned_api_version = required(&lookup, PINNED_API_VERSION_VAR)?;
        let deadline_ms = required(&lookup, TX_DEADLINE_VAR)?;
        let deadline_ms = deadline_ms
            .parse::<u64>()
            .ok()
            .filter(|value| *value > 0)
            .ok_or_else(|| FinanceIngestConfigError::Invalid {
                name: TX_DEADLINE_VAR,
                reason: format!("expected a positive integer, got `{deadline_ms}`"),
            })?;
        let mut data_api = DataApiConfig::new(cluster_arn, secret_arn, database);
        data_api.transaction_deadline = Duration::from_millis(deadline_ms);
        data_api.statement_deadline = Duration::from_millis(deadline_ms);
        data_api
            .validate()
            .map_err(|error| invalid(TX_DEADLINE_VAR, &error))?;
        Ok(Self {
            plane,
            region,
            data_api,
            database_role,
            pinned_api_version,
        })
    }
}

/// Reads a variable, treating blank as absent.
fn required<F>(lookup: &F, name: &'static str) -> Result<String, FinanceIngestConfigError>
where
    F: Fn(&str) -> Option<String>,
{
    match lookup(name) {
        Some(value) if !value.trim().is_empty() => Ok(value.trim().to_owned()),
        _ => Err(FinanceIngestConfigError::Missing { name }),
    }
}

/// Renders a peer validation failure as this deployable's own refusal.
fn invalid(name: &'static str, error: &impl std::fmt::Display) -> FinanceIngestConfigError {
    FinanceIngestConfigError::Invalid {
        name,
        reason: error.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::{
        CLUSTER_ARN_VAR, Config, FinanceIngestConfigError, DATABASE_ROLE_VAR, NAMESPACE, PLANE_VAR,
        REQUIRED_VARS,
    };

    fn complete() -> BTreeMap<&'static str, String> {
        BTreeMap::from([
            (super::PLANE_VAR, "prd".to_owned()),
            (super::REGION_VAR, "eu-west-1".to_owned()),
            (
                super::CLUSTER_ARN_VAR,
                "arn:aws:rds:eu-west-1:000000000000:cluster:aex-central".to_owned(),
            ),
            (
                super::SECRET_ARN_VAR,
                "arn:aws:secretsmanager:eu-west-1:000000000000:secret:aex/finance-ingest"
                    .to_owned(),
            ),
            (super::DATABASE_NAME_VAR, "aex".to_owned()),
            (super::DATABASE_ROLE_VAR, "aex_finance_ingest".to_owned()),
            (
                super::PINNED_API_VERSION_VAR,
                "2026-06-24.dahlia".to_owned(),
            ),
            (super::TX_DEADLINE_VAR, "6000".to_owned()),
        ])
    }

    fn read(vars: &BTreeMap<&'static str, String>) -> Result<Config, FinanceIngestConfigError> {
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
        assert_eq!(config.pinned_api_version, "2026-06-24.dahlia");
        assert_eq!(config.plane, "prd");
    }

    #[test]
    fn each_missing_variable_is_named_in_the_refusal() {
        for name in REQUIRED_VARS {
            let mut vars = complete();
            vars.remove(name);
            assert_eq!(
                read(&vars),
                Err(FinanceIngestConfigError::Missing { name }),
                "removing {name}"
            );
        }
    }

    #[test]
    fn the_wrong_database_role_is_refused_before_a_connection_exists() {
        let mut vars = complete();
        vars.insert(DATABASE_ROLE_VAR, "aex_finance_api".to_owned());
        let error = read(&vars).expect_err("finance-ingest may only be aex_finance_ingest");
        assert!(
            matches!(error, FinanceIngestConfigError::Invalid { name, .. } if name == DATABASE_ROLE_VAR),
            "{error:?}"
        );
    }

    #[test]
    fn a_cluster_that_is_not_an_arn_is_refused() {
        let mut vars = complete();
        vars.insert(CLUSTER_ARN_VAR, "aex-central".to_owned());
        let error = read(&vars).expect_err("a bare cluster name is not an ARN");
        assert!(
            matches!(error, FinanceIngestConfigError::Invalid { name, .. } if name == CLUSTER_ARN_VAR),
            "{error:?}"
        );
    }

    #[test]
    fn an_unknown_plane_is_refused() {
        let mut vars = complete();
        vars.insert(PLANE_VAR, "local".to_owned());
        let error = read(&vars).expect_err("an unknown plane is refused");
        assert!(
            matches!(error, FinanceIngestConfigError::Invalid { name, .. } if name == PLANE_VAR),
            "{error:?}"
        );
    }
}
