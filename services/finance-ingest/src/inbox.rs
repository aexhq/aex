//! The provider-event inbox and the one transaction that settles an event.
//!
//! `finance-ingest` returns success **only** after the inbox row and the money
//! transition it implies are both durable, so the webhook edge can return `2xx`
//! knowing the provider will not have to redeliver. Every failure path returns
//! an error, which the edge renders as `5xx` and Stripe redelivers.

use std::sync::Arc;

use aex_finance_domain::account::{AccountKind, AccountRef};
use aex_finance_domain::journal::{BusinessKey, IntentHash, TransactionId};
use aex_finance_domain::money::{MICROUSD_PER_CENT, Microusd};
use aex_payment_contracts::{ProviderEventEnvelope, ProviderEventFacts};
use aex_rds_data::{
    CommitFailure, DataApiClient, DataApiError, DecodeError, Isolation, Record, Row, SqlValue,
    Statement, Transaction,
};
use aex_wire::ids::OrganizationId;

use crate::journal::{
    JournalWrite, account_kind_sql, customer_account, dispute_opened, kind_sql, refund,
    top_up_settled,
};

/// Every statement this deployable runs.
pub mod sql {
    /// Proves the connection holds its own role and cannot mutate the journal.
    pub const PROBE_ROLE: &str = "\
SELECT pg_has_role(current_user, :role, 'MEMBER'), \
       has_table_privilege(:role, 'finance.journal_transaction', 'UPDATE'), \
       has_table_privilege(:role, 'finance.provider_event_inbox', 'INSERT')";

    /// Claims one provider event. The primary key absorbs a duplicate delivery.
    pub const CLAIM_EVENT: &str = "\
INSERT INTO finance.provider_event_inbox \
  (provider_event_id, event_type, object_id, org_id, provider_api_version, \
   created_at_provider, raw_body_sha256, schema_id, normalized, applied_state) \
VALUES \
  (:provider_event_id, :event_type, :object_id, :org_id, :provider_api_version, \
   (TIMESTAMPTZ 'epoch' + (:created_at_ms) * INTERVAL '1 millisecond'), \
   :raw_body_sha256, :schema_id, :normalized, :applied_state) \
ON CONFLICT (provider_event_id) DO NOTHING \
RETURNING provider_event_id";

    /// Reads back an event a duplicate delivery already applied.
    pub const READ_EVENT: &str = "\
SELECT applied_state, raw_body_sha256 FROM finance.provider_event_inbox \
 WHERE provider_event_id = :provider_event_id";

    /// Binds the settled transaction to the inbox row.
    pub const BIND_EVENT_TRANSACTION: &str = "\
UPDATE finance.provider_event_inbox SET transaction_id = :transaction_id \
 WHERE provider_event_id = :provider_event_id";

    /// Creates an organization's account on first use, converging on one row.
    pub const ENSURE_ACCOUNT: &str = "\
INSERT INTO finance.account (account_id, org_id, kind, normal_side) \
VALUES (:account_id, :org_id, :kind, :normal_side) \
ON CONFLICT (account_id) DO NOTHING";

    /// Creates the projection row the balance rules are attached to.
    pub const ENSURE_BALANCE: &str = "\
INSERT INTO finance.account_balance (account_id, org_id, kind, currency) \
SELECT a.account_id, a.org_id, a.kind, a.currency FROM finance.account a \
 WHERE a.account_id = :account_id \
ON CONFLICT (account_id) DO NOTHING";

    /// The header. The unique business key is the money-path fence.
    pub const INSERT_TRANSACTION: &str = "\
INSERT INTO finance.journal_transaction \
  (transaction_id, org_id, kind, business_key, intent_hash, posting_count, occurred_at) \
VALUES \
  (:transaction_id, :org_id, :kind, :business_key, :intent_hash, :posting_count, \
   (TIMESTAMPTZ 'epoch' + (:occurred_at_ms) * INTERVAL '1 millisecond')) \
ON CONFLICT (business_key) DO NOTHING \
RETURNING transaction_id, intent_hash";

    /// Reads back the transaction a replayed business key already committed.
    pub const READ_TRANSACTION: &str = "\
SELECT transaction_id, intent_hash FROM finance.journal_transaction \
 WHERE business_key = :business_key";

    /// One posting.
    pub const INSERT_POSTING: &str = "\
INSERT INTO finance.journal_posting \
  (transaction_id, posting_seq, account_id, currency, amount_microusd) \
VALUES (:transaction_id, :posting_seq, :account_id, 'USD', :amount_microusd)";

    /// The projection, written in the same transaction as its postings.
    ///
    /// A deduction that takes `customer_available` past zero commits. Since
    /// `20260801001300_finance_customer_overdraw` the prepaid CHECK covers only
    /// `customer_reserved`, so the overdraw lands, and landing it is what fires
    /// `account_balance_credit_exhaustion` and pauses the account. The earlier
    /// behaviour — abort the whole transaction — discarded the usage and left
    /// the account running, which is the opposite of stopping it.
    pub const APPLY_PROJECTION: &str = "\
UPDATE finance.account_balance \
   SET balance_microusd = balance_microusd + :amount_microusd, \
       posting_count = posting_count + 1, \
       last_transaction_id = :transaction_id, \
       updated_at = now() \
 WHERE account_id = :account_id";

    /// The spendable credit of one organization, for the dispute claw-back.
    pub const READ_AVAILABLE: &str = "\
SELECT coalesce(-b.balance_microusd, 0)::bigint FROM finance.account_balance b \
 WHERE b.account_id = :account_id";
}

/// What the ingest of one event settled on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AppliedState {
    /// The event moved money or state.
    Applied,
    /// The event is validly signed but AEX takes no action on it.
    IgnoredUnsupported,
    /// The event contradicts durable state or arrived under a wrong pin.
    Quarantined,
}

impl AppliedState {
    /// The durable spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Applied => "applied",
            Self::IgnoredUnsupported => "ignored_unsupported",
            Self::Quarantined => "quarantined",
        }
    }
}

/// What one ingest call answered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IngestOutcome {
    /// How the event was settled.
    pub applied: AppliedState,
    /// Whether this call is the one that made it durable.
    pub first_delivery: bool,
    /// The journal transaction it posted, when it posted one.
    pub transaction_id: Option<uuid::Uuid>,
}

/// Why an event could not be made durable.
///
/// Every variant is returned to the caller so the edge answers non-`2xx` and
/// the provider redelivers. There is no arm that reports success on a failure.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum IngestError {
    /// The event declared a provider API version this build does not parse.
    #[error("event `{event}` declares provider API version `{found}`, not the pinned `{pinned}`")]
    ApiVersionMismatch {
        /// The event identity.
        event: String,
        /// What the event declared.
        found: String,
        /// What this build pins.
        pinned: String,
    },
    /// The same event identity is already stored with different bytes.
    #[error("event `{0}` is stored with a different raw digest")]
    DigestConflict(String),
    /// The same business key is committed under a different intent.
    #[error("business key `{0}` is committed with a different intent")]
    IntentConflict(String),
    /// The transition could not be built as a balanced transaction.
    #[error("the transition does not conserve: {0}")]
    Conservation(String),
    /// The authority is unreachable. Nothing was applied.
    #[error("the finance authority is unavailable: {0}")]
    Unavailable(String),
    /// The commit response was lost. The transition may or may not be durable.
    #[error("the finance commit outcome is unknown: {0}")]
    OutcomeUnknown(String),
    /// The stored row does not match what this deployable projects.
    #[error("the finance authority returned an undecodable row: {0}")]
    Decode(String),
}

/// Ingesting one provider event.
#[async_trait::async_trait]
pub trait ProviderEventInbox: Send + Sync + 'static {
    /// Proves this deployable can reach the database as its own role.
    async fn probe_role(&self) -> Result<(), IngestError>;

    /// Commits the inbox row and its money transition in one transaction.
    async fn ingest(&self, event: &ProviderEventEnvelope) -> Result<IngestOutcome, IngestError>;
}

/// The Aurora-backed inbox.
#[derive(Debug, Clone)]
pub struct AuroraProviderEventInbox {
    client: Arc<DataApiClient>,
    role: String,
    pinned_api_version: String,
}

impl AuroraProviderEventInbox {
    /// Builds the inbox over a configured Data `API` client.
    #[must_use]
    pub fn new(client: Arc<DataApiClient>, role: String, pinned_api_version: String) -> Self {
        Self {
            client,
            role,
            pinned_api_version,
        }
    }

    /// The provider API version this build accepts.
    #[must_use]
    pub fn pinned_api_version(&self) -> &str {
        &self.pinned_api_version
    }
}

/// The three booleans the readiness probe checks.
#[derive(Debug)]
struct GrantRow {
    holds_role: bool,
    can_mutate_journal: bool,
    can_append_inbox: bool,
}

impl Row for GrantRow {
    fn from_record(record: &Record<'_>) -> Result<Self, DecodeError> {
        record.expect_arity(3)?;
        Ok(Self {
            holds_role: record.bool(0)?,
            can_mutate_journal: record.bool(1)?,
            can_append_inbox: record.bool(2)?,
        })
    }
}

/// A stored inbox row.
#[derive(Debug)]
struct StoredEvent {
    applied_state: String,
    raw_digest: [u8; 32],
}

impl Row for StoredEvent {
    fn from_record(record: &Record<'_>) -> Result<Self, DecodeError> {
        record.expect_arity(2)?;
        Ok(Self {
            applied_state: record.text(0)?.to_owned(),
            raw_digest: record.fixed::<32>(1)?,
        })
    }
}

/// A stored journal header.
#[derive(Debug)]
struct StoredTransaction {
    transaction_id: uuid::Uuid,
    intent_hash: [u8; 32],
}

impl Row for StoredTransaction {
    fn from_record(record: &Record<'_>) -> Result<Self, DecodeError> {
        record.expect_arity(2)?;
        Ok(Self {
            transaction_id: record.uuid(0)?,
            intent_hash: record.fixed::<32>(1)?,
        })
    }
}

/// A single `bigint` column.
#[derive(Debug)]
struct AmountRow(i64);

impl Row for AmountRow {
    fn from_record(record: &Record<'_>) -> Result<Self, DecodeError> {
        record.expect_arity(1)?;
        Ok(Self(record.i64(0)?))
    }
}

/// Classifies a transport failure without inventing an outcome.
fn store(error: DataApiError) -> IngestError {
    match error {
        DataApiError::Decode(decode) => IngestError::Decode(decode.to_string()),
        other => IngestError::Unavailable(other.to_string()),
    }
}

/// Classifies a commit failure. Only an explicit rollback is a clean failure.
fn commit(failure: CommitFailure) -> IngestError {
    match failure {
        CommitFailure::RolledBack(error) => IngestError::Unavailable(error.to_string()),
        CommitFailure::Unknown(error) => IngestError::OutcomeUnknown(error.to_string()),
    }
}

/// The organization identity as the `uuid` column stores it.
fn org_uuid(organization: OrganizationId) -> uuid::Uuid {
    use aex_wire::PrefixedId as _;
    uuid::Uuid::from_bytes(*organization.uuid7().as_bytes())
}

/// Widens whole cents to micro-USD, refusing an amount outside the bound.
fn to_microusd(cents: aex_wire::types::Cents) -> Result<Microusd, IngestError> {
    i64::try_from(cents.get())
        .ok()
        .and_then(|value| value.checked_mul(MICROUSD_PER_CENT))
        .and_then(|value| Microusd::new(value).ok())
        .ok_or_else(|| {
            IngestError::Conservation(format!(
                "{} cents is outside the permitted money bound",
                cents.get()
            ))
        })
}

/// The business key for one event's transition.
fn business_key(event: &ProviderEventEnvelope) -> Result<BusinessKey, IngestError> {
    let raw = match &event.facts {
        ProviderEventFacts::PaymentIntentSucceeded { intent, .. } => {
            format!("topup:{}", intent.0.to_lowercase())
        }
        ProviderEventFacts::ChargeDisputeCreated { charge, .. } => {
            format!("dispute:{}:open", charge.0.to_lowercase())
        }
        ProviderEventFacts::RefundCreated { refund, .. } => {
            format!("refund:{}", refund.0.to_lowercase())
        }
        ProviderEventFacts::RefundFailed { refund, .. } => {
            format!("rev:refund:{}", refund.0.to_lowercase())
        }
        ProviderEventFacts::PaymentIntentPaymentFailed { intent, .. } => {
            format!("intentfailed:{}", intent.0.to_lowercase())
        }
    };
    BusinessKey::parse(&raw).map_err(|error| {
        IngestError::Conservation(format!("`{raw}` is not a business key: {error}"))
    })
}

#[async_trait::async_trait]
impl ProviderEventInbox for AuroraProviderEventInbox {
    async fn probe_role(&self) -> Result<(), IngestError> {
        let row: GrantRow = self
            .client
            .query_one(Statement::with(
                sql::PROBE_ROLE,
                vec![("role", SqlValue::Text(self.role.clone()))],
            ))
            .await
            .map_err(store)?;
        if !row.holds_role || !row.can_append_inbox {
            return Err(IngestError::Unavailable(format!(
                "`{}` cannot append to the provider event inbox",
                self.role
            )));
        }
        if row.can_mutate_journal {
            return Err(IngestError::Unavailable(format!(
                "`{}` holds UPDATE on finance.journal_transaction; history is append-only",
                self.role
            )));
        }
        Ok(())
    }

    async fn ingest(&self, event: &ProviderEventEnvelope) -> Result<IngestOutcome, IngestError> {
        if event.provider_api_version.0 != self.pinned_api_version {
            // A wrong pin is configuration drift, not a droppable delivery: the
            // event is stored quarantined so it can be replayed after the pin is
            // corrected, and it never reaches a money transition.
            return self.quarantine(event).await;
        }
        let organization = event.facts.organization();
        let key = business_key(event)?;

        let mut transaction = self
            .client
            .begin(Isolation::Serializable)
            .await
            .map_err(store)?;
        let outcome = self
            .apply(&mut transaction, event, organization, &key)
            .await;
        match outcome {
            Ok(outcome) => {
                transaction.commit().await.map_err(commit)?;
                Ok(outcome)
            }
            Err(error) => {
                let _ = transaction.rollback().await;
                Err(error)
            }
        }
    }
}

impl AuroraProviderEventInbox {
    /// Stores an event this build refuses to interpret, without a transition.
    async fn quarantine(
        &self,
        event: &ProviderEventEnvelope,
    ) -> Result<IngestOutcome, IngestError> {
        let mut transaction = self
            .client
            .begin(Isolation::Serializable)
            .await
            .map_err(store)?;
        let claim = claim_event(&mut transaction, event, AppliedState::Quarantined).await;
        match claim {
            Ok(_) => {
                transaction.commit().await.map_err(commit)?;
                Err(IngestError::ApiVersionMismatch {
                    event: event.provider_event_id.0.clone(),
                    found: event.provider_api_version.0.clone(),
                    pinned: self.pinned_api_version.clone(),
                })
            }
            Err(error) => {
                let _ = transaction.rollback().await;
                Err(error)
            }
        }
    }

    /// Everything inside the one serializable transaction.
    async fn apply(
        &self,
        transaction: &mut Transaction<'_>,
        event: &ProviderEventEnvelope,
        organization: OrganizationId,
        key: &BusinessKey,
    ) -> Result<IngestOutcome, IngestError> {
        let intended = intended_state(&event.facts);
        let claimed = claim_event(transaction, event, intended).await?;
        if !claimed {
            // A duplicate delivery: the stored row is the answer, and no second
            // posting happens.
            let stored: StoredEvent = transaction
                .query_one(Statement::with(
                    sql::READ_EVENT,
                    vec![(
                        "provider_event_id",
                        SqlValue::Text(event.provider_event_id.0.clone()),
                    )],
                ))
                .await
                .map_err(store)?;
            if stored.raw_digest != *event.raw_digest.as_bytes() {
                return Err(IngestError::DigestConflict(
                    event.provider_event_id.0.clone(),
                ));
            }
            return Ok(IngestOutcome {
                applied: parse_applied(&stored.applied_state)?,
                first_delivery: false,
                transaction_id: None,
            });
        }
        if intended != AppliedState::Applied {
            return Ok(IngestOutcome {
                applied: intended,
                first_delivery: true,
                transaction_id: None,
            });
        }

        let write = self.build(transaction, event, organization, key).await?;
        let posted = post(transaction, &write).await?;
        transaction
            .execute(Statement::with(
                sql::BIND_EVENT_TRANSACTION,
                vec![
                    ("transaction_id", SqlValue::Uuid(posted)),
                    (
                        "provider_event_id",
                        SqlValue::Text(event.provider_event_id.0.clone()),
                    ),
                ],
            ))
            .await
            .map_err(store)?;
        Ok(IngestOutcome {
            applied: AppliedState::Applied,
            first_delivery: true,
            transaction_id: Some(posted),
        })
    }

    /// Builds the balanced transition one event implies.
    async fn build(
        &self,
        transaction: &mut Transaction<'_>,
        event: &ProviderEventEnvelope,
        organization: OrganizationId,
        key: &BusinessKey,
    ) -> Result<JournalWrite, IngestError> {
        let id = TransactionId::from_uuid(uuid::Uuid::now_v7());
        let intent = IntentHash::new(*event.raw_digest.as_bytes());
        let occurred_at = time::OffsetDateTime::from_unix_timestamp_nanos(
            i128::from(event.occurred_at.unix_millis()) * 1_000_000,
        )
        .map_err(|error| IngestError::Decode(error.to_string()))?;
        let conservation = |error: aex_finance_domain::journal::ConservationError| {
            IngestError::Conservation(error.to_string())
        };
        match &event.facts {
            ProviderEventFacts::PaymentIntentSucceeded { credit, .. } => top_up_settled(
                id,
                organization,
                key.clone(),
                intent,
                occurred_at,
                to_microusd(*credit)?,
                Microusd::ZERO,
            )
            .map_err(conservation),
            ProviderEventFacts::RefundCreated { amount, .. }
            | ProviderEventFacts::RefundFailed { amount, .. } => refund(
                id,
                organization,
                key.clone(),
                intent,
                occurred_at,
                to_microusd(*amount)?,
                Microusd::ZERO,
            )
            .map_err(conservation),
            ProviderEventFacts::ChargeDisputeCreated { amount, .. } => {
                let account = customer_account(organization, AccountKind::CustomerAvailable);
                let available: Option<AmountRow> = transaction
                    .query_opt(Statement::with(
                        sql::READ_AVAILABLE,
                        vec![("account_id", SqlValue::Uuid(account.id()))],
                    ))
                    .await
                    .map_err(store)?;
                let available = Microusd::new(available.map_or(0, |row| row.0.max(0)))
                    .map_err(|error| IngestError::Decode(error.to_string()))?;
                dispute_opened(
                    id,
                    organization,
                    key.clone(),
                    intent,
                    occurred_at,
                    to_microusd(*amount)?,
                    available,
                )
                .map_err(conservation)
            }
            ProviderEventFacts::PaymentIntentPaymentFailed { .. } => Err(
                IngestError::Conservation("a failed payment intent moves no money".to_owned()),
            ),
        }
    }
}

/// Which durable state an event's facts imply.
const fn intended_state(facts: &ProviderEventFacts) -> AppliedState {
    match facts {
        ProviderEventFacts::PaymentIntentSucceeded { .. }
        | ProviderEventFacts::ChargeDisputeCreated { .. }
        | ProviderEventFacts::RefundCreated { .. }
        | ProviderEventFacts::RefundFailed { .. } => AppliedState::Applied,
        // A failed intent grants nothing and reverses nothing: it is recorded so
        // the reconciler can see it, and it posts no transaction.
        ProviderEventFacts::PaymentIntentPaymentFailed { .. } => AppliedState::IgnoredUnsupported,
    }
}

/// Reads a stored applied state.
fn parse_applied(raw: &str) -> Result<AppliedState, IngestError> {
    match raw {
        "applied" => Ok(AppliedState::Applied),
        "ignored_unsupported" => Ok(AppliedState::IgnoredUnsupported),
        "quarantined" => Ok(AppliedState::Quarantined),
        other => Err(IngestError::Decode(format!(
            "unknown applied state `{other}`"
        ))),
    }
}

/// Claims one event, answering whether this call is the first delivery.
async fn claim_event(
    transaction: &mut Transaction<'_>,
    event: &ProviderEventEnvelope,
    applied: AppliedState,
) -> Result<bool, IngestError> {
    let claimed: Vec<ClaimedRow> = transaction
        .query(Statement::with(
            sql::CLAIM_EVENT,
            vec![
                (
                    "provider_event_id",
                    SqlValue::Text(event.provider_event_id.0.clone()),
                ),
                ("event_type", SqlValue::Text(event_type(event).to_owned())),
                ("object_id", SqlValue::Text(event.object.0.clone())),
                (
                    "org_id",
                    SqlValue::Uuid(org_uuid(event.facts.organization())),
                ),
                (
                    "provider_api_version",
                    SqlValue::Text(event.provider_api_version.0.clone()),
                ),
                (
                    "created_at_ms",
                    SqlValue::I64(event.occurred_at.unix_millis()),
                ),
                (
                    "raw_body_sha256",
                    SqlValue::Bytes(event.raw_digest.as_bytes().to_vec()),
                ),
                (
                    "schema_id",
                    SqlValue::Text(format!(
                        "aex-payment-contracts/ProviderEventEnvelope/v{}",
                        event.schema_version.0
                    )),
                ),
                (
                    "normalized",
                    SqlValue::Json(serde_json::to_value(&event.facts).map_err(|error| {
                        IngestError::Decode(format!("the event facts do not serialize: {error}"))
                    })?),
                ),
                ("applied_state", SqlValue::Text(applied.as_str().to_owned())),
            ],
        ))
        .await
        .map_err(store)?;
    Ok(!claimed.is_empty())
}

/// The durable event-type spelling.
const fn event_type(event: &ProviderEventEnvelope) -> &'static str {
    use aex_payment_contracts::ProviderEventKind as Kind;
    match event.kind {
        Kind::PaymentIntentSucceeded => "payment_intent.succeeded",
        Kind::PaymentIntentPaymentFailed => "payment_intent.payment_failed",
        Kind::ChargeDisputeCreated => "charge.dispute.created",
        Kind::RefundCreated => "refund.created",
        Kind::RefundFailed => "refund.failed",
    }
}

/// Writes the header, its postings and the projection, in that order.
async fn post(
    transaction: &mut Transaction<'_>,
    write: &JournalWrite,
) -> Result<uuid::Uuid, IngestError> {
    for posting in &write.postings {
        if posting.account.kind().is_customer() {
            ensure_account(transaction, write.organization, posting.account).await?;
        }
    }
    let header: Vec<StoredTransaction> = transaction
        .query(Statement::with(
            sql::INSERT_TRANSACTION,
            vec![
                ("transaction_id", SqlValue::Uuid(write.id.as_uuid())),
                (
                    "org_id",
                    write
                        .organization
                        .map_or(SqlValue::Null, |value| SqlValue::Uuid(org_uuid(value))),
                ),
                ("kind", SqlValue::Text(kind_sql(write.kind).to_owned())),
                (
                    "business_key",
                    SqlValue::Text(write.business_key.as_str().to_owned()),
                ),
                (
                    "intent_hash",
                    SqlValue::Bytes(write.intent_hash.as_bytes().to_vec()),
                ),
                ("posting_count", SqlValue::I64(write.posting_count())),
                (
                    "occurred_at_ms",
                    SqlValue::I64(
                        i64::try_from(
                            write
                                .occurred_at
                                .unix_timestamp_nanos()
                                .div_euclid(1_000_000),
                        )
                        .unwrap_or(0),
                    ),
                ),
            ],
        ))
        .await
        .map_err(store)?;

    if let Some(stored) = header.first() {
        write_postings(transaction, write).await?;
        return Ok(stored.transaction_id);
    }

    // The business key is already committed. Exact replay returns the original
    // receipt; a different intent under the same key is a conflict that
    // quarantines rather than overwrites.
    let existing: StoredTransaction = transaction
        .query_one(Statement::with(
            sql::READ_TRANSACTION,
            vec![(
                "business_key",
                SqlValue::Text(write.business_key.as_str().to_owned()),
            )],
        ))
        .await
        .map_err(store)?;
    if existing.intent_hash == *write.intent_hash.as_bytes() {
        Ok(existing.transaction_id)
    } else {
        Err(IngestError::IntentConflict(
            write.business_key.as_str().to_owned(),
        ))
    }
}

/// Writes every posting and its projection update.
async fn write_postings(
    transaction: &mut Transaction<'_>,
    write: &JournalWrite,
) -> Result<(), IngestError> {
    for (index, posting) in write.postings.iter().enumerate() {
        let seq = i64::try_from(index + 1).unwrap_or(i64::MAX);
        transaction
            .execute(Statement::with(
                sql::INSERT_POSTING,
                vec![
                    ("transaction_id", SqlValue::Uuid(write.id.as_uuid())),
                    ("posting_seq", SqlValue::I64(seq)),
                    ("account_id", SqlValue::Uuid(posting.account.id())),
                    ("amount_microusd", SqlValue::I64(posting.amount.get())),
                ],
            ))
            .await
            .map_err(store)?;
        transaction
            .execute(Statement::with(
                sql::APPLY_PROJECTION,
                vec![
                    ("amount_microusd", SqlValue::I64(posting.amount.get())),
                    ("transaction_id", SqlValue::Uuid(write.id.as_uuid())),
                    ("account_id", SqlValue::Uuid(posting.account.id())),
                ],
            ))
            .await
            .map_err(store)?;
    }
    Ok(())
}

/// Creates an organization's account and projection row on first use.
async fn ensure_account(
    transaction: &mut Transaction<'_>,
    organization: Option<OrganizationId>,
    account: AccountRef,
) -> Result<(), IngestError> {
    let Some(organization) = organization else {
        return Err(IngestError::Conservation(
            "a customer account posting carries no organization".to_owned(),
        ));
    };
    let normal_side = match account.kind().normal_side() {
        aex_finance_domain::account::AccountSide::Debit => "debit",
        aex_finance_domain::account::AccountSide::Credit => "credit",
    };
    transaction
        .execute(Statement::with(
            sql::ENSURE_ACCOUNT,
            vec![
                ("account_id", SqlValue::Uuid(account.id())),
                ("org_id", SqlValue::Uuid(org_uuid(organization))),
                (
                    "kind",
                    SqlValue::Text(account_kind_sql(account.kind()).to_owned()),
                ),
                ("normal_side", SqlValue::Text(normal_side.to_owned())),
            ],
        ))
        .await
        .map_err(store)?;
    transaction
        .execute(Statement::with(
            sql::ENSURE_BALANCE,
            vec![("account_id", SqlValue::Uuid(account.id()))],
        ))
        .await
        .map_err(store)?;
    Ok(())
}

/// A claimed identity. The value is never read: the presence of the row is the
/// whole answer, and reading it back would invite a caller to trust it.
#[derive(Debug)]
struct ClaimedRow;

impl Row for ClaimedRow {
    fn from_record(record: &Record<'_>) -> Result<Self, DecodeError> {
        record.expect_arity(1)?;
        record.text(0)?;
        Ok(Self)
    }
}

#[cfg(test)]
mod tests {
    use aex_payment_contracts::{ProviderEventFacts, ProviderObjectRef};
    use aex_wire::PrefixedId as _;
    use aex_wire::ids::OrganizationId;
    use aex_wire::types::Cents;

    use super::{AppliedState, business_key, intended_state, sql, to_microusd};

    fn organization() -> OrganizationId {
        OrganizationId::parse("org_01kyw2qa4pew48j2gb1g6gw3rg").expect("a fixture organization")
    }

    #[test]
    fn every_statement_binds_and_never_touches_floating_point() {
        for statement in [
            sql::PROBE_ROLE,
            sql::CLAIM_EVENT,
            sql::READ_EVENT,
            sql::BIND_EVENT_TRANSACTION,
            sql::ENSURE_ACCOUNT,
            sql::ENSURE_BALANCE,
            sql::INSERT_TRANSACTION,
            sql::READ_TRANSACTION,
            sql::INSERT_POSTING,
            sql::APPLY_PROJECTION,
            sql::READ_AVAILABLE,
        ] {
            assert!(statement.contains(':'), "{statement} binds no parameter");
            assert!(!statement.to_lowercase().contains("float"));
            assert!(!statement.to_lowercase().contains("double"));
        }
    }

    #[test]
    fn a_duplicate_event_is_absorbed_by_the_primary_key_rather_than_by_a_read() {
        assert!(sql::CLAIM_EVENT.contains("ON CONFLICT (provider_event_id) DO NOTHING"));
        assert!(sql::INSERT_TRANSACTION.contains("ON CONFLICT (business_key) DO NOTHING"));
    }

    #[test]
    fn a_failed_intent_records_itself_and_moves_no_money() {
        let facts = ProviderEventFacts::PaymentIntentPaymentFailed {
            organization: organization(),
            intent: ProviderObjectRef("pi_1".to_owned()),
            failure: aex_payment_contracts::PaymentFailure {
                class: aex_payment_contracts::PaymentFailureClass::CardDeclined,
                provider_code: None,
                decline_code: None,
                retryable: false,
            },
        };
        assert_eq!(intended_state(&facts), AppliedState::IgnoredUnsupported);
    }

    #[test]
    fn every_money_event_intends_to_apply() {
        for facts in [
            ProviderEventFacts::PaymentIntentSucceeded {
                organization: organization(),
                credit: Cents::new(1_000),
                charged: Cents::new(1_000),
                intent: ProviderObjectRef("pi_1".to_owned()),
            },
            ProviderEventFacts::ChargeDisputeCreated {
                organization: organization(),
                charge: ProviderObjectRef("ch_1".to_owned()),
                amount: Cents::new(1_000),
            },
            ProviderEventFacts::RefundCreated {
                organization: organization(),
                charge: ProviderObjectRef("ch_1".to_owned()),
                refund: ProviderObjectRef("re_1".to_owned()),
                amount: Cents::new(500),
            },
        ] {
            assert_eq!(intended_state(&facts), AppliedState::Applied);
        }
    }

    #[test]
    fn cents_widen_exactly_and_refuse_an_amount_outside_the_bound() {
        assert_eq!(
            to_microusd(Cents::new(1_234)).expect("in bound").get(),
            12_340_000
        );
        assert!(to_microusd(Cents::new(u64::MAX)).is_err());
    }

    #[test]
    fn a_business_key_is_derived_from_the_provider_object_and_is_canonical() {
        let facts = ProviderEventFacts::PaymentIntentSucceeded {
            organization: organization(),
            credit: Cents::new(1_000),
            charged: Cents::new(1_000),
            intent: ProviderObjectRef("PI_ABCDEFGHIJ".to_owned()),
        };
        let event = envelope(facts);
        let key = business_key(&event).expect("a valid business key");
        assert_eq!(key.as_str(), "topup:pi_abcdefghij");
    }

    #[test]
    fn a_dispute_and_its_open_state_are_one_key() {
        let facts = ProviderEventFacts::ChargeDisputeCreated {
            organization: organization(),
            charge: ProviderObjectRef("ch_abcdefghij".to_owned()),
            amount: Cents::new(1_000),
        };
        let event = envelope(facts);
        assert_eq!(
            business_key(&event).expect("a valid key").as_str(),
            "dispute:ch_abcdefghij:open"
        );
    }

    fn envelope(facts: ProviderEventFacts) -> aex_payment_contracts::ProviderEventEnvelope {
        aex_payment_contracts::ProviderEventEnvelope {
            schema_version: aex_internal_contracts::SchemaVersion::V1,
            provider_event_id: aex_payment_contracts::ProviderEventId("evt_1".to_owned()),
            object: ProviderObjectRef("obj_1".to_owned()),
            kind: facts.kind(),
            occurred_at: aex_wire::types::Timestamp::from_unix_millis(1_800_000_000_000)
                .expect("an instant"),
            facts,
            raw_digest: aex_wire::ids::ContentHash::from_bytes([3u8; 32]),
            provider_api_version: aex_payment_contracts::PinnedApiVersion(
                "2026-06-24.dahlia".to_owned(),
            ),
            effect: None,
            received_at: aex_wire::types::Timestamp::from_unix_millis(1_800_000_000_001)
                .expect("an instant"),
        }
    }
}
