//! The receipt outbox and its replay to the regional frontiers.
//!
//! Receipts are immutable, so replay is free: the dispatcher keeps publishing
//! until the regional frontier converges, and central truth never waits on a
//! regional acknowledgement. The only column this deployable may move is
//! `dispatch_state`, which is exactly what its grants allow.

use std::sync::Arc;

use aex_rds_data::{DataApiClient, DataApiError, DecodeError, Record, Row, SqlValue, Statement};

/// Every statement this deployable runs.
pub mod sql {
    /// Proves the connection can move a receipt and nothing else.
    pub const PROBE_ROLE: &str = "\
SELECT pg_has_role(current_user, :role, 'MEMBER'), \
       has_table_privilege(:role, 'finance.receipt_outbox', 'UPDATE'), \
       has_table_privilege(:role, 'finance.journal_transaction', 'INSERT')";

    /// One page of pending receipts for one region and category.
    pub const PENDING_RECEIPTS: &str = "\
SELECT r.receipt_id, r.region, r.category, r.fact_id, r.payload::text, r.attempts \
  FROM finance.receipt_outbox r \
 WHERE r.category = :category AND r.dispatch_state = 'pending' \
 ORDER BY r.created_at, r.receipt_id \
 LIMIT :page_limit";

    /// Marks one receipt dispatched after the regional queue accepted it.
    pub const MARK_DISPATCHED: &str = "\
UPDATE finance.receipt_outbox \
   SET dispatch_state = 'dispatched', attempts = attempts + 1 \
 WHERE receipt_id = :receipt_id AND dispatch_state = 'pending'";

    /// Counts an attempt that did not reach the regional queue.
    pub const COUNT_ATTEMPT: &str = "\
UPDATE finance.receipt_outbox SET attempts = attempts + 1 \
 WHERE receipt_id = :receipt_id AND dispatch_state = 'pending'";
}

/// One immutable settlement receipt, ready to publish.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingReceipt {
    /// The receipt identity, which is also its deduplication identity.
    pub receipt_id: uuid::Uuid,
    /// The region the fact came from.
    pub region: String,
    /// The rating category.
    pub category: String,
    /// The regional fact identity.
    pub fact_id: String,
    /// The published body, exactly as settlement wrote it.
    pub payload: String,
    /// How many attempts this receipt has already taken.
    pub attempts: i64,
}

impl PendingReceipt {
    /// The fold identity for this receipt.
    ///
    /// Identical to the identity the settlement inbox folds on, so a replayed
    /// receipt is the same receipt rather than a second one. This is *not* an
    /// SQS `MessageDeduplicationId`: the receipt queues are standard, which
    /// rejects that parameter, so deduplication is the consumer's idempotent
    /// fold rather than the transport's. Carried on the publish failure path so
    /// a rejected receipt names itself.
    #[must_use]
    pub fn deduplication_id(&self) -> String {
        format!("{}:{}:{}", self.region, self.category, self.fact_id)
    }
}

/// Why the outbox could not be replayed.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum DispatchError {
    /// The authority is unreachable.
    #[error("the finance authority is unavailable: {0}")]
    Unavailable(String),
    /// The regional queue is unreachable.
    #[error("the regional receipt queue is unavailable: {0}")]
    QueueUnavailable(String),
    /// The stored row does not match what this deployable projects.
    #[error("the finance authority returned an undecodable row: {0}")]
    Decode(String),
}

/// Reading and retiring receipts.
#[async_trait::async_trait]
pub trait ReceiptOutbox: Send + Sync + 'static {
    /// Proves this deployable can reach the database as its own role.
    async fn probe_role(&self) -> Result<(), DispatchError>;

    /// One page of pending receipts for `category`.
    async fn pending(
        &self,
        category: &str,
        page_limit: u32,
    ) -> Result<Vec<PendingReceipt>, DispatchError>;

    /// Marks one receipt dispatched.
    async fn mark_dispatched(&self, receipt_id: uuid::Uuid) -> Result<(), DispatchError>;

    /// Counts an attempt that did not reach the regional queue.
    async fn count_attempt(&self, receipt_id: uuid::Uuid) -> Result<(), DispatchError>;
}

/// Publishing a receipt to its regional queue.
#[async_trait::async_trait]
pub trait ReceiptPublisher: Send + Sync + 'static {
    /// Publishes one receipt. A failure leaves the receipt pending.
    async fn publish(&self, receipt: &PendingReceipt) -> Result<(), DispatchError>;
}

/// The Aurora-backed outbox.
#[derive(Debug, Clone)]
pub struct AuroraReceiptOutbox {
    client: Arc<DataApiClient>,
    role: String,
}

impl AuroraReceiptOutbox {
    /// Builds the outbox over a configured Data `API` client.
    #[must_use]
    pub fn new(client: Arc<DataApiClient>, role: String) -> Self {
        Self { client, role }
    }
}

/// The three booleans the readiness probe checks.
#[derive(Debug)]
struct GrantRow {
    holds_role: bool,
    can_retire_receipt: bool,
    can_post_money: bool,
}

impl Row for GrantRow {
    fn from_record(record: &Record<'_>) -> Result<Self, DecodeError> {
        record.expect_arity(3)?;
        Ok(Self {
            holds_role: record.bool(0)?,
            can_retire_receipt: record.bool(1)?,
            can_post_money: record.bool(2)?,
        })
    }
}

impl Row for PendingReceipt {
    fn from_record(record: &Record<'_>) -> Result<Self, DecodeError> {
        record.expect_arity(6)?;
        Ok(Self {
            receipt_id: record.uuid(0)?,
            region: record.text(1)?.to_owned(),
            category: record.text(2)?.to_owned(),
            fact_id: record.text(3)?.to_owned(),
            payload: record.text(4)?.to_owned(),
            attempts: record.i64(5)?,
        })
    }
}

/// Classifies a transport failure.
fn store(error: DataApiError) -> DispatchError {
    match error {
        DataApiError::Decode(decode) => DispatchError::Decode(decode.to_string()),
        other => DispatchError::Unavailable(other.to_string()),
    }
}

#[async_trait::async_trait]
impl ReceiptOutbox for AuroraReceiptOutbox {
    async fn probe_role(&self) -> Result<(), DispatchError> {
        let row: GrantRow = self
            .client
            .query_one(Statement::with(
                sql::PROBE_ROLE,
                vec![("role", SqlValue::Text(self.role.clone()))],
            ))
            .await
            .map_err(store)?;
        if !row.holds_role || !row.can_retire_receipt {
            return Err(DispatchError::Unavailable(format!(
                "`{}` cannot retire a receipt",
                self.role
            )));
        }
        if row.can_post_money {
            return Err(DispatchError::Unavailable(format!(
                "`{}` can write the journal; a dispatcher may neither create nor alter a \
                 settlement",
                self.role
            )));
        }
        Ok(())
    }

    async fn pending(
        &self,
        category: &str,
        page_limit: u32,
    ) -> Result<Vec<PendingReceipt>, DispatchError> {
        self.client
            .query(Statement::with(
                sql::PENDING_RECEIPTS,
                vec![
                    ("category", SqlValue::Text(category.to_owned())),
                    ("page_limit", SqlValue::I64(i64::from(page_limit))),
                ],
            ))
            .await
            .map_err(store)
    }

    async fn mark_dispatched(&self, receipt_id: uuid::Uuid) -> Result<(), DispatchError> {
        self.client
            .execute(Statement::with(
                sql::MARK_DISPATCHED,
                vec![("receipt_id", SqlValue::Uuid(receipt_id))],
            ))
            .await
            .map(|_| ())
            .map_err(store)
    }

    async fn count_attempt(&self, receipt_id: uuid::Uuid) -> Result<(), DispatchError> {
        self.client
            .execute(Statement::with(
                sql::COUNT_ATTEMPT,
                vec![("receipt_id", SqlValue::Uuid(receipt_id))],
            ))
            .await
            .map(|_| ())
            .map_err(store)
    }
}

/// The SQS implementation of the publisher.
#[derive(Debug, Clone)]
pub struct SqsReceiptPublisher {
    client: aws_sdk_sqs::Client,
    queue_url_by_category: std::collections::BTreeMap<String, String>,
}

impl SqsReceiptPublisher {
    /// Builds the publisher over the bound category queues.
    #[must_use]
    pub const fn new(
        client: aws_sdk_sqs::Client,
        queue_url_by_category: std::collections::BTreeMap<String, String>,
    ) -> Self {
        Self {
            client,
            queue_url_by_category,
        }
    }
}

#[async_trait::async_trait]
impl ReceiptPublisher for SqsReceiptPublisher {
    async fn publish(&self, receipt: &PendingReceipt) -> Result<(), DispatchError> {
        let queue = self
            .queue_url_by_category
            .get(&receipt.category)
            .ok_or_else(|| {
                DispatchError::QueueUnavailable(format!(
                    "no queue is bound for category `{}`",
                    receipt.category
                ))
            })?;
        // The three `usage-receipt-*` queues are standard, not FIFO, and SQS
        // rejects `MessageDeduplicationId` and `MessageGroupId` on a standard
        // queue with `InvalidParameterValue`. Setting them here made every
        // publish fail on first contact with a real plane; no plane had ever run
        // this unit, so nothing caught it.
        //
        // Standard is the right queue: the consumer's own contract is that
        // receipts have no ordering guarantee and one failing message fails only
        // itself. FIFO would also serialize the whole lane, because the group id
        // was the region and a plane is one region.
        //
        // Exactly-once still holds, and not by SQS: the settlement fold is
        // idempotent on the identity `deduplication_id()` renders, so a
        // redelivered receipt folds to `AlreadySettled`. That is durable, unlike
        // FIFO's five-minute dedup window.
        self.client
            .send_message()
            .queue_url(queue)
            .message_body(&receipt.payload)
            .send()
            .await
            .map(|_| ())
            .map_err(|error| {
                DispatchError::QueueUnavailable(format!(
                    "receipt `{}` could not be published: {}",
                    receipt.deduplication_id(),
                    aws_sdk_sqs::error::DisplayErrorContext(&error)
                ))
            })
    }
}

#[cfg(test)]
mod tests {
    use super::{PendingReceipt, sql};

    fn receipt() -> PendingReceipt {
        PendingReceipt {
            receipt_id: uuid::Uuid::from_u128(1),
            region: "eu-west-1".to_owned(),
            category: "compute".to_owned(),
            fact_id: "usage_abc".to_owned(),
            payload: "{}".to_owned(),
            attempts: 0,
        }
    }

    #[test]
    fn every_statement_binds_and_never_touches_floating_point() {
        for statement in [
            sql::PROBE_ROLE,
            sql::PENDING_RECEIPTS,
            sql::MARK_DISPATCHED,
            sql::COUNT_ATTEMPT,
        ] {
            assert!(statement.contains(':'), "{statement} binds no parameter");
            assert!(!statement.to_lowercase().contains("float"));
            assert!(!statement.to_lowercase().contains("double"));
        }
    }

    #[test]
    fn the_dispatcher_touches_only_the_outbox() {
        for statement in [sql::MARK_DISPATCHED, sql::COUNT_ATTEMPT] {
            assert!(
                statement.contains("finance.receipt_outbox"),
                "the dispatcher may move a receipt and nothing else"
            );
            assert!(!statement.contains("finance.journal"));
            assert!(!statement.contains("finance.account_balance"));
        }
    }

    #[test]
    fn a_receipt_is_only_retired_from_pending() {
        assert!(sql::MARK_DISPATCHED.contains("dispatch_state = 'pending'"));
        assert!(sql::COUNT_ATTEMPT.contains("dispatch_state = 'pending'"));
    }

    #[test]
    fn a_replayed_receipt_carries_the_identity_the_regional_frontier_folds_on() {
        assert_eq!(receipt().deduplication_id(), "eu-west-1:compute:usage_abc");
    }

    #[test]
    fn the_outbox_page_is_ordered_so_replay_is_deterministic() {
        assert!(sql::PENDING_RECEIPTS.contains("ORDER BY r.created_at, r.receipt_id"));
        assert!(sql::PENDING_RECEIPTS.contains("LIMIT :page_limit"));
    }
}
