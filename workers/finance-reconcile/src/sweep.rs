//! The finance sweeps. They report and fence; they never repair money (F-30).
//!
//! An auto-repairing reconciler hides the defect that caused the drift, so every
//! finding here becomes an operations record and, where the account must stop
//! moving, a fence. The one thing this worker may change is the state of an
//! effect whose outcome the provider has now told it.

use std::sync::Arc;
use std::time::Duration;

use aex_finance_domain::effect::{EffectKind, RecoveryAction};
use aex_rds_data::{DataApiClient, DataApiError, DecodeError, Record, Row, SqlValue, Statement};
use time::OffsetDateTime;

/// Every statement the sweeps run. All read-only except the effect transition.
pub mod sql {
    /// Proves the connection holds its own role and cannot mutate the journal.
    pub const PROBE_ROLE: &str = "\
SELECT pg_has_role(current_user, :role, 'MEMBER'), \
       has_table_privilege(:role, 'finance.journal_transaction', 'UPDATE'), \
       has_table_privilege(:role, 'finance.provider_effect', 'UPDATE')";

    /// R-BAL: the projection must equal the journal, per account.
    pub const CONSERVATION_BY_ACCOUNT: &str = "\
SELECT b.account_id, b.balance_microusd, coalesce(j.s, 0)::bigint \
  FROM finance.account_balance b \
  LEFT JOIN (SELECT account_id, sum(amount_microusd) AS s \
               FROM finance.journal_posting GROUP BY account_id) j \
    ON j.account_id = b.account_id \
 WHERE b.balance_microusd IS DISTINCT FROM coalesce(j.s, 0) \
 LIMIT :page_limit";

    /// R-SUM: the global law.
    pub const CONSERVATION_GLOBAL: &str = "\
SELECT coalesce(sum(amount_microusd), 0)::bigint FROM finance.journal_posting";

    /// Effects whose outcome the provider never confirmed.
    pub const UNRESOLVED_EFFECTS: &str = "\
SELECT pe.effect_id, pe.kind, pe.state, \
       (EXTRACT(EPOCH FROM pe.first_dispatch_at)*1000)::bigint, \
       pe.provider_object_id \
  FROM finance.provider_effect pe \
 WHERE pe.state IN ('prepared', 'dispatched', 'outcome_unknown') \
 ORDER BY pe.effect_id \
 LIMIT :page_limit";

    /// Escalates an effect automatic recovery can no longer resolve.
    pub const ESCALATE_EFFECT: &str = "\
UPDATE finance.provider_effect \
   SET state = 'manual_review', resolved_at = now(), revision = revision + 1 \
 WHERE effect_id = :effect_id AND state = 'outcome_unknown'";
}

/// What one sweep found.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SweepReport {
    /// Accounts whose projection disagrees with the journal.
    pub divergent_accounts: Vec<String>,
    /// The global signed posting sum, which must be zero.
    pub global_imbalance_microusd: i64,
    /// Effects that may be recovered by replaying the exact idempotency key.
    pub replayable: Vec<String>,
    /// Effects that must be resolved by looking the provider object up.
    pub lookup_required: Vec<String>,
    /// Effects that never reached `outcome_unknown` and cannot be replayed from
    /// the incomplete durable request projection.
    pub stranded: Vec<String>,
    /// Effects escalated to an operator.
    pub escalated: Vec<String>,
}

impl SweepReport {
    /// Whether the sweep found anything an operator must see.
    #[must_use]
    pub fn has_findings(&self) -> bool {
        !self.divergent_accounts.is_empty()
            || self.global_imbalance_microusd != 0
            || !self.replayable.is_empty()
            || !self.lookup_required.is_empty()
            || !self.stranded.is_empty()
            || !self.escalated.is_empty()
    }
}

/// Why a sweep did not complete.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SweepError {
    /// The authority is unreachable.
    #[error("the finance authority is unavailable: {0}")]
    Unavailable(String),
    /// The stored row does not match what this deployable projects.
    #[error("the finance authority returned an undecodable row: {0}")]
    Decode(String),
}

/// Reporting a finding to the operations topic.
#[async_trait::async_trait]
pub trait OperationsAlarm: Send + Sync + 'static {
    /// Publishes one finding. A failure to publish is itself a failure.
    async fn report(&self, subject: &str, body: &str) -> Result<(), SweepError>;
}

/// The SNS implementation.
#[derive(Debug, Clone)]
pub struct SnsOperationsAlarm {
    client: aws_sdk_sns::Client,
    topic_arn: String,
}

impl SnsOperationsAlarm {
    /// Builds the publisher for one topic.
    #[must_use]
    pub const fn new(client: aws_sdk_sns::Client, topic_arn: String) -> Self {
        Self { client, topic_arn }
    }
}

#[async_trait::async_trait]
impl OperationsAlarm for SnsOperationsAlarm {
    async fn report(&self, subject: &str, body: &str) -> Result<(), SweepError> {
        self.client
            .publish()
            .topic_arn(&self.topic_arn)
            .subject(subject)
            .message(body)
            .send()
            .await
            .map(|_| ())
            .map_err(|error| {
                SweepError::Unavailable(aws_sdk_sns::error::DisplayErrorContext(&error).to_string())
            })
    }
}

/// Running the sweeps.
#[async_trait::async_trait]
pub trait ReconcileAuthority: Send + Sync + 'static {
    /// Proves this deployable can reach the database as its own role.
    async fn probe_role(&self) -> Result<(), SweepError>;

    /// Runs the conservation and unresolved-effect sweeps.
    async fn sweep(
        &self,
        page_limit: u32,
        retry_window: Duration,
        now: OffsetDateTime,
    ) -> Result<SweepReport, SweepError>;
}

/// The Aurora-backed reconcile authority.
#[derive(Debug, Clone)]
pub struct AuroraReconcileAuthority {
    client: Arc<DataApiClient>,
    role: String,
}

impl AuroraReconcileAuthority {
    /// Builds the authority over a configured Data `API` client.
    #[must_use]
    pub fn new(client: Arc<DataApiClient>, role: String) -> Self {
        Self { client, role }
    }
}

/// The three booleans the readiness probe checks.
#[derive(Debug)]
struct GrantRow {
    holds_role: bool,
    can_mutate_journal: bool,
    can_resolve_effects: bool,
}

impl Row for GrantRow {
    fn from_record(record: &Record<'_>) -> Result<Self, DecodeError> {
        record.expect_arity(3)?;
        Ok(Self {
            holds_role: record.bool(0)?,
            can_mutate_journal: record.bool(1)?,
            can_resolve_effects: record.bool(2)?,
        })
    }
}

/// One divergent account.
#[derive(Debug)]
struct DivergenceRow {
    account_id: uuid::Uuid,
}

impl Row for DivergenceRow {
    fn from_record(record: &Record<'_>) -> Result<Self, DecodeError> {
        record.expect_arity(3)?;
        let account_id = record.uuid(0)?;
        record.i64(1)?;
        record.i64(2)?;
        Ok(Self { account_id })
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

/// One unresolved effect.
#[derive(Debug)]
struct EffectRow {
    effect_id: uuid::Uuid,
    kind: String,
    state: String,
    first_dispatch_millis: Option<i64>,
    provider_object_id: Option<String>,
}

impl Row for EffectRow {
    fn from_record(record: &Record<'_>) -> Result<Self, DecodeError> {
        record.expect_arity(5)?;
        Ok(Self {
            effect_id: record.uuid(0)?,
            kind: record.text(1)?.to_owned(),
            state: record.text(2)?.to_owned(),
            first_dispatch_millis: record.opt(3, Record::i64)?,
            provider_object_id: record.opt(4, |r, i| r.text(i).map(str::to_owned))?,
        })
    }
}

/// Classifies a transport failure.
fn store(error: DataApiError) -> SweepError {
    match error {
        DataApiError::Decode(decode) => SweepError::Decode(decode.to_string()),
        other => SweepError::Unavailable(other.to_string()),
    }
}

/// The domain effect kind behind a durable spelling.
fn effect_kind(raw: &str) -> Option<EffectKind> {
    match raw {
        "customer_create" => Some(EffectKind::CustomerCreate),
        "checkout_session_create" => Some(EffectKind::CheckoutSessionCreate),
        "payment_intent_off_session" => Some(EffectKind::OffSessionCharge),
        "refund_create" => Some(EffectKind::RefundCreate),
        "tax_calculation_create" => Some(EffectKind::TaxCalculationCreate),
        "tax_transaction_create" => Some(EffectKind::TaxTransactionCreate),
        _ => None,
    }
}

/// Rebuilds enough of the domain aggregate to ask it what recovery is allowed.
fn recovery(
    row: &EffectRow,
    now: OffsetDateTime,
    retry_window: Duration,
) -> Option<RecoveryAction> {
    let kind = effect_kind(&row.kind)?;
    if row.state != "outcome_unknown" {
        return None;
    }
    let first_dispatch_millis = row.first_dispatch_millis?;
    let dispatched =
        OffsetDateTime::from_unix_timestamp_nanos(i128::from(first_dispatch_millis) * 1_000_000)
            .ok()?;
    if now < dispatched {
        return Some(RecoveryAction::EscalateManualReview);
    }
    let elapsed_millis = (now - dispatched).whole_milliseconds();
    if elapsed_millis < i128::try_from(retry_window.as_millis()).unwrap_or(i128::MAX) {
        return Some(RecoveryAction::RetryExactKey);
    }
    if let Some(provider_object_id) = &row.provider_object_id {
        return Some(RecoveryAction::LookupByObject(provider_object_id.clone()));
    }
    // The current command edge can search only PaymentIntents. Returning a
    // search action for another kind would claim an automatic recovery path
    // that does not exist.
    Some(if kind == EffectKind::OffSessionCharge {
        RecoveryAction::SearchByEffectId
    } else {
        RecoveryAction::EscalateManualReview
    })
}

#[async_trait::async_trait]
impl ReconcileAuthority for AuroraReconcileAuthority {
    async fn probe_role(&self) -> Result<(), SweepError> {
        let row: GrantRow = self
            .client
            .query_one(Statement::with(
                sql::PROBE_ROLE,
                vec![("role", SqlValue::Text(self.role.clone()))],
            ))
            .await
            .map_err(store)?;
        if !row.holds_role || !row.can_resolve_effects {
            return Err(SweepError::Unavailable(format!(
                "`{}` cannot resolve a provider effect",
                self.role
            )));
        }
        if row.can_mutate_journal {
            return Err(SweepError::Unavailable(format!(
                "`{}` holds UPDATE on finance.journal_transaction; the sweeps report and fence, \
                 they never repair money",
                self.role
            )));
        }
        Ok(())
    }

    async fn sweep(
        &self,
        page_limit: u32,
        retry_window: Duration,
        now: OffsetDateTime,
    ) -> Result<SweepReport, SweepError> {
        let mut report = SweepReport::default();
        let divergent: Vec<DivergenceRow> = self
            .client
            .query(Statement::with(
                sql::CONSERVATION_BY_ACCOUNT,
                vec![("page_limit", SqlValue::I64(i64::from(page_limit)))],
            ))
            .await
            .map_err(store)?;
        report.divergent_accounts = divergent
            .into_iter()
            .map(|row| row.account_id.to_string())
            .collect();

        let global: AmountRow = self
            .client
            .query_one(Statement::new(sql::CONSERVATION_GLOBAL))
            .await
            .map_err(store)?;
        report.global_imbalance_microusd = global.0;

        let unresolved: Vec<EffectRow> = self
            .client
            .query(Statement::with(
                sql::UNRESOLVED_EFFECTS,
                vec![("page_limit", SqlValue::I64(i64::from(page_limit)))],
            ))
            .await
            .map_err(store)?;
        for row in &unresolved {
            match recovery(row, now, retry_window) {
                Some(RecoveryAction::RetryExactKey) => {
                    report.replayable.push(row.effect_id.to_string());
                }
                Some(RecoveryAction::LookupByObject(_) | RecoveryAction::SearchByEffectId) => {
                    report.lookup_required.push(row.effect_id.to_string());
                }
                Some(RecoveryAction::EscalateManualReview) => {
                    self.client
                        .execute(Statement::with(
                            sql::ESCALATE_EFFECT,
                            vec![("effect_id", SqlValue::Uuid(row.effect_id))],
                        ))
                        .await
                        .map_err(store)?;
                    report.escalated.push(row.effect_id.to_string());
                }
                None => report.stranded.push(row.effect_id.to_string()),
            }
        }
        Ok(report)
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use aex_finance_domain::effect::RecoveryAction;
    use time::OffsetDateTime;
    use uuid::Uuid;

    use super::{EffectRow, SweepReport, effect_kind, recovery, sql};

    #[test]
    fn every_sweep_statement_binds_and_never_touches_floating_point() {
        for statement in [
            sql::PROBE_ROLE,
            sql::CONSERVATION_BY_ACCOUNT,
            sql::CONSERVATION_GLOBAL,
            sql::UNRESOLVED_EFFECTS,
            sql::ESCALATE_EFFECT,
        ] {
            assert!(!statement.to_lowercase().contains("float"));
            assert!(!statement.to_lowercase().contains("double"));
        }
    }

    #[test]
    fn the_sweeps_touch_no_journal_row() {
        for statement in [
            sql::CONSERVATION_BY_ACCOUNT,
            sql::CONSERVATION_GLOBAL,
            sql::UNRESOLVED_EFFECTS,
            sql::ESCALATE_EFFECT,
        ] {
            let upper = statement.to_uppercase();
            assert!(
                !upper.contains("UPDATE FINANCE.JOURNAL")
                    && !upper.contains("DELETE FROM FINANCE.JOURNAL")
                    && !upper.contains("INSERT INTO FINANCE.JOURNAL"),
                "F-30: a sweep reports and fences; it never repairs money"
            );
        }
    }

    #[test]
    fn an_escalation_only_moves_an_effect_that_is_already_unknown() {
        assert!(sql::ESCALATE_EFFECT.contains("state = 'outcome_unknown'"));
        assert!(sql::ESCALATE_EFFECT.contains("'manual_review'"));
    }

    #[test]
    fn every_durable_effect_kind_the_ddl_admits_maps_to_the_domain() {
        let ddl = include_str!("../../../migrations/central/20260801000500_baseline_finance.sql");
        for kind in [
            "customer_create",
            "checkout_session_create",
            "payment_intent_off_session",
            "refund_create",
            "tax_calculation_create",
            "tax_transaction_create",
        ] {
            assert!(ddl.contains(&format!("'{kind}'")), "{kind} is not declared");
            assert!(effect_kind(kind).is_some(), "{kind} has no domain arm");
        }
        assert!(effect_kind("invoice_finalize").is_none());
    }

    #[test]
    fn a_clean_sweep_reports_nothing_and_a_dirty_one_does() {
        let clean = SweepReport::default();
        assert!(!clean.has_findings());
        let imbalanced = SweepReport {
            global_imbalance_microusd: 1,
            ..SweepReport::default()
        };
        assert!(imbalanced.has_findings());
        let divergent = SweepReport {
            divergent_accounts: vec!["a".to_owned()],
            ..SweepReport::default()
        };
        assert!(divergent.has_findings());
        let replay_only = SweepReport {
            replayable: vec!["e".to_owned()],
            ..SweepReport::default()
        };
        assert!(
            replay_only.has_findings(),
            "until exact request replay is composed, a replayable effect must not be silent"
        );
    }

    #[test]
    fn recovery_obeys_the_configured_window_instead_of_a_hidden_twelve_hours() {
        let row = EffectRow {
            effect_id: Uuid::now_v7(),
            kind: "payment_intent_off_session".to_owned(),
            state: "outcome_unknown".to_owned(),
            first_dispatch_millis: Some(0),
            provider_object_id: None,
        };
        let now = OffsetDateTime::UNIX_EPOCH + time::Duration::hours(2);
        assert_eq!(
            recovery(&row, now, Duration::from_hours(1)),
            Some(RecoveryAction::SearchByEffectId),
            "a one-hour deployment window must not replay a two-hour-old charge"
        );
        assert_eq!(
            recovery(&row, now, Duration::from_hours(3)),
            Some(RecoveryAction::RetryExactKey)
        );
    }

    #[test]
    fn only_the_effect_kind_the_edge_can_search_gets_an_automatic_search_action() {
        let base = EffectRow {
            effect_id: Uuid::now_v7(),
            kind: "payment_intent_off_session".to_owned(),
            state: "outcome_unknown".to_owned(),
            first_dispatch_millis: Some(0),
            provider_object_id: None,
        };
        let now = OffsetDateTime::UNIX_EPOCH + time::Duration::hours(13);
        assert_eq!(
            recovery(&base, now, Duration::from_hours(12)),
            Some(RecoveryAction::SearchByEffectId)
        );
        let checkout = EffectRow {
            kind: "checkout_session_create".to_owned(),
            ..base
        };
        assert_eq!(
            recovery(&checkout, now, Duration::from_hours(12)),
            Some(RecoveryAction::EscalateManualReview)
        );
    }

    #[test]
    fn an_unknown_effect_without_a_dispatch_instant_is_not_blindly_replayed() {
        let row = EffectRow {
            effect_id: Uuid::now_v7(),
            kind: "payment_intent_off_session".to_owned(),
            state: "outcome_unknown".to_owned(),
            first_dispatch_millis: None,
            provider_object_id: None,
        };
        assert_eq!(
            recovery(&row, OffsetDateTime::UNIX_EPOCH, Duration::from_hours(12)),
            None,
            "missing timing authority becomes a stranded finding, never an exact-key replay"
        );
    }
}
