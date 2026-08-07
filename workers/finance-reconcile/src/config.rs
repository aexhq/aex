//! Total, fail-fast start-up configuration for `finance-reconcile`.

use std::time::Duration;

use aex_rds_data::config::{DataApiConfig, DatabaseName, ResourceArn, SecretArn};

/// The configuration namespace `release/units.toml` registers for this unit.
pub const NAMESPACE: &str = "AEX_FINANCE_RECONCILE_";

/// The deployment plane.
pub const PLANE_VAR: &str = "AEX_FINANCE_RECONCILE_PLANE";
/// The bound AWS region.
pub const REGION_VAR: &str = "AEX_FINANCE_RECONCILE_REGION";
/// The Aurora cluster holding the `finance` schema.
pub const CLUSTER_ARN_VAR: &str = "AEX_FINANCE_RECONCILE_AURORA_CLUSTER_ARN";
/// The Secrets Manager secret naming this deployable's own login role.
pub const SECRET_ARN_VAR: &str = "AEX_FINANCE_RECONCILE_AURORA_SECRET_ARN";
/// The logical database inside the cluster.
pub const DATABASE_NAME_VAR: &str = "AEX_FINANCE_RECONCILE_DATABASE_NAME";
/// The `PostgreSQL` role this deployable connects as.
pub const DATABASE_ROLE_VAR: &str = "AEX_FINANCE_RECONCILE_DATABASE_ROLE";
/// The `stripe-command-edge` function this deployable may invoke.
pub const COMMAND_EDGE_ARN_VAR: &str = "AEX_FINANCE_RECONCILE_STRIPE_COMMAND_EDGE_ARN";
/// How long an unresolved effect may be recovered by exact-key replay.
pub const RETRY_WINDOW_VAR: &str = "AEX_FINANCE_RECONCILE_UNKNOWN_EFFECT_RETRY_WINDOW_HOURS";
/// The hard page budget of one sweep.
pub const SWEEP_PAGE_VAR: &str = "AEX_FINANCE_RECONCILE_SWEEP_PAGE";
/// The operations topic every finding is reported to.
pub const ALARM_TOPIC_VAR: &str = "AEX_FINANCE_RECONCILE_ALARM_TOPIC_ARN";
/// The bucket issued statement artifacts are written to.
pub const STATEMENT_BUCKET_VAR: &str = "AEX_FINANCE_RECONCILE_STATEMENT_BUCKET";
/// The application deadline for one Aurora transaction.
pub const TX_DEADLINE_VAR: &str = "AEX_FINANCE_RECONCILE_TX_DEADLINE_MS";

/// Every variable this deployable requires, in declaration order.
pub const REQUIRED_VARS: [&str; 12] = [
    PLANE_VAR,
    REGION_VAR,
    CLUSTER_ARN_VAR,
    SECRET_ARN_VAR,
    DATABASE_NAME_VAR,
    DATABASE_ROLE_VAR,
    COMMAND_EDGE_ARN_VAR,
    RETRY_WINDOW_VAR,
    SWEEP_PAGE_VAR,
    ALARM_TOPIC_VAR,
    STATEMENT_BUCKET_VAR,
    TX_DEADLINE_VAR,
];

/// The only role `finance-reconcile` may connect as.
pub const REQUIRED_ROLE: &str = "aex_finance_reconcile";

/// The planes this deployable may be bound to.
pub const PLANES: [&str; 2] = ["dev", "prd"];

/// F-15: half of the window the provider guarantees for key replay.
pub const MAX_RETRY_WINDOW_HOURS: u32 = 12;

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
    /// How long exact-key replay stays admissible.
    pub unknown_effect_retry_window: Duration,
    /// The hard page budget of one sweep.
    pub sweep_page: u32,
    /// The operations topic.
    pub alarm_topic_arn: String,
    /// The statement artifact bucket.
    pub statement_bucket: String,
}

/// Why `finance-reconcile` refused to start.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum FinanceReconcileConfigError {
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
    /// Returns [`FinanceReconcileConfigError::Missing`] naming the first absent or blank
    /// variable and [`FinanceReconcileConfigError::Invalid`] naming the first unusable one.
    pub fn from_env() -> Result<Self, FinanceReconcileConfigError> {
        Self::from_lookup(|name| std::env::var(name).ok())
    }

    /// Reads and validates the configuration from an arbitrary lookup.
    ///
    /// # Errors
    ///
    /// Identical to [`Config::from_env`].
    pub fn from_lookup<F>(lookup: F) -> Result<Self, FinanceReconcileConfigError>
    where
        F: Fn(&str) -> Option<String>,
    {
        let plane = required(&lookup, PLANE_VAR)?;
        if !PLANES.contains(&plane.as_str()) {
            return Err(FinanceReconcileConfigError::Invalid {
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
            return Err(FinanceReconcileConfigError::Invalid {
                name: DATABASE_ROLE_VAR,
                reason: format!("expected `{REQUIRED_ROLE}`, got `{database_role}`"),
            });
        }
        let command_edge_arn = arn(&lookup, COMMAND_EDGE_ARN_VAR)?;
        let window_hours = bounded(&lookup, RETRY_WINDOW_VAR, 1, MAX_RETRY_WINDOW_HOURS)?;
        let sweep_page = bounded(&lookup, SWEEP_PAGE_VAR, 1, 10_000)?;
        let alarm_topic_arn = arn(&lookup, ALARM_TOPIC_VAR)?;
        let statement_bucket = required(&lookup, STATEMENT_BUCKET_VAR)?;
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
            command_edge_arn,
            unknown_effect_retry_window: Duration::from_secs(u64::from(window_hours) * 3_600),
            sweep_page,
            alarm_topic_arn,
            statement_bucket,
        })
    }
}

/// Reads a variable, treating blank as absent.
fn required<F>(lookup: &F, name: &'static str) -> Result<String, FinanceReconcileConfigError>
where
    F: Fn(&str) -> Option<String>,
{
    match lookup(name) {
        Some(value) if !value.trim().is_empty() => Ok(value.trim().to_owned()),
        _ => Err(FinanceReconcileConfigError::Missing { name }),
    }
}

/// Reads a variable that must be an ARN.
fn arn<F>(lookup: &F, name: &'static str) -> Result<String, FinanceReconcileConfigError>
where
    F: Fn(&str) -> Option<String>,
{
    let value = required(lookup, name)?;
    if value.starts_with("arn:") {
        Ok(value)
    } else {
        Err(FinanceReconcileConfigError::Invalid {
            name,
            reason: format!("expected an ARN, got `{value}`"),
        })
    }
}

/// Reads a variable that must be an integer inside a closed range.
fn bounded<F>(
    lookup: &F,
    name: &'static str,
    low: u32,
    high: u32,
) -> Result<u32, FinanceReconcileConfigError>
where
    F: Fn(&str) -> Option<String>,
{
    let raw = required(lookup, name)?;
    raw.parse::<u32>()
        .ok()
        .filter(|value| (low..=high).contains(value))
        .ok_or_else(|| FinanceReconcileConfigError::Invalid {
            name,
            reason: format!("expected {low}..={high}, got `{raw}`"),
        })
}

/// Renders a peer validation failure as this deployable's own refusal.
fn invalid(name: &'static str, error: &impl std::fmt::Display) -> FinanceReconcileConfigError {
    FinanceReconcileConfigError::Invalid {
        name,
        reason: error.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::{
        ALARM_TOPIC_VAR, Config, DATABASE_ROLE_VAR, FinanceReconcileConfigError, NAMESPACE,
        REQUIRED_VARS, RETRY_WINDOW_VAR,
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
                "arn:aws:secretsmanager:eu-west-1:000000000000:secret:aex/reconcile".to_owned(),
            ),
            (super::DATABASE_NAME_VAR, "aex".to_owned()),
            (super::DATABASE_ROLE_VAR, "aex_finance_reconcile".to_owned()),
            (
                super::COMMAND_EDGE_ARN_VAR,
                "arn:aws:lambda:eu-west-1:000000000000:function:aex-prd-stripe-command-edge"
                    .to_owned(),
            ),
            (super::RETRY_WINDOW_VAR, "12".to_owned()),
            (super::SWEEP_PAGE_VAR, "500".to_owned()),
            (
                super::ALARM_TOPIC_VAR,
                "arn:aws:sns:eu-west-1:000000000000:aex-prd-finance-ops".to_owned(),
            ),
            (super::STATEMENT_BUCKET_VAR, "aex-prd-statements".to_owned()),
            (super::TX_DEADLINE_VAR, "30000".to_owned()),
        ])
    }

    fn read(vars: &BTreeMap<&'static str, String>) -> Result<Config, FinanceReconcileConfigError> {
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
        assert_eq!(config.unknown_effect_retry_window.as_secs(), 12 * 3_600);
        assert_eq!(config.sweep_page, 500);
    }

    #[test]
    fn each_missing_variable_is_named_in_the_refusal() {
        for name in REQUIRED_VARS {
            let mut vars = complete();
            vars.remove(name);
            assert_eq!(
                read(&vars),
                Err(FinanceReconcileConfigError::Missing { name }),
                "removing {name}"
            );
        }
    }

    #[test]
    fn a_replay_window_beyond_the_provider_guarantee_is_refused() {
        let mut vars = complete();
        vars.insert(RETRY_WINDOW_VAR, "48".to_owned());
        let error = read(&vars).expect_err("F-15 bounds exact-key replay at twelve hours");
        assert!(
            matches!(error, FinanceReconcileConfigError::Invalid { name, .. } if name == RETRY_WINDOW_VAR),
            "{error:?}"
        );
    }

    #[test]
    fn an_alarm_topic_that_is_not_an_arn_is_refused() {
        let mut vars = complete();
        vars.insert(ALARM_TOPIC_VAR, "finance-ops".to_owned());
        let error = read(&vars).expect_err("a topic is named by ARN");
        assert!(
            matches!(error, FinanceReconcileConfigError::Invalid { name, .. } if name == ALARM_TOPIC_VAR),
            "{error:?}"
        );
    }

    #[test]
    fn the_wrong_database_role_is_refused_before_a_connection_exists() {
        let mut vars = complete();
        vars.insert(DATABASE_ROLE_VAR, "aex_finance_api".to_owned());
        let error = read(&vars).expect_err("the reconciler has its own role");
        assert!(
            matches!(error, FinanceReconcileConfigError::Invalid { name, .. } if name == DATABASE_ROLE_VAR),
            "{error:?}"
        );
    }
}
