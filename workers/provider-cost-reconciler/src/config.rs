//! Total, fail-fast start-up configuration for `provider-cost-reconciler`.

use std::time::Duration;

use aex_rds_data::config::{DataApiConfig, DatabaseName, ResourceArn, SecretArn};

/// The configuration namespace `release/units.toml` registers for this unit.
pub const NAMESPACE: &str = "AEX_PROVIDER_COST_";

/// The deployment plane.
pub const PLANE_VAR: &str = "AEX_PROVIDER_COST_PLANE";
/// The bound AWS region.
pub const REGION_VAR: &str = "AEX_PROVIDER_COST_REGION";
/// The Aurora cluster holding the `finance` schema.
pub const CLUSTER_ARN_VAR: &str = "AEX_PROVIDER_COST_AURORA_CLUSTER_ARN";
/// The Secrets Manager secret naming this deployable's own login role.
pub const SECRET_ARN_VAR: &str = "AEX_PROVIDER_COST_AURORA_SECRET_ARN";
/// The logical database inside the cluster.
pub const DATABASE_NAME_VAR: &str = "AEX_PROVIDER_COST_DATABASE_NAME";
/// The `PostgreSQL` role this deployable connects as.
pub const DATABASE_ROLE_VAR: &str = "AEX_PROVIDER_COST_DATABASE_ROLE";
/// The bucket the provider cost export is delivered to.
pub const CUR_BUCKET_VAR: &str = "AEX_PROVIDER_COST_CUR_BUCKET";
/// The prefix inside that bucket.
pub const CUR_PREFIX_VAR: &str = "AEX_PROVIDER_COST_CUR_PREFIX";
/// The margin, in basis points, below which a finding is reported.
pub const MARGIN_THRESHOLD_VAR: &str = "AEX_PROVIDER_COST_MARGIN_ALERT_THRESHOLD_BPS";
/// The operations topic every finding is reported to.
pub const ALARM_TOPIC_VAR: &str = "AEX_PROVIDER_COST_ALARM_TOPIC_ARN";
/// The hard bound on bytes scanned in one run.
pub const MAX_SCAN_BYTES_VAR: &str = "AEX_PROVIDER_COST_MAX_SCAN_BYTES";
/// The application deadline for one Aurora transaction.
pub const TX_DEADLINE_VAR: &str = "AEX_PROVIDER_COST_TX_DEADLINE_MS";

/// Every variable this deployable requires, in declaration order.
pub const REQUIRED_VARS: [&str; 12] = [
    PLANE_VAR,
    REGION_VAR,
    CLUSTER_ARN_VAR,
    SECRET_ARN_VAR,
    DATABASE_NAME_VAR,
    DATABASE_ROLE_VAR,
    CUR_BUCKET_VAR,
    CUR_PREFIX_VAR,
    MARGIN_THRESHOLD_VAR,
    ALARM_TOPIC_VAR,
    MAX_SCAN_BYTES_VAR,
    TX_DEADLINE_VAR,
];

/// The only role `provider-cost-reconciler` may connect as.
pub const REQUIRED_ROLE: &str = "aex_provider_cost";

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
    /// The bucket the provider cost export is delivered to.
    pub cur_bucket: String,
    /// The prefix inside that bucket.
    pub cur_prefix: String,
    /// The margin threshold, in basis points.
    pub margin_alert_threshold_bps: u32,
    /// The operations topic.
    pub alarm_topic_arn: String,
    /// The hard bound on bytes scanned in one run.
    pub max_scan_bytes: u64,
}

/// Why `provider-cost-reconciler` refused to start.
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
    /// variable and [`ConfigError::Invalid`] naming the first unusable one.
    pub fn from_env() -> Result<Self, ConfigError> {
        Self::from_lookup(|name| std::env::var(name).ok())
    }

    /// Reads and validates the configuration from an arbitrary lookup.
    ///
    /// # Errors
    ///
    /// Identical to [`Config::from_env`].
    pub fn from_lookup<F>(lookup: F) -> Result<Self, ConfigError>
    where
        F: Fn(&str) -> Option<String>,
    {
        let plane = required(&lookup, PLANE_VAR)?;
        if !PLANES.contains(&plane.as_str()) {
            return Err(ConfigError::Invalid {
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
            return Err(ConfigError::Invalid {
                name: DATABASE_ROLE_VAR,
                reason: format!("expected `{REQUIRED_ROLE}`, got `{database_role}`"),
            });
        }
        let cur_bucket = required(&lookup, CUR_BUCKET_VAR)?;
        let cur_prefix = required(&lookup, CUR_PREFIX_VAR)?;
        if cur_prefix.starts_with('/') || cur_prefix.contains("..") {
            return Err(ConfigError::Invalid {
                name: CUR_PREFIX_VAR,
                reason: format!("expected a normalized object prefix, got `{cur_prefix}`"),
            });
        }
        // 10,000 basis points is one hundred per cent; a larger threshold would
        // report on every export and therefore on none.
        let margin_alert_threshold_bps = bounded(&lookup, MARGIN_THRESHOLD_VAR, 0, 10_000)?;
        let alarm_topic_arn = required(&lookup, ALARM_TOPIC_VAR)?;
        if !alarm_topic_arn.starts_with("arn:") {
            return Err(ConfigError::Invalid {
                name: ALARM_TOPIC_VAR,
                reason: format!("expected an ARN, got `{alarm_topic_arn}`"),
            });
        }
        let max_scan_bytes = positive(&lookup, MAX_SCAN_BYTES_VAR)?;
        let deadline_ms = bounded(&lookup, TX_DEADLINE_VAR, 1, 900_000)?;
        let mut data_api = DataApiConfig::new(cluster_arn, secret_arn, database);
        data_api.transaction_deadline = Duration::from_millis(u64::from(deadline_ms));
        data_api.statement_deadline = Duration::from_millis(u64::from(deadline_ms));
        data_api
            .validate()
            .map_err(|error| invalid(TX_DEADLINE_VAR, &error))?;
        Ok(Self {
            plane,
            region,
            data_api,
            database_role,
            cur_bucket,
            cur_prefix,
            margin_alert_threshold_bps,
            alarm_topic_arn,
            max_scan_bytes,
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

/// Reads a variable that must be an integer inside a closed range.
fn bounded<F>(lookup: &F, name: &'static str, low: u32, high: u32) -> Result<u32, ConfigError>
where
    F: Fn(&str) -> Option<String>,
{
    let raw = required(lookup, name)?;
    raw.parse::<u32>()
        .ok()
        .filter(|value| (low..=high).contains(value))
        .ok_or_else(|| ConfigError::Invalid {
            name,
            reason: format!("expected {low}..={high}, got `{raw}`"),
        })
}

/// Reads a variable that must be a positive integer.
fn positive<F>(lookup: &F, name: &'static str) -> Result<u64, ConfigError>
where
    F: Fn(&str) -> Option<String>,
{
    let raw = required(lookup, name)?;
    raw.parse::<u64>()
        .ok()
        .filter(|value| *value > 0)
        .ok_or_else(|| ConfigError::Invalid {
            name,
            reason: format!("expected a positive integer, got `{raw}`"),
        })
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
        CUR_PREFIX_VAR, Config, ConfigError, DATABASE_ROLE_VAR, MARGIN_THRESHOLD_VAR, NAMESPACE,
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
                "arn:aws:secretsmanager:eu-west-1:000000000000:secret:aex/provider-cost".to_owned(),
            ),
            (super::DATABASE_NAME_VAR, "aex".to_owned()),
            (super::DATABASE_ROLE_VAR, "aex_provider_cost".to_owned()),
            (super::CUR_BUCKET_VAR, "aex-prd-cost-exports".to_owned()),
            (super::CUR_PREFIX_VAR, "cur/aex-prd/".to_owned()),
            (super::MARGIN_THRESHOLD_VAR, "1500".to_owned()),
            (
                super::ALARM_TOPIC_VAR,
                "arn:aws:sns:eu-west-1:000000000000:aex-prd-finance-ops".to_owned(),
            ),
            (super::MAX_SCAN_BYTES_VAR, "536870912".to_owned()),
            (super::TX_DEADLINE_VAR, "60000".to_owned()),
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
        assert_eq!(config.margin_alert_threshold_bps, 1_500);
        assert_eq!(config.max_scan_bytes, 536_870_912);
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
    fn an_unnormalized_export_prefix_is_refused() {
        for value in ["/cur/aex-prd/", "cur/../aex-prd/"] {
            let mut vars = complete();
            vars.insert(CUR_PREFIX_VAR, value.to_owned());
            let error = read(&vars).expect_err("an object prefix is normalized");
            assert!(
                matches!(error, ConfigError::Invalid { name, .. } if name == CUR_PREFIX_VAR),
                "{error:?}"
            );
        }
    }

    #[test]
    fn a_margin_threshold_above_one_hundred_per_cent_is_refused() {
        let mut vars = complete();
        vars.insert(MARGIN_THRESHOLD_VAR, "10001".to_owned());
        let error = read(&vars).expect_err("a threshold above 100 per cent reports on nothing");
        assert!(
            matches!(error, ConfigError::Invalid { name, .. } if name == MARGIN_THRESHOLD_VAR),
            "{error:?}"
        );
    }

    #[test]
    fn the_wrong_database_role_is_refused_before_a_connection_exists() {
        let mut vars = complete();
        vars.insert(DATABASE_ROLE_VAR, "aex_finance_api".to_owned());
        let error = read(&vars).expect_err("the cost reconciler has its own role");
        assert!(
            matches!(error, ConfigError::Invalid { name, .. } if name == DATABASE_ROLE_VAR),
            "{error:?}"
        );
    }
}
