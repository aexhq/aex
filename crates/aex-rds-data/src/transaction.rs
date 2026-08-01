//! Transaction lifecycle and the commit-ambiguity rule.
//!
//! Every in-transaction call takes `&mut self`, which makes serial use a
//! compile-time property rather than a convention — the Data `API` rejects
//! concurrent use of one transaction id, and a `&mut` borrow is the cheapest way
//! to make that unrepresentable.
//!
//! [`Transaction::commit`] consumes `self` and answers with [`CommitFailure`].
//! A lost response is always [`CommitFailure::Unknown`]. This crate never
//! retries a commit and never regenerates an identity; reconciliation is the
//! application's job because only the application knows what identity it
//! preassigned.

use std::time::{Duration, Instant};

use crate::client::DataApiClient;
use crate::error::DataApiError;
use crate::params::Statement;
use crate::record::{Record, Row};

/// The Data `API` transaction identifier.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct TransactionId(String);

impl TransactionId {
    /// Wraps an identifier returned by the service.
    #[must_use]
    pub fn new(raw: impl Into<String>) -> Self {
        Self(raw.into())
    }

    /// The identifier as a string slice.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// The isolation level a transaction runs at.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum Isolation {
    /// `PostgreSQL`'s default.
    #[default]
    ReadCommitted,
    /// Full serializability, for the transitions whose guard is a read.
    Serializable,
}

impl Isolation {
    /// The statement to run first, when the level is not the default.
    #[must_use]
    pub const fn statement(self) -> Option<&'static str> {
        match self {
            Self::ReadCommitted => None,
            Self::Serializable => Some("SET TRANSACTION ISOLATION LEVEL SERIALIZABLE"),
        }
    }
}

/// A commit that the service confirmed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Committed;

/// Why a commit did not confirm.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CommitFailure {
    /// The service stated the transaction aborted. Nothing was applied.
    #[error("the transaction was rolled back: {0}")]
    RolledBack(DataApiError),
    /// The outcome is not known. The transaction may have committed.
    ///
    /// This is the only correct answer to a lost commit response, and it is
    /// deliberately not convertible into either success or failure.
    #[error("the commit outcome is unknown: {0}")]
    Unknown(DataApiError),
}

/// The result of finishing a transaction.
pub type CommitOutcome = Result<Committed, CommitFailure>;

/// An open Data `API` transaction.
///
/// Neither `Clone` nor `Copy`: a transaction id is a resource with an exclusive
/// owner, and the type says so.
#[derive(Debug)]
pub struct Transaction<'a> {
    client: &'a DataApiClient,
    id: TransactionId,
    deadline: Instant,
    terminated: bool,
}

impl<'a> Transaction<'a> {
    /// Wraps a freshly opened transaction.
    pub(crate) const fn new(
        client: &'a DataApiClient,
        id: TransactionId,
        deadline: Instant,
    ) -> Self {
        Self {
            client,
            id,
            deadline,
            terminated: false,
        }
    }

    /// The transaction identifier.
    #[must_use]
    pub const fn id(&self) -> &TransactionId {
        &self.id
    }

    /// How long the client-side guard deadline still allows.
    #[must_use]
    pub fn remaining(&self) -> Duration {
        self.deadline.saturating_duration_since(Instant::now())
    }

    /// Fails when the guard deadline has already elapsed.
    fn check_deadline(&self) -> Result<(), DataApiError> {
        if self.remaining().is_zero() {
            return Err(DataApiError::DeadlineExceeded);
        }
        Ok(())
    }

    /// Runs a statement inside the transaction and decodes every record.
    ///
    /// # Errors
    ///
    /// Returns [`DataApiError::DeadlineExceeded`] when the guard deadline has
    /// elapsed, plus every failure the client reports.
    pub async fn query<R: Row>(
        &mut self,
        statement: Statement<'_>,
    ) -> Result<Vec<R>, DataApiError> {
        self.check_deadline()?;
        let response = self.client.run(statement, Some(&self.id)).await?;
        response
            .records
            .iter()
            .map(|fields| R::from_record(&Record::new(fields)).map_err(DataApiError::Decode))
            .collect()
    }

    /// Runs a statement inside the transaction expecting at most one record.
    ///
    /// # Errors
    ///
    /// Returns [`DataApiError::Decode`] with an arity mismatch when more than
    /// one record arrives, plus every failure [`Transaction::query`] reports.
    pub async fn query_opt<R: Row>(
        &mut self,
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

    /// Runs a statement inside the transaction expecting exactly one record.
    ///
    /// # Errors
    ///
    /// Returns [`DataApiError::Decode`] with an arity mismatch when the record
    /// count is not one, plus every failure [`Transaction::query`] reports.
    pub async fn query_one<R: Row>(&mut self, statement: Statement<'_>) -> Result<R, DataApiError> {
        self.query_opt::<R>(statement)
            .await?
            .ok_or(DataApiError::Decode(
                crate::error::DecodeError::ArityMismatch {
                    expected: 1,
                    actual: 0,
                },
            ))
    }

    /// Runs a DML statement inside the transaction.
    ///
    /// # Errors
    ///
    /// Returns [`DataApiError::DeadlineExceeded`] when the guard deadline has
    /// elapsed, plus every failure the client reports.
    pub async fn execute(&mut self, statement: Statement<'_>) -> Result<u64, DataApiError> {
        self.check_deadline()?;
        Ok(self
            .client
            .run(statement, Some(&self.id))
            .await?
            .rows_affected)
    }

    /// Commits, consuming the transaction.
    ///
    /// # Errors
    ///
    /// Returns [`CommitFailure::RolledBack`] only when the service stated the
    /// transaction aborted, and [`CommitFailure::Unknown`] for every other
    /// failure — including a timeout, a reset and an elapsed guard deadline.
    pub async fn commit(mut self) -> CommitOutcome {
        self.terminated = true;
        self.client.finish(&self.id).await
    }

    /// Rolls back, consuming the transaction.
    ///
    /// # Errors
    ///
    /// Returns [`DataApiError`] when the rollback itself failed. The transaction
    /// is still considered terminated: a failed rollback expires on its own.
    pub async fn rollback(mut self) -> Result<(), DataApiError> {
        self.terminated = true;
        self.client
            .transport()
            .rollback(&self.id)
            .await
            .map_err(|error| error.classify())
    }
}

impl Drop for Transaction<'_> {
    fn drop(&mut self) {
        if !self.terminated {
            tracing::error!(
                event = "transaction_leaked",
                "a Data API transaction was dropped without commit or rollback"
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{CommitFailure, Isolation, TransactionId};
    use crate::error::DataApiError;

    #[test]
    fn only_serializable_emits_an_isolation_statement() {
        assert_eq!(Isolation::ReadCommitted.statement(), None);
        assert_eq!(
            Isolation::Serializable.statement(),
            Some("SET TRANSACTION ISOLATION LEVEL SERIALIZABLE")
        );
        assert_eq!(Isolation::default(), Isolation::ReadCommitted);
    }

    #[test]
    fn a_transaction_id_is_carried_verbatim() {
        assert_eq!(TransactionId::new("abc").as_str(), "abc");
    }

    #[test]
    fn the_two_commit_failures_are_distinct_and_neither_is_success() {
        let unknown = CommitFailure::Unknown(DataApiError::Timeout);
        let rolled_back = CommitFailure::RolledBack(DataApiError::Serialization);
        assert_ne!(unknown, rolled_back);
        assert!(unknown.to_string().contains("unknown"));
        assert!(rolled_back.to_string().contains("rolled back"));
    }
}
