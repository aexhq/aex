//! The Aurora Data `API` implementation of [`BillingAuthority`].
//!
//! Every statement is a `const &str` with bound parameters; nothing is
//! assembled by concatenation. Money is projected as `bigint` micro-USD and
//! decoded through [`aex_rds_data::Record::i64`], which has no `doubleValue`
//! arm — a schema change that introduced floating point would fail here rather
//! than round silently.

use std::sync::Arc;

use aex_control_domain::{AccountProfile, AccountState};
use aex_finance_domain::money::Microusd;
use aex_payment_contracts::{
    CommandKind, EffectId, PaymentResult, ProviderCustomerRef, RedactedEmail,
};
use aex_rds_data::{
    CommitFailure, DataApiClient, DataApiError, DecodeError, Isolation, Record, Row, SqlValue,
    Statement,
};
use aex_wire::Uuid7;
use aex_wire::ids::OrganizationId;

use crate::authority::{
    AuthorityError, BalanceRecord, BillingAuthority, EffectPreparation, PolicyChange, PolicyRecord,
    StatementHeader, StatementLineRecord, StatementPage,
};

/// Every statement this deployable runs, in one place.
pub mod sql {
    /// Proves the connection holds its own role and that the role cannot mutate
    /// the journal. Readiness is a grant fact, not a liveness fact.
    pub const PROBE_ROLE: &str = "\
SELECT pg_has_role(current_user, :role, 'MEMBER'), \
       has_table_privilege(:role, 'finance.journal_transaction', 'UPDATE'), \
       has_table_privilege(:role, 'finance.account_balance', 'SELECT')";

    /// The published account state of one organization.
    ///
    /// The same view, the same four columns and the same fail-closed `CASE` the
    /// control plane reads at `aex_control_aurora::sql::GET_ACCOUNT_PROFILE`.
    /// Finance reads the projection rather than its own `billing_account.state`
    /// so that one account has one published state: deriving it here from the
    /// durable column and the live balance is what let the same account read
    /// active on one route and paused on another in the same second.
    ///
    /// The revocation epoch that statement also selects is deliberately absent.
    /// It is a `control.authorization_epoch` row, no finance route publishes it,
    /// and `aex_finance_api` holds no privilege on that table.
    pub const READ_ACCOUNT_PROFILE: &str = "\
SELECT a.status, a.reason, a.revision, \
       (EXTRACT(EPOCH FROM a.changed_at)*1000)::bigint \
  FROM finance.account_state_v1 a \
 WHERE a.organization_id = :org_id";

    /// The prepaid position of one organization.
    ///
    /// `customer_available` and `customer_reserved` are credit-normal, so the
    /// spendable amount is the negation of the stored balance. `pending` is the
    /// part of the escrow already consumed by rated usage whose reservation has
    /// not closed.
    ///
    /// The available amount is floored at zero. Since
    /// `20260801001300_finance_customer_overdraw` an asynchronous deduction may
    /// take `customer_available` past zero — that is the signal that pauses the
    /// account, not a corruption — and `BillingBalance.availableCents` is an
    /// unsigned published amount that `Microusd` refuses to hold a negative in.
    /// `GREATEST` is the honest projection rather than a rounding: the field
    /// means *spendable now*, and an overdrawn account can spend nothing. The
    /// shortfall is reported as a fact of its own, by `operationalState` going
    /// `paused` with `reason = top_up_required` and the flat
    /// `minimumRestoreCents` remedy beside it. Without the floor the balance
    /// endpoint would fail to decode for precisely the customers who need it to
    /// tell them to top up.
    pub const READ_BALANCE: &str = "\
SELECT GREATEST(coalesce((SELECT -sum(b.balance_microusd) FROM finance.account_balance b \
                  JOIN finance.account a ON a.account_id = b.account_id \
                 WHERE a.org_id = ba.org_id AND a.kind = 'customer_available'), 0), 0)::bigint, \
       coalesce((SELECT -sum(b.balance_microusd) FROM finance.account_balance b \
                  JOIN finance.account a ON a.account_id = b.account_id \
                 WHERE a.org_id = ba.org_id AND a.kind = 'customer_reserved'), 0)::bigint, \
       coalesce((SELECT sum(r.settled_microusd) FROM finance.reservation r \
                 WHERE r.org_id = ba.org_id AND r.state IN ('open','closing')), 0)::bigint, \
       coalesce((SELECT sum(b.posting_count) FROM finance.account_balance b \
                  JOIN finance.account a ON a.account_id = b.account_id \
                 WHERE a.org_id = ba.org_id), 0)::bigint, \
       coalesce((SELECT (EXTRACT(EPOCH FROM max(b.updated_at))*1000)::bigint \
                   FROM finance.account_balance b \
                   JOIN finance.account a ON a.account_id = b.account_id \
                  WHERE a.org_id = ba.org_id), 0)::bigint \
  FROM finance.billing_account ba \
 WHERE ba.org_id = :org_id";

    /// The automatic top-up policy of one organization.
    pub const READ_POLICY: &str = "\
SELECT ba.auto_topup_enabled, ba.auto_topup_threshold_microusd, ba.auto_topup_amount_microusd, \
       (ba.default_payment_method_id IS NOT NULL), ba.revision, \
       coalesce((SELECT (EXTRACT(EPOCH FROM max(b.updated_at))*1000)::bigint \
                   FROM finance.account_balance b \
                   JOIN finance.account a ON a.account_id = b.account_id \
                  WHERE a.org_id = ba.org_id), 0)::bigint \
  FROM finance.billing_account ba \
 WHERE ba.org_id = :org_id";

    /// Replaces the policy under a revision precondition.
    pub const REPLACE_POLICY: &str = "\
UPDATE finance.billing_account \
   SET auto_topup_enabled = :enabled, \
       auto_topup_threshold_microusd = :threshold_microusd, \
       auto_topup_amount_microusd = :amount_microusd, \
       revision = revision + 1 \
 WHERE org_id = :org_id AND revision = :expect_revision";

    /// One page of issued statement headers, newest first.
    pub const LIST_STATEMENTS: &str = "\
SELECT s.statement_id, s.period, s.closing_microusd, s.content_sha256, s.object_key, \
       coalesce((EXTRACT(EPOCH FROM s.issued_at)*1000)::bigint, 0) \
  FROM finance.statement s \
 WHERE s.org_id = :org_id AND s.state = 'issued' \
   AND (:before_period = '' OR s.period < :before_period) \
 ORDER BY s.period DESC \
 LIMIT :page_limit";

    /// One issued statement header.
    pub const READ_STATEMENT: &str = "\
SELECT s.statement_id, s.period, s.closing_microusd, s.content_sha256, s.object_key, \
       coalesce((EXTRACT(EPOCH FROM s.issued_at)*1000)::bigint, 0) \
  FROM finance.statement s \
 WHERE s.org_id = :org_id AND s.statement_id = :statement_id AND s.state = 'issued'";

    /// The priced category lines of one issued period.
    pub const READ_STATEMENT_LINES: &str = "\
SELECT i.category, coalesce(sum(i.rated_microusd), 0)::bigint \
  FROM finance.usage_inbox i \
 WHERE i.org_id = :org_id AND i.state = 'rated' \
   AND to_char(i.interval_start, 'YYYY-MM') = :period \
 GROUP BY i.category \
 ORDER BY i.category";

    /// The provider customer record, which every hosted command needs.
    pub const READ_PROVIDER_CUSTOMER: &str = "\
SELECT ba.provider_customer_id FROM finance.billing_account ba WHERE ba.org_id = :org_id";

    /// The address a provider customer record is created against.
    pub const READ_BILLING_CONTACT: &str = "SELECT finance.billing_contact_email(:org_id)";

    /// Claims the provider customer record for an organization that has none.
    ///
    /// Guarded rather than blind. The column is `UNIQUE`, so a second concurrent
    /// `EnsureCustomer` that overwrote the first would orphan a provider customer
    /// together with every payment method saved against it, and the row that lost
    /// would be a live Stripe customer nothing in AEX names.
    pub const RECORD_PROVIDER_CUSTOMER: &str = "\
UPDATE finance.billing_account \
   SET provider_customer_id = :provider_customer_id \
 WHERE org_id = :org_id AND provider_customer_id IS NULL";

    /// Commits a `prepared` effect. The unique intent claim is the fence: a
    /// replayed intent resolves the original effect instead of opening a second.
    pub const PREPARE_EFFECT: &str = "\
INSERT INTO finance.provider_effect \
  (effect_id, org_id, kind, state, intent_hash, request_json, amount_microusd, deadline_at) \
VALUES \
  (:effect_id, :org_id, :kind, 'prepared', :intent_hash, :request_json, :amount_microusd, \
   (TIMESTAMPTZ 'epoch' + (:deadline_ms) * INTERVAL '1 millisecond')) \
ON CONFLICT (org_id, kind, intent_hash) DO NOTHING \
RETURNING effect_id";

    /// Reads back the effect a conflicting intent already claimed.
    pub const READ_EFFECT_BY_INTENT: &str = "\
SELECT pe.effect_id, pe.state FROM finance.provider_effect pe \
 WHERE pe.org_id = :org_id AND pe.kind = :kind AND pe.intent_hash = :intent_hash";

    /// Replaces the preparation marker with the complete admitted command.
    pub const BIND_EFFECT_COMMAND: &str = "\
UPDATE finance.provider_effect \
   SET request_json = :request_json, revision = revision + 1 \
 WHERE effect_id = :effect_id AND intent_hash = :intent_hash AND state = 'prepared' \
   AND (request_json ->> 'schemaVersion') IS NULL";

    /// Accepts an identical replay after a lost binding response.
    pub const READ_BOUND_EFFECT_COMMAND: &str = "\
SELECT request_json FROM finance.provider_effect \
 WHERE effect_id = :effect_id AND intent_hash = :intent_hash";

    /// Records what the provider actually did.
    ///
    /// `outcome_unknown` is a first-class terminal-for-now state: the row keeps
    /// its identity so recovery replays the same idempotency key.
    pub const FINALIZE_EFFECT: &str = "\
UPDATE finance.provider_effect \
   SET state = :state, \
       provider_object_id = coalesce(:provider_object_id, provider_object_id), \
       provider_status = :provider_status, \
       failure_code = :failure_code, \
       decline_code = :decline_code, \
       attempts = attempts + 1, \
       first_dispatch_at = coalesce(first_dispatch_at, now()), \
       resolved_at = CASE WHEN :state IN ('succeeded','failed') THEN now() ELSE resolved_at END, \
       revision = revision + 1 \
 WHERE effect_id = :effect_id \
   AND state IN ('prepared','dispatched','outcome_unknown')";
}

/// The Aurora-backed billing authority.
#[derive(Debug, Clone)]
pub struct AuroraBillingAuthority {
    client: Arc<DataApiClient>,
    role: String,
}

impl AuroraBillingAuthority {
    /// Builds the adapter over a configured Data `API` client.
    #[must_use]
    pub fn new(client: Arc<DataApiClient>, role: String) -> Self {
        Self { client, role }
    }
}

/// The durable `finance.provider_effect.kind` for one command.
///
/// `CreatePortalSession` and `LookupEffectOutcome` have no arm: the first
/// creates no money effect and the second resolves one that already exists.
#[must_use]
pub const fn durable_effect_kind(kind: CommandKind) -> Option<&'static str> {
    match kind {
        CommandKind::EnsureCustomer => Some("customer_create"),
        CommandKind::CreateTopUpCheckout => Some("checkout_session_create"),
        CommandKind::ChargeSavedMethod => Some("payment_intent_off_session"),
        CommandKind::RefundCharge => Some("refund_create"),
        CommandKind::CreatePortalSession | CommandKind::LookupEffectOutcome => None,
    }
}

/// The prepaid position row.
#[derive(Debug)]
struct BalanceRow {
    available: i64,
    reserved: i64,
    pending: i64,
    revision: i64,
    updated_at_millis: i64,
}

impl Row for BalanceRow {
    fn from_record(record: &Record<'_>) -> Result<Self, DecodeError> {
        record.expect_arity(5)?;
        Ok(Self {
            available: record.i64(0)?,
            reserved: record.i64(1)?,
            pending: record.i64(2)?,
            revision: record.i64(3)?,
            updated_at_millis: record.i64(4)?,
        })
    }
}

/// The published account-state row.
#[derive(Debug)]
struct AccountProfileRow(AccountProfile);

impl Row for AccountProfileRow {
    fn from_record(record: &Record<'_>) -> Result<Self, DecodeError> {
        record.expect_arity(4)?;
        let state = AccountState::parse(record.text(0)?).ok_or(DecodeError::TypeMismatch {
            index: 0,
            expected: "an account state",
        })?;
        Ok(Self(AccountProfile {
            state,
            reason: record.opt(1, |row, index| row.text(index).map(str::to_owned))?,
            revision: u64::try_from(record.i64(2)?)
                .map_err(|_| DecodeError::Overflow { index: 2 })?,
            changed_at: record.timestamp_millis(3)?,
        }))
    }
}

/// The automatic top-up policy row.
#[derive(Debug)]
struct PolicyRow {
    enabled: bool,
    threshold: i64,
    amount: i64,
    has_payment_method: bool,
    revision: i64,
    updated_at_millis: i64,
}

impl Row for PolicyRow {
    fn from_record(record: &Record<'_>) -> Result<Self, DecodeError> {
        record.expect_arity(6)?;
        Ok(Self {
            enabled: record.bool(0)?,
            threshold: record.i64(1)?,
            amount: record.i64(2)?,
            has_payment_method: record.bool(3)?,
            revision: record.i64(4)?,
            updated_at_millis: record.i64(5)?,
        })
    }
}

/// One issued statement header row.
#[derive(Debug)]
struct StatementRow {
    statement_id: uuid::Uuid,
    period: String,
    closing: i64,
    content_sha256: Option<[u8; 32]>,
    object_key: Option<String>,
    issued_at_millis: i64,
}

impl Row for StatementRow {
    fn from_record(record: &Record<'_>) -> Result<Self, DecodeError> {
        record.expect_arity(6)?;
        Ok(Self {
            statement_id: record.uuid(0)?,
            period: record.text(1)?.to_owned(),
            closing: record.i64(2)?,
            content_sha256: record.opt(3, Record::fixed::<32>)?,
            object_key: record.opt(4, |r, i| r.text(i).map(str::to_owned))?,
            issued_at_millis: record.i64(5)?,
        })
    }
}

/// One priced category line row.
#[derive(Debug)]
struct LineRow {
    category: String,
    total: i64,
}

impl Row for LineRow {
    fn from_record(record: &Record<'_>) -> Result<Self, DecodeError> {
        record.expect_arity(2)?;
        Ok(Self {
            category: record.text(0)?.to_owned(),
            total: record.i64(1)?,
        })
    }
}

/// A single text column.
#[derive(Debug)]
struct TextRow(Option<String>);

impl Row for TextRow {
    fn from_record(record: &Record<'_>) -> Result<Self, DecodeError> {
        record.expect_arity(1)?;
        Ok(Self(record.opt(0, |r, i| r.text(i).map(str::to_owned))?))
    }
}

/// A single uuid column.
#[derive(Debug)]
struct UuidRow(uuid::Uuid);

impl Row for UuidRow {
    fn from_record(record: &Record<'_>) -> Result<Self, DecodeError> {
        record.expect_arity(1)?;
        Ok(Self(record.uuid(0)?))
    }
}

/// The three booleans the readiness probe checks.
#[derive(Debug)]
struct GrantRow {
    holds_role: bool,
    can_mutate_journal: bool,
    can_read_balances: bool,
}

impl Row for GrantRow {
    fn from_record(record: &Record<'_>) -> Result<Self, DecodeError> {
        record.expect_arity(3)?;
        Ok(Self {
            holds_role: record.bool(0)?,
            can_mutate_journal: record.bool(1)?,
            can_read_balances: record.bool(2)?,
        })
    }
}

/// Wraps a stored `bigint` as a bounded non-negative money amount.
fn money(raw: i64, column: &'static str) -> Result<Microusd, AuthorityError> {
    Microusd::new(raw).map_err(|error| {
        AuthorityError::Decode(format!("{column} is not a valid micro-USD amount: {error}"))
    })
}

/// Classifies a transport failure without ever inventing an outcome.
fn store(error: DataApiError) -> AuthorityError {
    match error {
        DataApiError::Decode(decode) => AuthorityError::Decode(decode.to_string()),
        other => AuthorityError::Unavailable(other.to_string()),
    }
}

/// Classifies a commit failure. Only an explicit rollback is a clean failure.
fn commit(failure: CommitFailure) -> AuthorityError {
    match failure {
        CommitFailure::RolledBack(error) => AuthorityError::Unavailable(error.to_string()),
        CommitFailure::Unknown(error) => AuthorityError::OutcomeUnknown(error.to_string()),
    }
}

/// The organization identity as the `uuid` column stores it.
fn org_uuid(organization: OrganizationId) -> uuid::Uuid {
    use aex_wire::PrefixedId as _;
    uuid::Uuid::from_bytes(*organization.uuid7().as_bytes())
}

#[async_trait::async_trait]
#[allow(
    clippy::too_many_lines,
    reason = "one method per port operation; splitting the impl block would hide the surface"
)]
impl BillingAuthority for AuroraBillingAuthority {
    async fn probe_role(&self) -> Result<(), AuthorityError> {
        let row: GrantRow = self
            .client
            .query_one(Statement::with(
                sql::PROBE_ROLE,
                vec![("role", SqlValue::Text(self.role.clone()))],
            ))
            .await
            .map_err(store)?;
        if !row.holds_role {
            return Err(AuthorityError::Refused(format!(
                "the connected login does not hold `{}`",
                self.role
            )));
        }
        if !row.can_read_balances {
            return Err(AuthorityError::Refused(format!(
                "`{}` cannot read finance.account_balance",
                self.role
            )));
        }
        if row.can_mutate_journal {
            return Err(AuthorityError::Refused(format!(
                "`{}` holds UPDATE on finance.journal_transaction; history is append-only",
                self.role
            )));
        }
        Ok(())
    }

    async fn balance(&self, organization: OrganizationId) -> Result<BalanceRecord, AuthorityError> {
        let row: BalanceRow = self
            .client
            .query_opt(Statement::with(
                sql::READ_BALANCE,
                vec![("org_id", SqlValue::Uuid(org_uuid(organization)))],
            ))
            .await
            .map_err(store)?
            .ok_or(AuthorityError::UnknownOrganization)?;
        Ok(BalanceRecord {
            available: money(row.available, "available_microusd")?,
            reserved: money(row.reserved, "reserved_microusd")?,
            pending: money(row.pending, "pending_microusd")?,
            revision: u64::try_from(row.revision).unwrap_or(0),
            updated_at_millis: row.updated_at_millis,
        })
    }

    async fn account_profile(
        &self,
        organization: OrganizationId,
    ) -> Result<AccountProfile, AuthorityError> {
        let row: AccountProfileRow = self
            .client
            .query_opt(Statement::with(
                sql::READ_ACCOUNT_PROFILE,
                vec![("org_id", SqlValue::Uuid(org_uuid(organization)))],
            ))
            .await
            .map_err(store)?
            .ok_or(AuthorityError::UnknownOrganization)?;
        Ok(row.0)
    }

    async fn policy(&self, organization: OrganizationId) -> Result<PolicyRecord, AuthorityError> {
        let row: PolicyRow = self
            .client
            .query_opt(Statement::with(
                sql::READ_POLICY,
                vec![("org_id", SqlValue::Uuid(org_uuid(organization)))],
            ))
            .await
            .map_err(store)?
            .ok_or(AuthorityError::UnknownOrganization)?;
        Ok(PolicyRecord {
            enabled: row.enabled,
            threshold: money(row.threshold, "auto_topup_threshold_microusd")?,
            amount: money(row.amount, "auto_topup_amount_microusd")?,
            has_payment_method: row.has_payment_method,
            revision: u64::try_from(row.revision).unwrap_or(0),
            updated_at_millis: row.updated_at_millis,
        })
    }

    async fn replace_policy(
        &self,
        organization: OrganizationId,
        change: PolicyChange,
        expect_revision: u64,
    ) -> Result<PolicyRecord, AuthorityError> {
        let organization_uuid = org_uuid(organization);
        let mut transaction = self
            .client
            .begin(Isolation::Serializable)
            .await
            .map_err(store)?;
        let updated = transaction
            .execute(Statement::with(
                sql::REPLACE_POLICY,
                vec![
                    ("enabled", SqlValue::Bool(change.enabled)),
                    ("threshold_microusd", SqlValue::I64(change.threshold.get())),
                    ("amount_microusd", SqlValue::I64(change.amount.get())),
                    ("org_id", SqlValue::Uuid(organization_uuid)),
                    (
                        "expect_revision",
                        SqlValue::I64(i64::try_from(expect_revision).unwrap_or(i64::MAX)),
                    ),
                ],
            ))
            .await;
        let updated = match updated {
            Ok(rows) => rows,
            Err(error) => {
                let _ = transaction.rollback().await;
                return Err(store(error));
            }
        };
        if updated == 0 {
            let _ = transaction.rollback().await;
            return Err(AuthorityError::RevisionConflict);
        }
        let row: PolicyRow = match transaction
            .query_one(Statement::with(
                sql::READ_POLICY,
                vec![("org_id", SqlValue::Uuid(organization_uuid))],
            ))
            .await
        {
            Ok(row) => row,
            Err(error) => {
                let _ = transaction.rollback().await;
                return Err(store(error));
            }
        };
        transaction.commit().await.map_err(commit)?;
        Ok(PolicyRecord {
            enabled: row.enabled,
            threshold: money(row.threshold, "auto_topup_threshold_microusd")?,
            amount: money(row.amount, "auto_topup_amount_microusd")?,
            has_payment_method: row.has_payment_method,
            revision: u64::try_from(row.revision).unwrap_or(0),
            updated_at_millis: row.updated_at_millis,
        })
    }

    async fn statements(
        &self,
        organization: OrganizationId,
        before_period: Option<&str>,
        limit: u32,
    ) -> Result<StatementPage, AuthorityError> {
        let rows: Vec<StatementRow> = self
            .client
            .query(Statement::with(
                sql::LIST_STATEMENTS,
                vec![
                    ("org_id", SqlValue::Uuid(org_uuid(organization))),
                    (
                        "before_period",
                        SqlValue::Text(before_period.unwrap_or_default().to_owned()),
                    ),
                    ("page_limit", SqlValue::I64(i64::from(limit))),
                ],
            ))
            .await
            .map_err(store)?;
        let complete = rows.len() < limit as usize;
        let mut items = Vec::with_capacity(rows.len());
        for row in rows {
            items.push(header(row)?);
        }
        let next_period = if complete {
            None
        } else {
            items.last().map(|item| item.period.clone())
        };
        Ok(StatementPage { items, next_period })
    }

    async fn statement(
        &self,
        organization: OrganizationId,
        statement_id: uuid::Uuid,
    ) -> Result<StatementHeader, AuthorityError> {
        let row: StatementRow = self
            .client
            .query_opt(Statement::with(
                sql::READ_STATEMENT,
                vec![
                    ("org_id", SqlValue::Uuid(org_uuid(organization))),
                    ("statement_id", SqlValue::Uuid(statement_id)),
                ],
            ))
            .await
            .map_err(store)?
            .ok_or(AuthorityError::NotFound("statement"))?;
        header(row)
    }

    async fn statement_lines(
        &self,
        organization: OrganizationId,
        period: &str,
    ) -> Result<Vec<StatementLineRecord>, AuthorityError> {
        let rows: Vec<LineRow> = self
            .client
            .query(Statement::with(
                sql::READ_STATEMENT_LINES,
                vec![
                    ("org_id", SqlValue::Uuid(org_uuid(organization))),
                    ("period", SqlValue::Text(period.to_owned())),
                ],
            ))
            .await
            .map_err(store)?;
        rows.into_iter()
            .map(|row| {
                Ok(StatementLineRecord {
                    total: money(row.total, "rated_microusd")?,
                    category: row.category,
                })
            })
            .collect()
    }

    async fn prepare_effect(
        &self,
        organization: OrganizationId,
        kind: CommandKind,
        intent: &[u8],
        amount: Option<Microusd>,
        deadline_millis: i64,
    ) -> Result<EffectPreparation, AuthorityError> {
        let organization_uuid = org_uuid(organization);
        let intent_hash: [u8; 32] = blake3::hash(intent).into();
        let Some(durable_kind) = durable_effect_kind(kind) else {
            // A hosted portal session creates no money effect and the durable
            // `finance.provider_effect.kind` set has no value for one. The
            // derived provider idempotency key still fences a duplicate object
            // inside the provider's own replay window.
            return Ok(EffectPreparation {
                effect: EffectId(mint_effect_id()),
                organization,
                intent_hash,
            });
        };

        let mut transaction = self
            .client
            .begin(Isolation::Serializable)
            .await
            .map_err(store)?;
        let minted = mint_effect_id();
        let inserted: Result<Vec<UuidRow>, DataApiError> = transaction
            .query(Statement::with(
                sql::PREPARE_EFFECT,
                vec![
                    (
                        "effect_id",
                        SqlValue::Uuid(uuid::Uuid::from_bytes(*minted.as_bytes())),
                    ),
                    ("org_id", SqlValue::Uuid(organization_uuid)),
                    ("kind", SqlValue::Text(durable_kind.to_owned())),
                    ("intent_hash", SqlValue::Bytes(intent_hash.to_vec())),
                    (
                        "request_json",
                        SqlValue::Json(serde_json::json!({
                            "kind": kind.as_str(),
                            "amountMicrousd": amount.map(Microusd::get),
                        })),
                    ),
                    (
                        "amount_microusd",
                        amount.map_or(SqlValue::Null, |value| SqlValue::I64(value.get())),
                    ),
                    ("deadline_ms", SqlValue::I64(deadline_millis)),
                ],
            ))
            .await;
        let inserted = match inserted {
            Ok(rows) => rows,
            Err(DataApiError::UniqueViolation { constraint }) => {
                let _ = transaction.rollback().await;
                return Err(if constraint.contains("one_open_charge") {
                    AuthorityError::EffectAlreadyOpen
                } else {
                    AuthorityError::Refused(format!("provider effect refused: {constraint}"))
                });
            }
            Err(error) => {
                let _ = transaction.rollback().await;
                return Err(store(error));
            }
        };

        let effect_uuid = if let Some(row) = inserted.first() {
            row.0
        } else {
            let claimed: Result<Option<EffectRow>, DataApiError> = transaction
                .query_opt(Statement::with(
                    sql::READ_EFFECT_BY_INTENT,
                    vec![
                        ("org_id", SqlValue::Uuid(organization_uuid)),
                        ("kind", SqlValue::Text(durable_kind.to_owned())),
                        ("intent_hash", SqlValue::Bytes(intent_hash.to_vec())),
                    ],
                ))
                .await;
            match claimed {
                Ok(Some(row)) => row.effect_id,
                Ok(None) => {
                    let _ = transaction.rollback().await;
                    return Err(AuthorityError::Refused(
                        "the effect intent was claimed and then vanished".to_owned(),
                    ));
                }
                Err(error) => {
                    let _ = transaction.rollback().await;
                    return Err(store(error));
                }
            }
        };
        transaction.commit().await.map_err(commit)?;

        Ok(EffectPreparation {
            effect: EffectId(Uuid7::from_bytes(*effect_uuid.as_bytes()).map_err(|error| {
                AuthorityError::Decode(format!("effect id is not a UUIDv7: {error}"))
            })?),
            organization,
            intent_hash,
        })
    }

    async fn provider_customer(
        &self,
        organization: OrganizationId,
    ) -> Result<Option<ProviderCustomerRef>, AuthorityError> {
        let row: TextRow = self
            .client
            .query_opt(Statement::with(
                sql::READ_PROVIDER_CUSTOMER,
                vec![("org_id", SqlValue::Uuid(org_uuid(organization)))],
            ))
            .await
            .map_err(store)?
            .ok_or(AuthorityError::UnknownOrganization)?;
        Ok(row.0.map(ProviderCustomerRef))
    }

    async fn billing_contact(
        &self,
        organization: OrganizationId,
    ) -> Result<RedactedEmail, AuthorityError> {
        let row: TextRow = self
            .client
            .query_one(Statement::with(
                sql::READ_BILLING_CONTACT,
                vec![("org_id", SqlValue::Uuid(org_uuid(organization)))],
            ))
            .await
            .map_err(store)?;
        row.0
            .map(RedactedEmail::new)
            .ok_or(AuthorityError::UnknownOrganization)
    }

    async fn settle_customer(
        &self,
        effect: EffectId,
        organization: OrganizationId,
        result: &PaymentResult,
    ) -> Result<Option<ProviderCustomerRef>, AuthorityError> {
        let organization_uuid = org_uuid(organization);
        let mut transaction = self
            .client
            .begin(Isolation::Serializable)
            .await
            .map_err(store)?;
        let finalized = finalize_in(&mut transaction, effect, result).await;
        if let Err(error) = finalized {
            let _ = transaction.rollback().await;
            return Err(error);
        }
        let PaymentResult::Succeeded { provider_ref, .. } = result else {
            transaction.commit().await.map_err(commit)?;
            return Ok(None);
        };
        let recorded = transaction
            .execute(Statement::with(
                sql::RECORD_PROVIDER_CUSTOMER,
                vec![
                    (
                        "provider_customer_id",
                        SqlValue::Text(provider_ref.0.clone()),
                    ),
                    ("org_id", SqlValue::Uuid(organization_uuid)),
                ],
            ))
            .await;
        if let Err(error) = recorded {
            let _ = transaction.rollback().await;
            return Err(store(error));
        }
        // Read back rather than assume: a concurrent ensure may have claimed the
        // column first, and the customer this organization *has* is the one every
        // later command must act on — not the one this call created.
        let effective: Result<TextRow, DataApiError> = transaction
            .query_one(Statement::with(
                sql::READ_PROVIDER_CUSTOMER,
                vec![("org_id", SqlValue::Uuid(organization_uuid))],
            ))
            .await;
        let effective = match effective {
            Ok(row) => row,
            Err(error) => {
                let _ = transaction.rollback().await;
                return Err(store(error));
            }
        };
        transaction.commit().await.map_err(commit)?;
        effective
            .0
            .map(ProviderCustomerRef)
            .map(Some)
            .ok_or_else(|| {
                AuthorityError::Refused(
                    "the provider customer record was claimed and then vanished".to_owned(),
                )
            })
    }

    async fn bind_effect_command(
        &self,
        effect: EffectId,
        intent_hash: [u8; 32],
        envelope: &aex_payment_contracts::PaymentCommandEnvelope,
    ) -> Result<(), AuthorityError> {
        let encoded = serde_json::to_value(envelope)
            .map_err(|error| AuthorityError::Decode(error.to_string()))?;
        let effect_id = uuid::Uuid::from_bytes(*effect.0.as_bytes());
        let updated = self
            .client
            .execute(Statement::with(
                sql::BIND_EFFECT_COMMAND,
                vec![
                    ("request_json", SqlValue::Json(encoded.clone())),
                    ("effect_id", SqlValue::Uuid(effect_id)),
                    ("intent_hash", SqlValue::Bytes(intent_hash.to_vec())),
                ],
            ))
            .await
            .map_err(store)?;
        if updated == 1 {
            return Ok(());
        }
        let stored: Option<JsonRow> = self
            .client
            .query_opt(Statement::with(
                sql::READ_BOUND_EFFECT_COMMAND,
                vec![
                    ("effect_id", SqlValue::Uuid(effect_id)),
                    ("intent_hash", SqlValue::Bytes(intent_hash.to_vec())),
                ],
            ))
            .await
            .map_err(store)?;
        match stored {
            Some(row) if row.0 == encoded => Ok(()),
            Some(_) => Err(AuthorityError::Refused(
                "the provider effect is already bound to another command".to_owned(),
            )),
            None => Err(AuthorityError::NotFound("provider effect")),
        }
    }

    async fn finalize_effect(
        &self,
        effect: EffectId,
        result: &PaymentResult,
    ) -> Result<(), AuthorityError> {
        let mut transaction = self
            .client
            .begin(Isolation::Serializable)
            .await
            .map_err(store)?;
        if let Err(error) = finalize_in(&mut transaction, effect, result).await {
            let _ = transaction.rollback().await;
            return Err(error);
        }
        transaction.commit().await.map(|_| ()).map_err(commit)
    }
}

/// Records the provider's answer inside a transaction the caller owns.
///
/// Shared so that `settle_customer` writes the provider customer id and closes
/// the effect that created it in **one** commit. Two commits would leave a window
/// in which an effect reads `succeeded` and the column is still null — and the
/// recovery from that window is a provider call whose answer no statement here
/// would accept, because `FINALIZE_EFFECT` refuses a terminal row.
async fn finalize_in(
    transaction: &mut aex_rds_data::Transaction<'_>,
    effect: EffectId,
    result: &PaymentResult,
) -> Result<(), AuthorityError> {
    let (state, object_id, status, failure_code, decline_code) = finalize_columns(result);
    let updated = transaction
        .execute(Statement::with(
            sql::FINALIZE_EFFECT,
            vec![
                ("state", SqlValue::Text(state.to_owned())),
                (
                    "provider_object_id",
                    object_id.map_or(SqlValue::Null, SqlValue::Text),
                ),
                ("provider_status", SqlValue::Text(status)),
                (
                    "failure_code",
                    failure_code.map_or(SqlValue::Null, SqlValue::Text),
                ),
                (
                    "decline_code",
                    decline_code.map_or(SqlValue::Null, SqlValue::Text),
                ),
                (
                    "effect_id",
                    SqlValue::Uuid(uuid::Uuid::from_bytes(*effect.0.as_bytes())),
                ),
            ],
        ))
        .await
        .map_err(store)?;
    if updated == 0 {
        return Err(AuthorityError::NotFound("provider effect"));
    }
    Ok(())
}

/// One JSON document.
#[derive(Debug)]
struct JsonRow(serde_json::Value);

impl Row for JsonRow {
    fn from_record(record: &Record<'_>) -> Result<Self, DecodeError> {
        record.expect_arity(1)?;
        Ok(Self(record.json(0)?))
    }
}

/// The effect row a conflicting intent already claimed.
#[derive(Debug)]
struct EffectRow {
    effect_id: uuid::Uuid,
    #[allow(dead_code, reason = "the state is projected for diagnostics only")]
    state: String,
}

impl Row for EffectRow {
    fn from_record(record: &Record<'_>) -> Result<Self, DecodeError> {
        record.expect_arity(2)?;
        Ok(Self {
            effect_id: record.uuid(0)?,
            state: record.text(1)?.to_owned(),
        })
    }
}

/// The five columns a finalization writes.
fn finalize_columns(
    result: &PaymentResult,
) -> (
    &'static str,
    Option<String>,
    String,
    Option<String>,
    Option<String>,
) {
    match result {
        PaymentResult::Succeeded { provider_ref, .. } => (
            "succeeded",
            Some(provider_ref.0.clone()),
            "succeeded".to_owned(),
            None,
            None,
        ),
        PaymentResult::Failed { failure, .. } => (
            "failed",
            None,
            "failed".to_owned(),
            Some(format!("{:?}", failure.class)),
            failure
                .decline_code
                .as_ref()
                .map(|code| code.as_str().to_owned()),
        ),
        PaymentResult::Unknown { evidence, .. } => (
            "outcome_unknown",
            None,
            "outcome_unknown".to_owned(),
            Some(evidence_label(evidence).to_owned()),
            None,
        ),
    }
}

/// A low-cardinality label for one indeterminate observation.
const fn evidence_label(evidence: &aex_payment_contracts::UnknownEvidence) -> &'static str {
    use aex_payment_contracts::UnknownEvidence as Evidence;
    match evidence {
        Evidence::Timeout { .. } => "timeout",
        Evidence::TransportLost => "transport_lost",
        Evidence::ServerError { .. } => "server_error",
        Evidence::AmbiguousResponse { .. } => "ambiguous_response",
    }
}

/// Converts one statement row into the port's record.
fn header(row: StatementRow) -> Result<StatementHeader, AuthorityError> {
    Ok(StatementHeader {
        statement_id: row.statement_id,
        period: row.period,
        closing: money(row.closing, "closing_microusd")?,
        content_sha256: row.content_sha256,
        object_key: row.object_key,
        issued_at_millis: row.issued_at_millis,
    })
}

/// Mints a fresh time-ordered effect identity.
fn mint_effect_id() -> Uuid7 {
    let raw = uuid::Uuid::now_v7();
    Uuid7::from_bytes(*raw.as_bytes()).unwrap_or_else(|error| {
        unreachable!("`uuid::Uuid::now_v7` always produces a UUIDv7: {error}")
    })
}

#[cfg(test)]
mod tests {
    use aex_payment_contracts::CommandKind;

    use super::{durable_effect_kind, sql};

    #[test]
    fn every_statement_binds_and_never_concatenates() {
        for statement in [
            sql::PROBE_ROLE,
            sql::READ_ACCOUNT_PROFILE,
            sql::READ_BALANCE,
            sql::READ_POLICY,
            sql::REPLACE_POLICY,
            sql::LIST_STATEMENTS,
            sql::READ_STATEMENT,
            sql::READ_STATEMENT_LINES,
            sql::READ_PROVIDER_CUSTOMER,
            sql::READ_BILLING_CONTACT,
            sql::RECORD_PROVIDER_CUSTOMER,
            sql::PREPARE_EFFECT,
            sql::READ_EFFECT_BY_INTENT,
            sql::BIND_EFFECT_COMMAND,
            sql::READ_BOUND_EFFECT_COMMAND,
            sql::FINALIZE_EFFECT,
        ] {
            assert!(statement.contains(':'), "{statement} binds no parameter");
            assert!(
                !statement.contains("::float8") && !statement.to_lowercase().contains("double"),
                "a finance statement must never cast money to floating point"
            );
        }
    }

    /// The published available amount is floored, because the ledger is not.
    ///
    /// `20260801001300_finance_customer_overdraw` lets an asynchronous deduction
    /// take `customer_available` past zero — that overdraw is the signal that
    /// pauses the account. `BillingBalance.availableCents` is unsigned and
    /// `Microusd::new` refuses a negative, so without this floor `balance()`
    /// answers `AuthorityError::Decode` for exactly the organizations whose
    /// balance is trying to tell them to top up. The floor is honest rather than
    /// cosmetic: the field means *spendable now*, and an overdrawn account can
    /// spend nothing.
    #[test]
    fn the_published_available_amount_is_floored_because_the_ledger_is_not() {
        assert!(
            sql::READ_BALANCE.contains("GREATEST(coalesce((SELECT -sum(b.balance_microusd)"),
            "the available amount is floored at zero before it is decoded"
        );
        assert!(
            super::money(-1, "available_microusd").is_err(),
            "a negative would be a decode failure, which is what the floor prevents"
        );
        assert!(
            super::money(0, "available_microusd").is_ok(),
            "the floored value is representable"
        );
    }

    #[test]
    fn provider_dispatch_requires_one_immutable_complete_command() {
        assert!(sql::BIND_EFFECT_COMMAND.contains("state = 'prepared'"));
        assert!(
            sql::BIND_EFFECT_COMMAND.contains("request_json ->> 'schemaVersion') IS NULL"),
            "only the preparation marker may be replaced"
        );
        assert!(sql::READ_BOUND_EFFECT_COMMAND.contains("intent_hash = :intent_hash"));
    }

    #[test]
    fn the_readiness_probe_asserts_the_append_only_boundary() {
        assert!(sql::PROBE_ROLE.contains("finance.journal_transaction"));
        assert!(sql::PROBE_ROLE.contains("UPDATE"));
        assert!(sql::PROBE_ROLE.contains("pg_has_role"));
    }

    #[test]
    fn the_effect_table_admits_no_kind_the_ddl_does_not_declare() {
        const DDL_KINDS: [&str; 6] = [
            "customer_create",
            "checkout_session_create",
            "payment_intent_off_session",
            "refund_create",
            "tax_calculation_create",
            "tax_transaction_create",
        ];
        for kind in CommandKind::ALL {
            if let Some(durable) = durable_effect_kind(kind) {
                assert!(
                    DDL_KINDS.contains(&durable),
                    "`{durable}` is not a declared finance.provider_effect kind"
                );
            }
        }
        assert!(durable_effect_kind(CommandKind::CreatePortalSession).is_none());
        assert!(durable_effect_kind(CommandKind::LookupEffectOutcome).is_none());
    }

    #[test]
    fn the_replacement_is_fenced_by_the_revision_it_read() {
        assert!(sql::REPLACE_POLICY.contains("revision = :expect_revision"));
        assert!(sql::REPLACE_POLICY.contains("revision = revision + 1"));
    }

    #[test]
    fn the_prepared_effect_claims_its_intent_before_any_provider_call() {
        assert!(sql::PREPARE_EFFECT.contains("'prepared'"));
        assert!(sql::PREPARE_EFFECT.contains("ON CONFLICT (org_id, kind, intent_hash) DO NOTHING"));
    }
}
