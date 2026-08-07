//! Validated transport configuration.
//!
//! Nothing here has a default that names a resource. The two byte budgets do
//! have defaults, because they are policy rather than identity, and both sit
//! strictly under the Data `API`'s own caps so a breach is reported by this
//! crate rather than by the service.

use std::time::Duration;

/// The Data `API` refuses a result set larger than one mebibyte.
pub const DATA_API_RESULT_CAP_BYTES: usize = 1_048_576;
/// The Data `API` refuses a single field larger than 64 kibibytes.
pub const DATA_API_FIELD_CAP_BYTES: usize = 65_536;
/// The result budget this transport enforces, under the service cap.
pub const DEFAULT_MAX_RESULT_BYTES: usize = 786_432;
/// The per-field budget this transport enforces, under the service cap.
pub const DEFAULT_MAX_FIELD_BYTES: usize = 57_344;

/// Why a configuration was refused.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RdsDataConfigError {
    /// A required value was empty.
    #[error("`{field}` must not be empty")]
    Empty {
        /// Which field.
        field: &'static str,
    },
    /// An ARN did not have the expected shape.
    #[error("`{field}` is not an ARN: {value}")]
    NotAnArn {
        /// Which field.
        field: &'static str,
        /// What was supplied, which is an ARN and never a secret value.
        value: String,
    },
    /// A budget was zero or above the service cap.
    #[error("`{field}` must be in 1..={cap}, got {value}")]
    BudgetOutOfRange {
        /// Which field.
        field: &'static str,
        /// The service cap it must stay under.
        cap: usize,
        /// What was supplied.
        value: usize,
    },
    /// A deadline was not positive.
    #[error("`{field}` must be a positive duration")]
    NonPositiveDeadline {
        /// Which field.
        field: &'static str,
    },
}

/// The Aurora cluster ARN.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ResourceArn(String);

/// The Secrets Manager ARN of the login role this transport connects as.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SecretArn(String);

/// The logical database inside the cluster.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct DatabaseName(String);

macro_rules! arn_newtype {
    ($name:ident, $field:literal) => {
        impl $name {
            /// Validates and wraps an ARN.
            ///
            /// # Errors
            ///
            /// Returns [`RdsDataConfigError::Empty`] for an empty value and
            /// [`RdsDataConfigError::NotAnArn`] when the value is not an `arn:` string.
            pub fn parse(raw: &str) -> Result<Self, RdsDataConfigError> {
                let trimmed = raw.trim();
                if trimmed.is_empty() {
                    return Err(RdsDataConfigError::Empty { field: $field });
                }
                if !trimmed.starts_with("arn:") || trimmed.split(':').count() < 6 {
                    return Err(RdsDataConfigError::NotAnArn {
                        field: $field,
                        value: trimmed.to_owned(),
                    });
                }
                Ok(Self(trimmed.to_owned()))
            }

            /// The ARN as a string slice.
            #[must_use]
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }
    };
}

arn_newtype!(ResourceArn, "resource_arn");
arn_newtype!(SecretArn, "secret_arn");

impl DatabaseName {
    /// Validates and wraps a database name.
    ///
    /// # Errors
    ///
    /// Returns [`RdsDataConfigError::Empty`] when the name is blank.
    pub fn parse(raw: &str) -> Result<Self, RdsDataConfigError> {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            return Err(RdsDataConfigError::Empty { field: "database" });
        }
        Ok(Self(trimmed.to_owned()))
    }

    /// The name as a string slice.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Everything the transport needs to address one cluster.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DataApiConfig {
    /// The Aurora cluster.
    pub resource_arn: ResourceArn,
    /// The Secrets Manager secret naming the login role.
    pub secret_arn: SecretArn,
    /// The logical database.
    pub database: DatabaseName,
    /// Client-side deadline for one statement.
    pub statement_deadline: Duration,
    /// Client-side deadline for a whole transaction, guarded well inside the
    /// Data `API`'s own transaction expiry.
    pub transaction_deadline: Duration,
    /// Largest decoded response this transport accepts.
    pub max_result_bytes: usize,
    /// Largest single field this transport accepts.
    pub max_field_bytes: usize,
}

impl DataApiConfig {
    /// The default statement deadline.
    pub const DEFAULT_STATEMENT_DEADLINE: Duration = Duration::from_secs(5);
    /// The default transaction deadline.
    pub const DEFAULT_TRANSACTION_DEADLINE: Duration = Duration::from_secs(10);

    /// Builds a configuration with the pinned default budgets and deadlines.
    #[must_use]
    pub fn new(resource_arn: ResourceArn, secret_arn: SecretArn, database: DatabaseName) -> Self {
        Self {
            resource_arn,
            secret_arn,
            database,
            statement_deadline: Self::DEFAULT_STATEMENT_DEADLINE,
            transaction_deadline: Self::DEFAULT_TRANSACTION_DEADLINE,
            max_result_bytes: DEFAULT_MAX_RESULT_BYTES,
            max_field_bytes: DEFAULT_MAX_FIELD_BYTES,
        }
    }

    /// Checks the budgets and deadlines against the service caps.
    ///
    /// # Errors
    ///
    /// Returns [`RdsDataConfigError::BudgetOutOfRange`] when a byte budget is zero or
    /// at/above the service cap, and [`RdsDataConfigError::NonPositiveDeadline`] when a
    /// deadline is zero.
    pub fn validate(&self) -> Result<(), RdsDataConfigError> {
        if self.max_result_bytes == 0 || self.max_result_bytes >= DATA_API_RESULT_CAP_BYTES {
            return Err(RdsDataConfigError::BudgetOutOfRange {
                field: "max_result_bytes",
                cap: DATA_API_RESULT_CAP_BYTES,
                value: self.max_result_bytes,
            });
        }
        if self.max_field_bytes == 0 || self.max_field_bytes >= DATA_API_FIELD_CAP_BYTES {
            return Err(RdsDataConfigError::BudgetOutOfRange {
                field: "max_field_bytes",
                cap: DATA_API_FIELD_CAP_BYTES,
                value: self.max_field_bytes,
            });
        }
        if self.statement_deadline.is_zero() {
            return Err(RdsDataConfigError::NonPositiveDeadline {
                field: "statement_deadline",
            });
        }
        if self.transaction_deadline.is_zero() {
            return Err(RdsDataConfigError::NonPositiveDeadline {
                field: "transaction_deadline",
            });
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::{
        DATA_API_FIELD_CAP_BYTES, DATA_API_RESULT_CAP_BYTES, DataApiConfig, DatabaseName,
        RdsDataConfigError, ResourceArn, SecretArn,
    };
    use std::time::Duration;

    fn config() -> DataApiConfig {
        DataApiConfig::new(
            ResourceArn::parse("arn:aws:rds:eu-west-1:000000000000:cluster:aex").expect("arn"),
            SecretArn::parse("arn:aws:secretsmanager:eu-west-1:000000000000:secret:aex-x")
                .expect("arn"),
            DatabaseName::parse("aex").expect("database"),
        )
    }

    #[test]
    fn an_arn_must_look_like_one() {
        assert_eq!(
            ResourceArn::parse("  "),
            Err(RdsDataConfigError::Empty {
                field: "resource_arn"
            })
        );
        assert_eq!(
            ResourceArn::parse("cluster-aex"),
            Err(RdsDataConfigError::NotAnArn {
                field: "resource_arn",
                value: "cluster-aex".to_owned()
            })
        );
    }

    #[test]
    fn the_default_budgets_sit_under_the_service_caps() {
        let config = config();
        assert!(config.max_result_bytes < DATA_API_RESULT_CAP_BYTES);
        assert!(config.max_field_bytes < DATA_API_FIELD_CAP_BYTES);
        assert_eq!(config.validate(), Ok(()));
    }

    #[test]
    fn a_budget_at_or_above_the_service_cap_is_refused() {
        let mut config = config();
        config.max_result_bytes = DATA_API_RESULT_CAP_BYTES;
        assert_eq!(
            config.validate(),
            Err(RdsDataConfigError::BudgetOutOfRange {
                field: "max_result_bytes",
                cap: DATA_API_RESULT_CAP_BYTES,
                value: DATA_API_RESULT_CAP_BYTES
            })
        );
    }

    #[test]
    fn a_zero_deadline_is_refused() {
        let mut config = config();
        config.statement_deadline = Duration::ZERO;
        assert_eq!(
            config.validate(),
            Err(RdsDataConfigError::NonPositiveDeadline {
                field: "statement_deadline"
            })
        );
    }
}
