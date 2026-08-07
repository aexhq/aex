//! Finance-specific Aurora statement contract.

/// Every write begins at serializable isolation.
pub const BEGIN_SERIALIZABLE: &str = "SET TRANSACTION ISOLATION LEVEL SERIALIZABLE";

/// Journal header insert; every value is a typed Data API parameter.
pub const INSERT_TRANSACTION: &str = r"
INSERT INTO finance.journal_transaction
  (transaction_id, org_id, kind, business_key, intent_hash, posting_count, occurred_at)
VALUES
  (:transaction_id, :org_id, :kind, :business_key, :intent_hash, :posting_count, :occurred_at)
ON CONFLICT (business_key) DO NOTHING
RETURNING transaction_id, business_key, intent_hash
";

/// Posting insert. Projection and postings execute in the same transaction.
pub const INSERT_POSTING: &str = r"
INSERT INTO finance.journal_posting
  (transaction_id, posting_seq, account_id, currency, amount_microusd)
VALUES
  (:transaction_id, :posting_seq, :account_id, 'USD', :amount_microusd)
";

/// Projection update protected by the database prepaid fence.
pub const APPLY_PROJECTION: &str = r"
UPDATE finance.account_balance
SET balance_microusd = balance_microusd + :amount_microusd,
    posting_count = posting_count + 1,
    last_transaction_id = :transaction_id,
    updated_at = :updated_at
WHERE account_id = :account_id
";

/// Immutable adapter configuration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FinanceDbConfig {
    /// Aurora cluster ARN.
    pub cluster_arn: String,
    /// DDL-independent application secret ARN.
    pub secret_arn: String,
    /// Database name.
    pub database: String,
    /// Transaction deadline in milliseconds.
    pub transaction_deadline_ms: u64,
}

impl FinanceDbConfig {
    /// Validates that no resource identifier is defaulted or blank.
    ///
    /// # Errors
    /// Rejects an empty identity or zero deadline.
    pub fn validate(&self) -> Result<(), FinanceAuroraConfigError> {
        if self.cluster_arn.trim().is_empty()
            || self.secret_arn.trim().is_empty()
            || self.database.trim().is_empty()
        {
            return Err(FinanceAuroraConfigError::MissingResourceIdentity);
        }
        if self.transaction_deadline_ms == 0 {
            return Err(FinanceAuroraConfigError::ZeroDeadline);
        }
        Ok(())
    }
}

/// Invalid adapter configuration.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum FinanceAuroraConfigError {
    /// Required resource identity was empty.
    #[error("finance database resource identity is missing")]
    MissingResourceIdentity,
    /// Deadline was zero.
    #[error("finance database transaction deadline must be positive")]
    ZeroDeadline,
}
