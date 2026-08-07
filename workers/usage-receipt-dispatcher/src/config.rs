//! Total, fail-fast start-up configuration for `usage-receipt-dispatcher`.

use std::collections::BTreeMap;
use std::time::Duration;

use aex_rds_data::config::{DataApiConfig, DatabaseName, ResourceArn, SecretArn};

/// The configuration namespace `release/units.toml` registers for this unit.
pub const NAMESPACE: &str = "AEX_USAGE_RECEIPT_";

/// The deployment plane.
pub const PLANE_VAR: &str = "AEX_USAGE_RECEIPT_PLANE";
/// The bound AWS region.
pub const REGION_VAR: &str = "AEX_USAGE_RECEIPT_REGION";
/// The Aurora cluster holding the `finance` schema.
pub const CLUSTER_ARN_VAR: &str = "AEX_USAGE_RECEIPT_AURORA_CLUSTER_ARN";
/// The Secrets Manager secret naming this deployable's own login role.
pub const SECRET_ARN_VAR: &str = "AEX_USAGE_RECEIPT_AURORA_SECRET_ARN";
/// The logical database inside the cluster.
pub const DATABASE_NAME_VAR: &str = "AEX_USAGE_RECEIPT_DATABASE_NAME";
/// The `PostgreSQL` role this deployable connects as.
pub const DATABASE_ROLE_VAR: &str = "AEX_USAGE_RECEIPT_DATABASE_ROLE";
/// The regional receipt queue for the compute category.
pub const COMPUTE_QUEUE_VAR: &str = "AEX_USAGE_RECEIPT_QUEUE_URL_COMPUTE";
/// The regional receipt queue for the storage category.
pub const STORAGE_QUEUE_VAR: &str = "AEX_USAGE_RECEIPT_QUEUE_URL_STORAGE";
/// The regional receipt queue for the transfer category.
pub const TRANSFER_QUEUE_VAR: &str = "AEX_USAGE_RECEIPT_QUEUE_URL_TRANSFER";
/// How many receipts one invocation replays per category.
pub const BATCH_SIZE_VAR: &str = "AEX_USAGE_RECEIPT_BATCH_SIZE";
/// How many attempts a receipt may take before it is reported.
pub const MAX_ATTEMPTS_VAR: &str = "AEX_USAGE_RECEIPT_MAX_ATTEMPTS";
/// The application deadline for one Aurora transaction.
pub const TX_DEADLINE_VAR: &str = "AEX_USAGE_RECEIPT_TX_DEADLINE_MS";

/// Every variable this deployable requires, in declaration order.
pub const REQUIRED_VARS: [&str; 12] = [
    PLANE_VAR,
    REGION_VAR,
    CLUSTER_ARN_VAR,
    SECRET_ARN_VAR,
    DATABASE_NAME_VAR,
    DATABASE_ROLE_VAR,
    COMPUTE_QUEUE_VAR,
    STORAGE_QUEUE_VAR,
    TRANSFER_QUEUE_VAR,
    BATCH_SIZE_VAR,
    MAX_ATTEMPTS_VAR,
    TX_DEADLINE_VAR,
];

/// The only role `usage-receipt-dispatcher` may connect as.
pub const REQUIRED_ROLE: &str = "aex_receipt_dispatcher";

/// The planes this deployable may be bound to.
pub const PLANES: [&str; 2] = ["dev", "prd"];

/// The three rating categories, which are also the three receipt queues.
pub const CATEGORIES: [&str; 3] = ["compute", "storage", "transfer"];

/// `SendMessageBatch` accepts at most ten entries.
pub const MAX_BATCH_SIZE: u32 = 10;

/// How much of the invocation deadline the drain loop leaves unspent.
///
/// `release/units.toml` gives this unit a 60 s Lambda timeout; the runtime
/// hands each invocation that deadline and kills the process at it. The margin
/// converts the platform deadline into a paging bound: the loop stops asking
/// for further pages once less than this remains, so the final queue round
/// trip, the response encode and the telemetry flush finish inside the timeout
/// instead of being killed mid-write.
pub const DEADLINE_SAFETY_MARGIN: Duration = Duration::from_secs(5);

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
    /// The receipt queue for each category.
    pub receipt_queue_url_by_category: BTreeMap<String, String>,
    /// How many receipts one invocation replays per category.
    pub batch_size: u32,
    /// How many attempts a receipt may take before it is reported.
    pub max_attempts: u32,
}

/// Why `usage-receipt-dispatcher` refused to start.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum UsageReceiptDispatcherConfigError {
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
    /// Returns [`UsageReceiptDispatcherConfigError::Missing`] naming the first absent or blank
    /// variable and [`UsageReceiptDispatcherConfigError::Invalid`] naming the first unusable one.
    pub fn from_env() -> Result<Self, UsageReceiptDispatcherConfigError> {
        Self::from_lookup(|name| std::env::var(name).ok())
    }

    /// Reads and validates the configuration from an arbitrary lookup.
    ///
    /// # Errors
    ///
    /// Identical to [`Config::from_env`].
    pub fn from_lookup<F>(lookup: F) -> Result<Self, UsageReceiptDispatcherConfigError>
    where
        F: Fn(&str) -> Option<String>,
    {
        let plane = required(&lookup, PLANE_VAR)?;
        if !PLANES.contains(&plane.as_str()) {
            return Err(UsageReceiptDispatcherConfigError::Invalid {
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
            return Err(UsageReceiptDispatcherConfigError::Invalid {
                name: DATABASE_ROLE_VAR,
                reason: format!("expected `{REQUIRED_ROLE}`, got `{database_role}`"),
            });
        }
        let mut receipt_queue_url_by_category = BTreeMap::new();
        for (category, name) in
            CATEGORIES
                .iter()
                .zip([COMPUTE_QUEUE_VAR, STORAGE_QUEUE_VAR, TRANSFER_QUEUE_VAR])
        {
            let url = required(&lookup, name)?;
            if !url.starts_with("https://") {
                return Err(UsageReceiptDispatcherConfigError::Invalid {
                    name,
                    reason: format!("expected an https queue URL, got `{url}`"),
                });
            }
            receipt_queue_url_by_category.insert((*category).to_owned(), url);
        }
        let batch_size = bounded(&lookup, BATCH_SIZE_VAR, 1, MAX_BATCH_SIZE)?;
        let max_attempts = bounded(&lookup, MAX_ATTEMPTS_VAR, 1, 100)?;
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
            receipt_queue_url_by_category,
            batch_size,
            max_attempts,
        })
    }
}

/// Reads a variable, treating blank as absent.
fn required<F>(lookup: &F, name: &'static str) -> Result<String, UsageReceiptDispatcherConfigError>
where
    F: Fn(&str) -> Option<String>,
{
    match lookup(name) {
        Some(value) if !value.trim().is_empty() => Ok(value.trim().to_owned()),
        _ => Err(UsageReceiptDispatcherConfigError::Missing { name }),
    }
}

/// Reads a variable that must be an integer inside a closed range.
fn bounded<F>(
    lookup: &F,
    name: &'static str,
    low: u32,
    high: u32,
) -> Result<u32, UsageReceiptDispatcherConfigError>
where
    F: Fn(&str) -> Option<String>,
{
    let raw = required(lookup, name)?;
    raw.parse::<u32>()
        .ok()
        .filter(|value| (low..=high).contains(value))
        .ok_or_else(|| UsageReceiptDispatcherConfigError::Invalid {
            name,
            reason: format!("expected {low}..={high}, got `{raw}`"),
        })
}

/// Renders a peer validation failure as this deployable's own refusal.
fn invalid(
    name: &'static str,
    error: &impl std::fmt::Display,
) -> UsageReceiptDispatcherConfigError {
    UsageReceiptDispatcherConfigError::Invalid {
        name,
        reason: error.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::{
        BATCH_SIZE_VAR, CATEGORIES, COMPUTE_QUEUE_VAR, Config, DATABASE_ROLE_VAR, NAMESPACE,
        REQUIRED_VARS, UsageReceiptDispatcherConfigError,
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
                "arn:aws:secretsmanager:eu-west-1:000000000000:secret:aex/receipt".to_owned(),
            ),
            (super::DATABASE_NAME_VAR, "aex".to_owned()),
            (
                super::DATABASE_ROLE_VAR,
                "aex_receipt_dispatcher".to_owned(),
            ),
            (
                super::COMPUTE_QUEUE_VAR,
                "https://sqs.eu-west-1.amazonaws.com/000000000000/aex-dev-usage-receipt-compute"
                    .to_owned(),
            ),
            (
                super::STORAGE_QUEUE_VAR,
                "https://sqs.eu-west-1.amazonaws.com/000000000000/aex-dev-usage-receipt-storage"
                    .to_owned(),
            ),
            (
                super::TRANSFER_QUEUE_VAR,
                "https://sqs.eu-west-1.amazonaws.com/000000000000/aex-dev-usage-receipt-transfer"
                    .to_owned(),
            ),
            (super::BATCH_SIZE_VAR, "10".to_owned()),
            (super::MAX_ATTEMPTS_VAR, "25".to_owned()),
            (super::TX_DEADLINE_VAR, "15000".to_owned()),
        ])
    }

    fn read(
        vars: &BTreeMap<&'static str, String>,
    ) -> Result<Config, UsageReceiptDispatcherConfigError> {
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
    fn a_complete_environment_binds_all_three_category_queues() {
        let config = read(&complete()).expect("the complete environment is accepted");
        assert_eq!(config.receipt_queue_url_by_category.len(), CATEGORIES.len());
        for category in CATEGORIES {
            assert!(
                config.receipt_queue_url_by_category.contains_key(category),
                "`{category}` has no bound queue"
            );
        }
    }

    #[test]
    fn each_missing_variable_is_named_in_the_refusal() {
        for name in REQUIRED_VARS {
            let mut vars = complete();
            vars.remove(name);
            assert_eq!(
                read(&vars),
                Err(UsageReceiptDispatcherConfigError::Missing { name }),
                "removing {name}"
            );
        }
    }

    #[test]
    fn a_batch_larger_than_the_service_accepts_is_refused() {
        let mut vars = complete();
        vars.insert(BATCH_SIZE_VAR, "11".to_owned());
        let error = read(&vars).expect_err("SendMessageBatch accepts at most ten entries");
        assert!(
            matches!(error, UsageReceiptDispatcherConfigError::Invalid { name, .. } if name == BATCH_SIZE_VAR),
            "{error:?}"
        );
    }

    #[test]
    fn a_queue_that_is_not_a_url_is_refused() {
        let mut vars = complete();
        vars.insert(
            COMPUTE_QUEUE_VAR,
            "aex-dev-usage-receipt-compute".to_owned(),
        );
        let error = read(&vars).expect_err("a queue is named by URL");
        assert!(
            matches!(error, UsageReceiptDispatcherConfigError::Invalid { name, .. } if name == COMPUTE_QUEUE_VAR),
            "{error:?}"
        );
    }

    #[test]
    fn the_wrong_database_role_is_refused_before_a_connection_exists() {
        let mut vars = complete();
        vars.insert(DATABASE_ROLE_VAR, "aex_finance_settlement".to_owned());
        let error = read(&vars).expect_err("the dispatcher has its own role");
        assert!(
            matches!(error, UsageReceiptDispatcherConfigError::Invalid { name, .. } if name == DATABASE_ROLE_VAR),
            "{error:?}"
        );
    }
}
