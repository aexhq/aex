//! The finance sweeps. They report and fence; they never repair money (F-30).
//!
//! An auto-repairing reconciler hides the defect that caused the drift, so every
//! finding here becomes an operations record and, where the account must stop
//! moving, a fence. The one thing this worker may change is the state of an
//! effect whose outcome the provider has now told it.

use std::sync::Arc;
use std::time::Duration;

use aex_finance_domain::effect::{EffectKind, RecoveryAction};
use aex_payment_contracts::{
    CommandKind, EffectId, EffectMetadata, PaymentCommand, PaymentCommandEnvelope, PaymentResult,
    ProviderIdempotencyKey,
};
use aex_rds_data::{DataApiClient, DataApiError, DecodeError, Record, Row, SqlValue, Statement};
use aex_wire::ids::OrganizationId;
use aex_wire::types::Timestamp;
use aex_wire::{PrefixedId as _, Uuid7};
use time::OffsetDateTime;

use crate::gateway::EffectRecoveryGateway;

/// Every statement the sweeps run. All read-only except the effect transition.
pub mod sql {
    /// Proves the connection holds its own role and cannot mutate the journal.
    pub const PROBE_ROLE: &str = "\
SELECT pg_has_role(current_user, :role, 'MEMBER'), \
       has_table_privilege(:role, 'finance.journal_transaction', 'UPDATE'), \
       has_table_privilege(:role, 'finance.provider_effect', 'UPDATE'), \
       has_table_privilege(:role, 'finance.reconcile_cursor', 'INSERT,UPDATE,DELETE')";

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
SELECT pe.effect_id, pe.org_id, pe.kind, pe.state, \
       (EXTRACT(EPOCH FROM pe.first_dispatch_at)*1000)::bigint, \
       pe.provider_object_id, pe.request_json, pe.revision \
  FROM finance.provider_effect pe \
 WHERE pe.state IN ('prepared', 'dispatched', 'outcome_unknown') \
   AND pe.effect_id > coalesce((SELECT last_effect_id FROM finance.reconcile_cursor \
                                WHERE duty = 'provider_effect'), \
                               '00000000-0000-0000-0000-000000000000'::uuid) \
 ORDER BY pe.effect_id \
 LIMIT :page_limit";

    /// Advances the durable keyset cursor only after the whole page was seen.
    pub const ADVANCE_EFFECT_CURSOR: &str = "\
INSERT INTO finance.reconcile_cursor (duty, last_effect_id, updated_at) \
VALUES ('provider_effect', :effect_id, now()) \
ON CONFLICT (duty) DO UPDATE SET last_effect_id = excluded.last_effect_id, \
                                  updated_at = excluded.updated_at";

    /// Wraps the keyset cursor after reaching the end.
    pub const RESET_EFFECT_CURSOR: &str = "\
DELETE FROM finance.reconcile_cursor WHERE duty = 'provider_effect'";

    /// Escalates an effect automatic recovery can no longer resolve.
    pub const ESCALATE_EFFECT: &str = "\
UPDATE finance.provider_effect \
   SET state = 'manual_review', resolved_at = now(), revision = revision + 1 \
 WHERE effect_id = :effect_id AND revision = :revision AND state = 'outcome_unknown'";

    /// Applies a provider answer only if no webhook or concurrent sweep moved
    /// the effect after this page read it.
    pub const RESOLVE_EFFECT: &str = "\
UPDATE finance.provider_effect \
   SET state = :state, provider_object_id = coalesce(:provider_object_id, provider_object_id), \
       provider_status = :provider_status, failure_code = :failure_code, \
       decline_code = :decline_code, attempts = attempts + 1, resolved_at = now(), \
       revision = revision + 1 \
 WHERE effect_id = :effect_id AND revision = :revision \
   AND state IN ('prepared', 'dispatched', 'outcome_unknown')";
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
    /// Effects whose provider outcome this sweep resolved durably.
    pub resolved: Vec<String>,
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
    /// A recovery invoke may have reached the provider edge. The same admitted
    /// idempotency key must be retried; this is never a determinate failure.
    #[error("the provider recovery outcome is unknown: {0}")]
    RecoveryUnknown(String),
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
#[derive(Clone)]
pub struct AuroraReconcileAuthority {
    client: Arc<DataApiClient>,
    role: String,
    recovery: Arc<dyn EffectRecoveryGateway>,
}

impl std::fmt::Debug for AuroraReconcileAuthority {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("AuroraReconcileAuthority")
            .field("client", &self.client)
            .field("role", &self.role)
            .field("recovery", &"<payment-command-edge>")
            .finish()
    }
}

impl AuroraReconcileAuthority {
    /// Builds the authority over a configured Data `API` client.
    #[must_use]
    pub fn new(
        client: Arc<DataApiClient>,
        role: String,
        recovery: Arc<dyn EffectRecoveryGateway>,
    ) -> Self {
        Self {
            client,
            role,
            recovery,
        }
    }
}

/// One privilege observation from the readiness probe.
#[derive(Debug, Clone, Copy)]
struct Granted(bool);

/// The four independent privilege observations the readiness probe checks.
#[derive(Debug)]
struct GrantRow {
    holds_role: Granted,
    can_mutate_journal: Granted,
    can_resolve_effects: Granted,
    can_advance_cursor: Granted,
}

impl Row for GrantRow {
    fn from_record(record: &Record<'_>) -> Result<Self, DecodeError> {
        record.expect_arity(4)?;
        Ok(Self {
            holds_role: Granted(record.bool(0)?),
            can_mutate_journal: Granted(record.bool(1)?),
            can_resolve_effects: Granted(record.bool(2)?),
            can_advance_cursor: Granted(record.bool(3)?),
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
    organization_id: uuid::Uuid,
    kind: String,
    state: String,
    first_dispatch_millis: Option<i64>,
    provider_object_id: Option<String>,
    request_json: serde_json::Value,
    revision: i64,
}

impl Row for EffectRow {
    fn from_record(record: &Record<'_>) -> Result<Self, DecodeError> {
        record.expect_arity(8)?;
        Ok(Self {
            effect_id: record.uuid(0)?,
            organization_id: record.uuid(1)?,
            kind: record.text(2)?.to_owned(),
            state: record.text(3)?.to_owned(),
            first_dispatch_millis: record.opt(4, Record::i64)?,
            provider_object_id: record.opt(5, |r, i| r.text(i).map(str::to_owned))?,
            request_json: record.json(6)?,
            revision: record.i64(7)?,
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
    if !matches!(
        row.state.as_str(),
        "prepared" | "dispatched" | "outcome_unknown"
    ) {
        return None;
    }
    let first_dispatch_millis = row.first_dispatch_millis.or_else(|| {
        Uuid7::from_bytes(*row.effect_id.as_bytes())
            .ok()
            .and_then(|id| i64::try_from(id.unix_millis()).ok())
    })?;
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
    // The current command edge can resolve only PaymentIntents. A known object
    // id for another kind is not authority to send it through the wrong Stripe
    // API; those effects leave the automatic lane after the replay window.
    Some(if kind == EffectKind::OffSessionCharge {
        row.provider_object_id
            .as_ref()
            .map_or(RecoveryAction::SearchByEffectId, |id| {
                RecoveryAction::LookupByObject(id.clone())
            })
    } else {
        RecoveryAction::EscalateManualReview
    })
}

/// The payment command kind behind a durable effect kind.
const fn command_kind(kind: EffectKind) -> Option<CommandKind> {
    match kind {
        EffectKind::CustomerCreate => Some(CommandKind::EnsureCustomer),
        EffectKind::CheckoutSessionCreate => Some(CommandKind::CreateTopUpCheckout),
        EffectKind::OffSessionCharge => Some(CommandKind::ChargeSavedMethod),
        EffectKind::RefundCreate => Some(CommandKind::RefundCharge),
        EffectKind::TaxCalculationCreate | EffectKind::TaxTransactionCreate => None,
    }
}

/// Parses the complete command finance committed before provider dispatch.
fn admitted_envelope(row: &EffectRow) -> Option<PaymentCommandEnvelope> {
    let envelope: PaymentCommandEnvelope = serde_json::from_value(row.request_json.clone()).ok()?;
    let effect = EffectId(Uuid7::from_bytes(*row.effect_id.as_bytes()).ok()?);
    let organization =
        OrganizationId::from_uuid7(Uuid7::from_bytes(*row.organization_id.as_bytes()).ok()?);
    let expected_kind = command_kind(effect_kind(&row.kind)?)?;
    let expected_key =
        ProviderIdempotencyKey::derive(expected_kind, organization, effect).as_header_value();
    (envelope.command.effect() == effect
        && envelope.command.kind() == expected_kind
        && envelope.metadata.effect == effect
        && envelope.metadata.organization == organization
        && envelope.metadata.kind == expected_kind
        && envelope.provider_idempotency_key == expected_key.as_str())
    .then_some(envelope)
}

/// Builds a fresh, read-only lookup while preserving the original effect and
/// account attribution. Lookup never reuses a mutation command.
fn lookup_envelope(
    row: &EffectRow,
    admitted: &PaymentCommandEnvelope,
    now: OffsetDateTime,
) -> Option<PaymentCommandEnvelope> {
    let effect = EffectId(Uuid7::from_bytes(*row.effect_id.as_bytes()).ok()?);
    let organization =
        OrganizationId::from_uuid7(Uuid7::from_bytes(*row.organization_id.as_bytes()).ok()?);
    let expect = command_kind(effect_kind(&row.kind)?)?;
    let kind = CommandKind::LookupEffectOutcome;
    let deadline = Timestamp::from_unix_millis(
        i64::try_from(now.unix_timestamp_nanos().div_euclid(1_000_000)).ok()? + 10_000,
    )
    .ok()?;
    Some(PaymentCommandEnvelope {
        schema_version: admitted.schema_version,
        command: PaymentCommand::LookupEffectOutcome {
            effect,
            expect,
            provider: row
                .provider_object_id
                .clone()
                .map(aex_payment_contracts::ProviderObjectRef),
        },
        provider_idempotency_key: ProviderIdempotencyKey::derive(kind, organization, effect)
            .as_header_value()
            .as_str()
            .to_owned(),
        deadline,
        metadata: EffectMetadata {
            effect,
            organization,
            kind,
            credit: admitted.metadata.credit,
        },
    })
}

/// Gives an exact replay a fresh transport deadline without changing the
/// admitted command or the provider idempotency key.
fn replay_envelope(
    admitted: &PaymentCommandEnvelope,
    now: OffsetDateTime,
) -> Option<PaymentCommandEnvelope> {
    let mut replay = admitted.clone();
    replay.deadline = Timestamp::from_unix_millis(
        i64::try_from(now.unix_timestamp_nanos().div_euclid(1_000_000)).ok()? + 10_000,
    )
    .ok()?;
    Some(replay)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RecoveryAttempt {
    Resolved,
    StillUnknown,
    Raced,
    Stranded,
}

impl AuroraReconcileAuthority {
    async fn recover_effect(
        &self,
        row: &EffectRow,
        action: &RecoveryAction,
        now: OffsetDateTime,
    ) -> Result<RecoveryAttempt, SweepError> {
        let Some(admitted) = admitted_envelope(row) else {
            return Ok(RecoveryAttempt::Stranded);
        };
        let envelope = match action {
            RecoveryAction::RetryExactKey => replay_envelope(&admitted, now),
            RecoveryAction::LookupByObject(_) | RecoveryAction::SearchByEffectId => {
                lookup_envelope(row, &admitted, now)
            }
            RecoveryAction::EscalateManualReview => return Ok(RecoveryAttempt::Stranded),
        }
        .ok_or_else(|| SweepError::Decode("the recovery envelope is unrepresentable".to_owned()))?;
        let result = match self.recovery.execute(&envelope).await {
            Ok(result) => result,
            Err(SweepError::RecoveryUnknown(_)) => return Ok(RecoveryAttempt::StillUnknown),
            Err(error) => return Err(error),
        };
        let effect = EffectId(
            Uuid7::from_bytes(*row.effect_id.as_bytes())
                .map_err(|error| SweepError::Decode(error.to_string()))?,
        );
        if result.effect() != effect {
            return Err(SweepError::Decode(
                "the payment edge returned another effect identity".to_owned(),
            ));
        }
        let (state, provider_object_id, provider_status, failure_code, decline_code) = match result
        {
            PaymentResult::Succeeded { provider_ref, .. } => {
                ("succeeded", Some(provider_ref.0), "succeeded", None, None)
            }
            PaymentResult::Failed { failure, .. } => (
                "failed",
                None,
                "failed",
                Some(format!("{:?}", failure.class)),
                failure.decline_code.map(|code| code.as_str().to_owned()),
            ),
            PaymentResult::Unknown { .. } => return Ok(RecoveryAttempt::StillUnknown),
        };
        let updated = self
            .client
            .execute(Statement::with(
                sql::RESOLVE_EFFECT,
                vec![
                    ("state", SqlValue::Text(state.to_owned())),
                    (
                        "provider_object_id",
                        provider_object_id.map_or(SqlValue::Null, SqlValue::Text),
                    ),
                    (
                        "provider_status",
                        SqlValue::Text(provider_status.to_owned()),
                    ),
                    (
                        "failure_code",
                        failure_code.map_or(SqlValue::Null, SqlValue::Text),
                    ),
                    (
                        "decline_code",
                        decline_code.map_or(SqlValue::Null, SqlValue::Text),
                    ),
                    ("effect_id", SqlValue::Uuid(row.effect_id)),
                    ("revision", SqlValue::I64(row.revision)),
                ],
            ))
            .await
            .map_err(store)?;
        Ok(if updated == 1 {
            RecoveryAttempt::Resolved
        } else {
            RecoveryAttempt::Raced
        })
    }
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
        if !row.holds_role.0 || !row.can_resolve_effects.0 || !row.can_advance_cursor.0 {
            return Err(SweepError::Unavailable(format!(
                "`{}` cannot resolve a provider effect",
                self.role
            )));
        }
        if row.can_mutate_journal.0 {
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
                Some(action @ RecoveryAction::RetryExactKey) => {
                    match self.recover_effect(row, &action, now).await? {
                        RecoveryAttempt::Resolved => {
                            report.resolved.push(row.effect_id.to_string());
                        }
                        RecoveryAttempt::StillUnknown => {
                            report.replayable.push(row.effect_id.to_string());
                        }
                        RecoveryAttempt::Stranded => {
                            report.stranded.push(row.effect_id.to_string());
                        }
                        RecoveryAttempt::Raced => {}
                    }
                }
                Some(
                    action @ (RecoveryAction::LookupByObject(_) | RecoveryAction::SearchByEffectId),
                ) => match self.recover_effect(row, &action, now).await? {
                    RecoveryAttempt::Resolved => report.resolved.push(row.effect_id.to_string()),
                    RecoveryAttempt::StillUnknown => {
                        report.lookup_required.push(row.effect_id.to_string());
                    }
                    RecoveryAttempt::Stranded => report.stranded.push(row.effect_id.to_string()),
                    RecoveryAttempt::Raced => {}
                },
                Some(RecoveryAction::EscalateManualReview) => {
                    let updated = self
                        .client
                        .execute(Statement::with(
                            sql::ESCALATE_EFFECT,
                            vec![
                                ("effect_id", SqlValue::Uuid(row.effect_id)),
                                ("revision", SqlValue::I64(row.revision)),
                            ],
                        ))
                        .await
                        .map_err(store)?;
                    if updated == 1 {
                        report.escalated.push(row.effect_id.to_string());
                    }
                }
                None => report.stranded.push(row.effect_id.to_string()),
            }
        }
        if let Some(last) = unresolved.last() {
            self.client
                .execute(Statement::with(
                    sql::ADVANCE_EFFECT_CURSOR,
                    vec![("effect_id", SqlValue::Uuid(last.effect_id))],
                ))
                .await
                .map_err(store)?;
        } else {
            self.client
                .execute(Statement::new(sql::RESET_EFFECT_CURSOR))
                .await
                .map_err(store)?;
        }
        Ok(report)
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use aex_finance_domain::effect::RecoveryAction;
    use aex_internal_contracts::SchemaVersion;
    use aex_payment_contracts::{
        CommandKind, EffectId, EffectMetadata, PaymentCommand, PaymentCommandEnvelope,
        ProviderIdempotencyKey, RedactedEmail,
    };
    use aex_wire::ids::OrganizationId;
    use aex_wire::types::{Cents, Timestamp};
    use aex_wire::{PrefixedId as _, Uuid7};
    use time::OffsetDateTime;
    use uuid::Uuid;

    use super::{
        EffectRow, SweepReport, admitted_envelope, effect_kind, recovery, replay_envelope, sql,
    };

    fn admitted_customer_effect() -> (EffectRow, PaymentCommandEnvelope) {
        let effect_uuid = Uuid7::compose(1, [1; 10]);
        let organization_uuid = Uuid7::compose(1, [2; 10]);
        let effect = EffectId(effect_uuid);
        let organization = OrganizationId::from_uuid7(organization_uuid);
        let kind = CommandKind::EnsureCustomer;
        let envelope = PaymentCommandEnvelope {
            schema_version: SchemaVersion::V1,
            command: PaymentCommand::EnsureCustomer {
                effect,
                organization,
                email: RedactedEmail::new("redacted@example.com"),
            },
            provider_idempotency_key: ProviderIdempotencyKey::derive(kind, organization, effect)
                .as_header_value()
                .as_str()
                .to_owned(),
            deadline: Timestamp::from_unix_millis(10_000).expect("timestamp"),
            metadata: EffectMetadata {
                effect,
                organization,
                kind,
                credit: Cents::ZERO,
            },
        };
        let row = EffectRow {
            effect_id: Uuid::from_bytes(*effect_uuid.as_bytes()),
            organization_id: Uuid::from_bytes(*organization_uuid.as_bytes()),
            kind: "customer_create".to_owned(),
            state: "outcome_unknown".to_owned(),
            first_dispatch_millis: Some(0),
            provider_object_id: None,
            request_json: serde_json::to_value(&envelope).expect("envelope JSON"),
            revision: 3,
        };
        (row, envelope)
    }

    #[test]
    fn every_sweep_statement_binds_and_never_touches_floating_point() {
        for statement in [
            sql::PROBE_ROLE,
            sql::CONSERVATION_BY_ACCOUNT,
            sql::CONSERVATION_GLOBAL,
            sql::UNRESOLVED_EFFECTS,
            sql::ESCALATE_EFFECT,
            sql::RESOLVE_EFFECT,
            sql::ADVANCE_EFFECT_CURSOR,
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
            sql::RESOLVE_EFFECT,
            sql::ADVANCE_EFFECT_CURSOR,
            sql::RESET_EFFECT_CURSOR,
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
        assert!(sql::ESCALATE_EFFECT.contains("revision = :revision"));
    }

    #[test]
    fn provider_resolution_is_an_optimistic_transition_and_pages_cannot_starve() {
        assert!(sql::RESOLVE_EFFECT.contains("revision = :revision"));
        assert!(
            sql::RESOLVE_EFFECT.contains("state IN ('prepared', 'dispatched', 'outcome_unknown')")
        );
        assert!(sql::UNRESOLVED_EFFECTS.contains("pe.effect_id > coalesce"));
        assert!(sql::ADVANCE_EFFECT_CURSOR.contains("ON CONFLICT (duty) DO UPDATE"));
        assert!(sql::RESET_EFFECT_CURSOR.contains("duty = 'provider_effect'"));
    }

    #[test]
    fn replay_accepts_only_the_original_effect_account_kind_and_provider_key() {
        let (row, envelope) = admitted_customer_effect();
        assert_eq!(admitted_envelope(&row), Some(envelope.clone()));

        let mut corrupt = row;
        corrupt.request_json["providerIdempotencyKey"] = serde_json::json!("attacker-key");
        assert!(admitted_envelope(&corrupt).is_none());

        let replay = replay_envelope(
            &envelope,
            OffsetDateTime::UNIX_EPOCH + time::Duration::seconds(20),
        )
        .expect("a fresh deadline is representable");
        assert_eq!(replay.command, envelope.command);
        assert_eq!(
            replay.provider_idempotency_key,
            envelope.provider_idempotency_key
        );
        assert_ne!(replay.deadline, envelope.deadline);
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
            organization_id: Uuid::nil(),
            kind: "payment_intent_off_session".to_owned(),
            state: "outcome_unknown".to_owned(),
            first_dispatch_millis: Some(0),
            provider_object_id: None,
            request_json: serde_json::Value::Null,
            revision: 0,
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
            organization_id: Uuid::nil(),
            kind: "payment_intent_off_session".to_owned(),
            state: "outcome_unknown".to_owned(),
            first_dispatch_millis: Some(0),
            provider_object_id: None,
            request_json: serde_json::Value::Null,
            revision: 0,
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
    fn an_effect_without_any_valid_timing_authority_is_not_blindly_replayed() {
        let row = EffectRow {
            effect_id: Uuid::new_v4(),
            organization_id: Uuid::nil(),
            kind: "payment_intent_off_session".to_owned(),
            state: "outcome_unknown".to_owned(),
            first_dispatch_millis: None,
            provider_object_id: None,
            request_json: serde_json::Value::Null,
            revision: 0,
        };
        assert_eq!(
            recovery(&row, OffsetDateTime::UNIX_EPOCH, Duration::from_hours(12)),
            None,
            "missing timing authority becomes a stranded finding, never an exact-key replay"
        );
    }
}
