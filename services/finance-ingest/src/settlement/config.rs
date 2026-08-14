//! Total, fail-fast start-up configuration for `finance-settlement-worker`.

use std::time::Duration;

use aex_rds_data::config::{DataApiConfig, DatabaseName, ResourceArn, SecretArn};

/// The configuration namespace `release/units.toml` registers for this unit.
pub const NAMESPACE: &str = "AEX_BILLING_WORKER_";

/// The deployment plane.
pub const PLANE_VAR: &str = "AEX_BILLING_WORKER_PLANE";
/// The bound AWS region.
pub const REGION_VAR: &str = "AEX_BILLING_WORKER_REGION";
/// The Aurora cluster holding the `finance` schema.
pub const CLUSTER_ARN_VAR: &str = "AEX_BILLING_WORKER_AURORA_CLUSTER_ARN";
/// The Secrets Manager secret naming this deployable's own login role.
pub const SECRET_ARN_VAR: &str = "AEX_BILLING_WORKER_AURORA_SECRET_ARN";
/// The logical database inside the cluster.
pub const DATABASE_NAME_VAR: &str = "AEX_BILLING_WORKER_DATABASE_NAME";
/// The FIFO rating queue this worker drains.
pub const QUEUE_URL_VAR: &str = "AEX_BILLING_WORKER_RATING_QUEUE_URL";
/// The largest number of messages settled in one organization transaction.
pub const MAX_GROUP_BATCH_VAR: &str = "AEX_BILLING_WORKER_MAX_GROUP_BATCH";
/// How many times a serialization failure may be retried inside one invocation.
pub const SERIALIZATION_RETRY_VAR: &str = "AEX_BILLING_WORKER_SERIALIZATION_RETRY_MAX";
/// The application deadline for one Aurora transaction.
pub const TX_DEADLINE_VAR: &str = "AEX_BILLING_WORKER_TX_DEADLINE_MS";

/// Every variable this deployable requires, in declaration order.
pub const REQUIRED_VARS: [&str; 9] = [
    PLANE_VAR,
    REGION_VAR,
    CLUSTER_ARN_VAR,
    SECRET_ARN_VAR,
    DATABASE_NAME_VAR,
    QUEUE_URL_VAR,
    MAX_GROUP_BATCH_VAR,
    SERIALIZATION_RETRY_VAR,
    TX_DEADLINE_VAR,
];

/// The only role `finance-settlement-worker` may connect as.
pub const REQUIRED_ROLE: &str = "aex_finance_settlement";

/// The planes this deployable may be bound to.
pub const PLANES: [&str; 2] = ["dev", "prd"];

/// The rating queue is FIFO by F-10, and a FIFO queue URL always ends `.fifo`.
pub const FIFO_SUFFIX: &str = ".fifo";

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
    /// The FIFO rating queue.
    pub queue_url: String,
    /// The largest number of facts settled in one organization transaction.
    pub max_group_batch: u32,
    /// Bounded serialization retries inside one invocation.
    pub serialization_retry_max: u32,
}

/// Why `finance-settlement-worker` refused to start.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum FinanceSettlementWorkerConfigError {
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
    /// Returns [`FinanceSettlementWorkerConfigError::Missing`] naming the first absent or blank
    /// variable and [`FinanceSettlementWorkerConfigError::Invalid`] naming the first unusable one.
    pub fn from_env() -> Result<Self, FinanceSettlementWorkerConfigError> {
        Self::from_lookup(|name| std::env::var(name).ok())
    }

    /// Reads and validates the configuration from an arbitrary lookup.
    ///
    /// # Errors
    ///
    /// Identical to [`Config::from_env`].
    pub fn from_lookup<F>(lookup: F) -> Result<Self, FinanceSettlementWorkerConfigError>
    where
        F: Fn(&str) -> Option<String>,
    {
        let plane = required(&lookup, PLANE_VAR)?;
        if !PLANES.contains(&plane.as_str()) {
            return Err(FinanceSettlementWorkerConfigError::Invalid {
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
        let database_role = REQUIRED_ROLE.to_owned();
        let queue_url = required(&lookup, QUEUE_URL_VAR)?;
        if !queue_url.ends_with(FIFO_SUFFIX) {
            return Err(FinanceSettlementWorkerConfigError::Invalid {
                name: QUEUE_URL_VAR,
                reason: format!(
                    "F-10 pins the rating queue to SQS FIFO; `{queue_url}` is not a `{FIFO_SUFFIX}` \
                     queue"
                ),
            });
        }
        let max_group_batch = bounded(&lookup, MAX_GROUP_BATCH_VAR, 1, 10_000)?;
        let serialization_retry_max = bounded(&lookup, SERIALIZATION_RETRY_VAR, 1, 3)?;
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
            queue_url,
            max_group_batch,
            serialization_retry_max,
        })
    }
}

/// Reads a variable, treating blank as absent.
fn required<F>(lookup: &F, name: &'static str) -> Result<String, FinanceSettlementWorkerConfigError>
where
    F: Fn(&str) -> Option<String>,
{
    match lookup(name) {
        Some(value) if !value.trim().is_empty() => Ok(value.trim().to_owned()),
        _ => Err(FinanceSettlementWorkerConfigError::Missing { name }),
    }
}

/// Reads a variable that must be an integer inside a closed range.
fn bounded<F>(
    lookup: &F,
    name: &'static str,
    low: u32,
    high: u32,
) -> Result<u32, FinanceSettlementWorkerConfigError>
where
    F: Fn(&str) -> Option<String>,
{
    let raw = required(lookup, name)?;
    raw.parse::<u32>()
        .ok()
        .filter(|value| (low..=high).contains(value))
        .ok_or_else(|| FinanceSettlementWorkerConfigError::Invalid {
            name,
            reason: format!("expected {low}..={high}, got `{raw}`"),
        })
}

/// Renders a peer validation failure as this deployable's own refusal.
fn invalid(
    name: &'static str,
    error: &impl std::fmt::Display,
) -> FinanceSettlementWorkerConfigError {
    FinanceSettlementWorkerConfigError::Invalid {
        name,
        reason: error.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::{
        Config, FinanceSettlementWorkerConfigError, NAMESPACE, QUEUE_URL_VAR, REQUIRED_VARS,
        SERIALIZATION_RETRY_VAR,
    };

    fn complete() -> BTreeMap<&'static str, String> {
        BTreeMap::from([
            (super::PLANE_VAR, "dev".to_owned()),
            (super::REGION_VAR, "eu-west-1".to_owned()),
            (
                super::CLUSTER_ARN_VAR,
                "arn:aws:rds:eu-west-1:000000000000:cluster:aex-central".to_owned(),
            ),
            (
                super::SECRET_ARN_VAR,
                "arn:aws:secretsmanager:eu-west-1:000000000000:secret:aex/settlement".to_owned(),
            ),
            (super::DATABASE_NAME_VAR, "aex".to_owned()),
            (
                super::QUEUE_URL_VAR,
                "https://sqs.eu-west-1.amazonaws.com/000000000000/aex-dev-usage-rating.fifo"
                    .to_owned(),
            ),
            (super::MAX_GROUP_BATCH_VAR, "100".to_owned()),
            (super::SERIALIZATION_RETRY_VAR, "3".to_owned()),
            (super::TX_DEADLINE_VAR, "20000".to_owned()),
        ])
    }

    fn read(
        vars: &BTreeMap<&'static str, String>,
    ) -> Result<Config, FinanceSettlementWorkerConfigError> {
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
        assert_eq!(config.max_group_batch, 100);
        assert_eq!(config.serialization_retry_max, 3);
    }

    #[test]
    fn each_missing_variable_is_named_in_the_refusal() {
        for name in REQUIRED_VARS {
            let mut vars = complete();
            vars.remove(name);
            assert_eq!(
                read(&vars),
                Err(FinanceSettlementWorkerConfigError::Missing { name }),
                "removing {name}"
            );
        }
    }

    #[test]
    fn a_standard_queue_is_refused_because_account_order_is_a_correctness_property() {
        let mut vars = complete();
        vars.insert(
            QUEUE_URL_VAR,
            "https://sqs.eu-west-1.amazonaws.com/000000000000/aex-dev-usage-rating".to_owned(),
        );
        let error = read(&vars).expect_err("F-10 pins the rating queue to FIFO");
        assert!(
            matches!(error, FinanceSettlementWorkerConfigError::Invalid { name, .. } if name == QUEUE_URL_VAR),
            "{error:?}"
        );
    }

    #[test]
    fn an_unbounded_serialization_retry_is_refused() {
        let mut vars = complete();
        vars.insert(SERIALIZATION_RETRY_VAR, "50".to_owned());
        let error = read(&vars).expect_err("retries are bounded at three");
        assert!(
            matches!(error, FinanceSettlementWorkerConfigError::Invalid { name, .. } if name == SERIALIZATION_RETRY_VAR),
            "{error:?}"
        );
    }
}
