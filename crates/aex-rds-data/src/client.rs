//! The transport seam and the client that enforces the byte budgets.
//!
//! [`Transport`] is the whole surface this crate needs from the Data `API`: four
//! calls, each returning either a service exception or an indeterminate
//! outcome. Splitting the seam there is what makes the budget, deadline and
//! commit-ambiguity rules testable without an AWS endpoint, and it is also why
//! the `*-aurora` crates can count statements in their own suites.

use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use aws_sdk_rdsdata::types::Field;

use crate::config::DataApiConfig;
use crate::error::{DataApiError, ExceptionKind, classify};
use crate::params::Statement;
use crate::record::{Record, Row, field_bytes};
use crate::transaction::{
    CommitFailure, CommitOutcome, Committed, Isolation, Transaction, TransactionId,
};

/// What a transport call reports when it does not succeed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TransportError {
    /// The service answered with a named exception. The outcome is known.
    Service {
        /// Which exception the service named.
        kind: ExceptionKind,
        /// The rendered message, which carries the `SQLSTATE` when there is one.
        message: String,
    },
    /// The call did not produce an answer. The outcome is **not** known: the
    /// statement may or may not have been applied.
    Indeterminate {
        /// A low-cardinality rendering of the transport failure.
        message: String,
    },
}

impl TransportError {
    /// Turns a transport failure into the classified transport error.
    #[must_use]
    pub fn classify(&self) -> DataApiError {
        match self {
            Self::Service { kind, message } => classify(*kind, message),
            Self::Indeterminate { message } => DataApiError::Unavailable {
                message: message.clone(),
            },
        }
    }
}

/// One result set, before decoding.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ExecuteResponse {
    /// The returned records, if any.
    pub records: Vec<Vec<Field>>,
    /// How many rows a DML statement affected.
    pub rows_affected: u64,
}

/// The four Data `API` calls this crate makes.
#[async_trait]
pub trait Transport: Send + Sync + std::fmt::Debug {
    /// Runs one statement, optionally inside a transaction.
    async fn execute(
        &self,
        sql: &str,
        parameters: Vec<aws_sdk_rdsdata::types::SqlParameter>,
        transaction: Option<&TransactionId>,
    ) -> Result<ExecuteResponse, TransportError>;

    /// Opens a transaction.
    async fn begin(&self) -> Result<TransactionId, TransportError>;

    /// Commits a transaction, returning the status string the service reported.
    async fn commit(&self, transaction: &TransactionId) -> Result<String, TransportError>;

    /// Rolls a transaction back.
    async fn rollback(&self, transaction: &TransactionId) -> Result<(), TransportError>;
}

/// The Aurora Data `API` client.
#[derive(Debug, Clone)]
pub struct DataApiClient {
    transport: Arc<dyn Transport>,
    config: DataApiConfig,
}

impl DataApiClient {
    /// Builds a client over a transport.
    #[must_use]
    pub fn new(transport: Arc<dyn Transport>, config: DataApiConfig) -> Self {
        Self { transport, config }
    }

    /// The configuration this client was built with.
    #[must_use]
    pub const fn config(&self) -> &DataApiConfig {
        &self.config
    }

    /// The transport, for the transaction type.
    pub(crate) fn transport(&self) -> &dyn Transport {
        self.transport.as_ref()
    }

    /// Runs a statement outside any transaction and decodes every record.
    ///
    /// # Errors
    ///
    /// Returns [`DataApiError`] for a classified service failure, a breached
    /// byte budget, or a record that does not decode into `R`.
    pub async fn query<R: Row>(&self, statement: Statement<'_>) -> Result<Vec<R>, DataApiError> {
        let response = self.run(statement, None).await?;
        decode(&response, &self.config)
    }

    /// Runs a statement expecting at most one record.
    ///
    /// # Errors
    ///
    /// Returns [`DataApiError::Decode`] with an arity mismatch when more than
    /// one record arrives, plus every failure [`DataApiClient::query`] reports.
    pub async fn query_opt<R: Row>(
        &self,
        statement: Statement<'_>,
    ) -> Result<Option<R>, DataApiError> {
        let mut rows = self.query::<R>(statement).await?;
        if rows.len() > 1 {
            return Err(DataApiError::Decode(
                crate::error::DecodeError::ArityMismatch {
                    expected: 1,
                    actual: rows.len(),
                },
            ));
        }
        Ok(rows.pop())
    }

    /// Runs a statement expecting exactly one record.
    ///
    /// # Errors
    ///
    /// Returns [`DataApiError::Decode`] with an arity mismatch when the record
    /// count is not one, plus every failure [`DataApiClient::query`] reports.
    pub async fn query_one<R: Row>(&self, statement: Statement<'_>) -> Result<R, DataApiError> {
        self.query_opt::<R>(statement)
            .await?
            .ok_or(DataApiError::Decode(
                crate::error::DecodeError::ArityMismatch {
                    expected: 1,
                    actual: 0,
                },
            ))
    }

    /// Runs a DML statement outside any transaction.
    ///
    /// # Errors
    ///
    /// Returns [`DataApiError`] for a classified service failure.
    pub async fn execute(&self, statement: Statement<'_>) -> Result<u64, DataApiError> {
        Ok(self.run(statement, None).await?.rows_affected)
    }

    /// Opens a transaction with the requested isolation level.
    ///
    /// # Errors
    ///
    /// Returns [`DataApiError`] when the transaction cannot be opened or the
    /// isolation statement is refused.
    pub async fn begin(&self, isolation: Isolation) -> Result<Transaction<'_>, DataApiError> {
        let id = self
            .transport
            .begin()
            .await
            .map_err(|error| error.classify())?;
        let deadline = Instant::now() + self.config.transaction_deadline;
        let mut transaction = Transaction::new(self, id, deadline);
        if let Some(sql) = isolation.statement() {
            transaction.execute(Statement::new(sql)).await?;
        }
        Ok(transaction)
    }

    /// Runs one statement and applies the byte budgets to the response.
    pub(crate) async fn run(
        &self,
        statement: Statement<'_>,
        transaction: Option<&TransactionId>,
    ) -> Result<ExecuteResponse, DataApiError> {
        let parameters = statement.parameters();
        let response = self
            .transport
            .execute(statement.sql, parameters, transaction)
            .await
            .map_err(|error| error.classify())?;
        guard(&response, &self.config)?;
        Ok(response)
    }

    /// Finishes a transaction, mapping the service answer onto the ambiguity
    /// rule: only a response that explicitly states the transaction aborted is
    /// [`CommitFailure::RolledBack`].
    pub(crate) async fn finish(&self, id: &TransactionId) -> CommitOutcome {
        match self.transport.commit(id).await {
            Ok(status) if status.to_ascii_lowercase().contains("rollback") => {
                Err(CommitFailure::RolledBack(DataApiError::Fatal {
                    code: None,
                    message: status,
                }))
            }
            Ok(_) => Ok(Committed),
            Err(error) => {
                let classified = error.classify();
                if definitively_aborted(&classified) {
                    Err(CommitFailure::RolledBack(classified))
                } else {
                    Err(CommitFailure::Unknown(classified))
                }
            }
        }
    }
}

/// Whether the classified failure states the transaction aborted.
///
/// A deferred constraint trigger raises at `COMMIT`; `PostgreSQL` has then
/// definitively rolled the transaction back, and treating that as ambiguous
/// would send a caller to reconcile a transaction that provably left no trace.
/// Everything else — a lost response, a timeout, a throttle, an unknown
/// transaction id — is ambiguous.
fn definitively_aborted(error: &DataApiError) -> bool {
    matches!(
        error,
        DataApiError::Serialization
            | DataApiError::Deadlock
            | DataApiError::UniqueViolation { .. }
            | DataApiError::ForeignKeyViolation { .. }
            | DataApiError::CheckViolation { .. }
            | DataApiError::NotNullViolation { .. }
            | DataApiError::IntegrityConstraintViolation { .. }
    )
}

/// Applies the result and field budgets **before** any record is decoded.
fn guard(response: &ExecuteResponse, config: &DataApiConfig) -> Result<(), DataApiError> {
    let mut total = 0_usize;
    for record in &response.records {
        for (index, field) in record.iter().enumerate() {
            let bytes = field_bytes(field);
            if bytes > config.max_field_bytes {
                return Err(DataApiError::FieldTooLarge { index, bytes });
            }
            total = total.saturating_add(bytes);
            if total > config.max_result_bytes {
                return Err(DataApiError::ResultTooLarge { bytes: total });
            }
        }
    }
    Ok(())
}

/// Decodes every record after the budgets have passed.
fn decode<R: Row>(
    response: &ExecuteResponse,
    _config: &DataApiConfig,
) -> Result<Vec<R>, DataApiError> {
    response
        .records
        .iter()
        .map(|fields| R::from_record(&Record::new(fields)).map_err(DataApiError::Decode))
        .collect()
}

/// The client-side deadline for one statement, as a duration remaining.
#[must_use]
pub fn remaining(deadline: Instant, now: Instant) -> Duration {
    deadline.saturating_duration_since(now)
}

#[cfg(test)]
mod tests {
    use super::{ExecuteResponse, TransportError, definitively_aborted, guard};
    use crate::config::{DataApiConfig, DatabaseName, ResourceArn, SecretArn};
    use crate::error::{DataApiError, ExceptionKind};
    use aws_sdk_rdsdata::types::Field;

    fn config() -> DataApiConfig {
        let mut config = DataApiConfig::new(
            ResourceArn::parse("arn:aws:rds:eu-west-1:000000000000:cluster:aex").expect("arn"),
            SecretArn::parse("arn:aws:secretsmanager:eu-west-1:000000000000:secret:aex-x")
                .expect("arn"),
            DatabaseName::parse("aex").expect("database"),
        );
        config.max_result_bytes = 64;
        config.max_field_bytes = 16;
        config
    }

    #[test]
    fn an_oversized_field_is_refused_before_decoding() {
        let response = ExecuteResponse {
            records: vec![vec![Field::StringValue("x".repeat(17))]],
            rows_affected: 0,
        };
        assert_eq!(
            guard(&response, &config()),
            Err(DataApiError::FieldTooLarge {
                index: 0,
                bytes: 17
            })
        );
    }

    #[test]
    fn an_oversized_result_is_refused_before_decoding() {
        let response = ExecuteResponse {
            records: (0..8)
                .map(|_| vec![Field::StringValue("x".repeat(16))])
                .collect(),
            rows_affected: 0,
        };
        assert_eq!(
            guard(&response, &config()),
            Err(DataApiError::ResultTooLarge { bytes: 80 })
        );
    }

    #[test]
    fn a_response_inside_both_budgets_passes() {
        let response = ExecuteResponse {
            records: vec![vec![Field::StringValue("x".repeat(16))]],
            rows_affected: 0,
        };
        assert_eq!(guard(&response, &config()), Ok(()));
    }

    #[test]
    fn an_indeterminate_transport_failure_classifies_as_unavailable() {
        let error = TransportError::Indeterminate {
            message: "connection reset".to_owned(),
        };
        assert_eq!(
            error.classify(),
            DataApiError::Unavailable {
                message: "connection reset".to_owned()
            }
        );
    }

    #[test]
    fn a_service_failure_classifies_through_the_error_table() {
        let error = TransportError::Service {
            kind: ExceptionKind::StatementTimeout,
            message: "timed out".to_owned(),
        };
        assert_eq!(error.classify(), DataApiError::Timeout);
    }

    #[test]
    fn only_a_definitive_abort_is_a_rollback() {
        assert!(definitively_aborted(&DataApiError::Serialization));
        assert!(definitively_aborted(&DataApiError::UniqueViolation {
            constraint: "x".to_owned()
        }));
        assert!(definitively_aborted(
            &DataApiError::IntegrityConstraintViolation {
                message: "organization has no active owner".to_owned()
            }
        ));
        assert!(!definitively_aborted(&DataApiError::Unavailable {
            message: "reset".to_owned()
        }));
        assert!(!definitively_aborted(&DataApiError::TransactionNotFound));
        assert!(!definitively_aborted(&DataApiError::Timeout));
        assert!(!definitively_aborted(&DataApiError::Throttled));
    }
}
