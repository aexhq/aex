//! Settling one account group in one serializable transaction.
//!
//! Every fact of one organization is claimed inside a single transaction. A
//! delivery is not complete until that same authority has also rated, posted
//! and written its receipt. The missing rating authorities are recorded in the
//! central-finance handoff; until they land, `pending` is returned explicitly
//! so the handler keeps the message in the SQS partial-batch response. No
//! provider call, sleep, queue operation or object write happens inside the
//! transaction.

use std::sync::Arc;

use aex_internal_contracts::usage::UsageFact;
use aex_rds_data::{
    CommitFailure, DataApiClient, DataApiError, DecodeError, Isolation, Record, Row, SqlValue,
    Statement, Transaction,
};
use aex_wire::PrefixedId as _;
use aex_wire::ids::OrganizationId;

use crate::backlog::Delivered;

/// Every statement this deployable runs.
pub mod sql {
    /// Proves the connection holds its own role and cannot mutate the journal.
    pub const PROBE_ROLE: &str = "\
SELECT pg_has_role(current_user, :role, 'MEMBER'), \
       has_table_privilege(:role, 'finance.journal_transaction', 'UPDATE'), \
       has_table_privilege(:role, 'finance.usage_inbox', 'INSERT')";

    /// Claims one regional fact. The composite primary key is the authority.
    ///
    /// A fact that is already stored returns no row, and the caller reads it
    /// back rather than posting a second time.
    pub const CLAIM_FACT: &str = "\
INSERT INTO finance.usage_inbox \
  (region, category, fact_id, intent_hash, org_id, workspace_id, reservation_id, meter, basis, \
   quantity, interval_start, interval_end, pricing_version, corrects_fact_id, accepted_sequence, \
   state) \
SELECT :region, :category, :fact_id, :intent_hash, :org_id, :workspace_id, r.reservation_id, \
       :meter, :basis, :quantity, \
       (TIMESTAMPTZ 'epoch' + (:interval_start_ms) * INTERVAL '1 millisecond'), \
       (TIMESTAMPTZ 'epoch' + (:interval_end_ms) * INTERVAL '1 millisecond'), \
       r.pricing_version, :corrects_fact_id, :accepted_sequence, 'pending' \
  FROM finance.reservation r \
 WHERE r.org_id = :org_id AND r.workspace_id = :workspace_id AND r.region = :region \
   AND r.scope_kind = :scope_kind AND r.scope_id = :scope_id \
   AND r.pricing_version = :pricing_version AND r.state IN ('open', 'closing') \
ON CONFLICT (region, category, fact_id) DO NOTHING \
RETURNING fact_id, intent_hash, state, rated_microusd, transaction_id";

    /// Reads back a fact a duplicate delivery already stored.
    pub const READ_FACT: &str = "\
SELECT fact_id, intent_hash, state, rated_microusd, transaction_id FROM finance.usage_inbox \
 WHERE region = :region AND category = :category AND fact_id = :fact_id";

    /// Quarantines a fact whose identity is committed under a different intent.
    pub const QUARANTINE_FACT: &str = "\
UPDATE finance.usage_inbox SET state = 'quarantined' \
 WHERE region = :region AND category = :category AND fact_id = :fact_id \
   AND state = 'pending'";
}

/// What settling one account group produced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GroupOutcome {
    /// The account this outcome is about.
    pub organization: OrganizationId,
    /// How many facts this invocation made durable for the first time.
    pub claimed: usize,
    /// How many facts a duplicate delivery had already stored.
    pub duplicates: usize,
    /// Facts that are durable but have not been rated and posted.
    ///
    /// A group containing one of these must remain in the SQS partial-batch
    /// response. A durable claim is not a settlement receipt.
    pub pending: usize,
    /// The facts whose identity is committed under a different intent.
    pub quarantined: Vec<String>,
}

/// Why an account group did not commit.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SettleError {
    /// The whole transaction lost a serialization race. It is safe to retry.
    #[error("the settlement transaction lost a serialization race")]
    Serialization,
    /// The fact names a reservation this account does not hold.
    #[error(
        "fact `{0}` names no open reservation with matching organization, workspace, region and pricing version"
    )]
    UnknownReservation(String),
    /// The authority is unreachable. Nothing was applied.
    #[error("the finance authority is unavailable: {0}")]
    Unavailable(String),
    /// The commit response was lost. The group may or may not be durable.
    #[error("the settlement commit outcome is unknown: {0}")]
    OutcomeUnknown(String),
    /// The stored row does not match what this deployable projects.
    #[error("the finance authority returned an undecodable row: {0}")]
    Decode(String),
}

impl SettleError {
    /// Whether an identical retry inside the same invocation is safe.
    ///
    /// Only a serialization failure is: the transaction provably left no trace,
    /// and the whole body is replay-safe with no external effect. An unknown
    /// commit outcome is deliberately **not** retryable here — the message is
    /// returned to the queue so redelivery resolves it against durable state.
    #[must_use]
    pub const fn is_retryable(&self) -> bool {
        matches!(self, Self::Serialization)
    }
}

/// Settling one account group.
#[async_trait::async_trait]
pub trait SettlementAuthority: Send + Sync + 'static {
    /// Proves this deployable can reach the database as its own role.
    async fn probe_role(&self) -> Result<(), SettleError>;

    /// Settles one account group in one serializable transaction.
    async fn settle(
        &self,
        organization: OrganizationId,
        messages: &[Delivered],
    ) -> Result<GroupOutcome, SettleError>;
}

/// The Aurora-backed settlement authority.
#[derive(Debug, Clone)]
pub struct AuroraSettlementAuthority {
    client: Arc<DataApiClient>,
    role: String,
}

impl AuroraSettlementAuthority {
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
struct FactRow {
    intent_hash: [u8; 32],
    state: FactState,
}

/// The closed inbox state this worker is allowed to observe.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FactState {
    Pending,
    Rated,
    Quarantined,
}

impl Row for FactRow {
    fn from_record(record: &Record<'_>) -> Result<Self, DecodeError> {
        record.expect_arity(5)?;
        record.text(0)?;
        let state = match record.text(2)? {
            "pending" => FactState::Pending,
            "rated" => FactState::Rated,
            "quarantined" => FactState::Quarantined,
            _ => {
                return Err(DecodeError::TypeMismatch {
                    index: 2,
                    expected: "a closed usage inbox state",
                });
            }
        };
        let rated_microusd = record.opt(3, Record::i64)?;
        let transaction_id = record.opt(4, Record::uuid)?;
        let columns_match_state = match state {
            FactState::Rated => rated_microusd.is_some() && transaction_id.is_some(),
            FactState::Pending | FactState::Quarantined => {
                rated_microusd.is_none() && transaction_id.is_none()
            }
        };
        if !columns_match_state {
            return Err(DecodeError::TypeMismatch {
                index: 2,
                expected: "an inbox state with matching rating and transaction columns",
            });
        }
        Ok(Self {
            intent_hash: record.fixed::<32>(1)?,
            state,
        })
    }
}

/// Classifies a transport failure without inventing an outcome.
fn store(error: DataApiError) -> SettleError {
    match error {
        DataApiError::Serialization | DataApiError::Deadlock => SettleError::Serialization,
        DataApiError::Decode(decode) => SettleError::Decode(decode.to_string()),
        other => SettleError::Unavailable(other.to_string()),
    }
}

/// Classifies a commit failure. Only an explicit rollback is a clean failure.
fn commit(failure: CommitFailure) -> SettleError {
    match failure {
        CommitFailure::RolledBack(DataApiError::Serialization | DataApiError::Deadlock) => {
            SettleError::Serialization
        }
        CommitFailure::RolledBack(error) => SettleError::Unavailable(error.to_string()),
        CommitFailure::Unknown(error) => SettleError::OutcomeUnknown(error.to_string()),
    }
}

/// The organization identity as the `uuid` column stores it.
fn org_uuid(organization: OrganizationId) -> uuid::Uuid {
    uuid::Uuid::from_bytes(*organization.uuid7().as_bytes())
}

/// The rating category one meter belongs to.
///
/// Delegates to the contract's own mapping: the same spelling appears in the
/// inbox primary key and the producer's FIFO grammar, and a local copy is a
/// second settlement authority waiting to drift.
#[must_use]
pub const fn category(meter: aex_internal_contracts::usage::Meter) -> &'static str {
    meter.category()
}

/// The reservation scope one fact's authority names.
#[must_use]
pub const fn scope_kind(authority: aex_internal_contracts::usage::AuthorityKind) -> &'static str {
    use aex_internal_contracts::usage::AuthorityKind;
    match authority {
        // The three authorities all attribute to a run-scoped reservation; the
        // scope identity is the authority's own id.
        AuthorityKind::Storage | AuthorityKind::Compute | AuthorityKind::Transfer => "run",
    }
}

/// The half-open service interval of one fact, in epoch milliseconds.
#[must_use]
pub fn interval(fact: &UsageFact) -> (i64, i64) {
    use aex_internal_contracts::usage::ServiceTime;
    match fact.service_time {
        ServiceTime::Instant { at } => (at.unix_millis(), at.unix_millis()),
        ServiceTime::Interval { start, end } => (start.unix_millis(), end.unix_millis()),
    }
}

#[async_trait::async_trait]
impl SettlementAuthority for AuroraSettlementAuthority {
    async fn probe_role(&self) -> Result<(), SettleError> {
        let row: GrantRow = self
            .client
            .query_one(Statement::with(
                sql::PROBE_ROLE,
                vec![("role", SqlValue::Text(self.role.clone()))],
            ))
            .await
            .map_err(store)?;
        if !row.holds_role || !row.can_append_inbox {
            return Err(SettleError::Unavailable(format!(
                "`{}` cannot append to the usage inbox",
                self.role
            )));
        }
        if row.can_mutate_journal {
            return Err(SettleError::Unavailable(format!(
                "`{}` holds UPDATE on finance.journal_transaction; history is append-only",
                self.role
            )));
        }
        Ok(())
    }

    async fn settle(
        &self,
        organization: OrganizationId,
        messages: &[Delivered],
    ) -> Result<GroupOutcome, SettleError> {
        let mut transaction = self
            .client
            .begin(Isolation::Serializable)
            .await
            .map_err(store)?;
        let outcome = claim_all(&mut transaction, organization, messages).await;
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

/// Claims every fact of one account inside the open transaction and reports
/// which claims still lack a settlement receipt.
async fn claim_all(
    transaction: &mut Transaction<'_>,
    organization: OrganizationId,
    messages: &[Delivered],
) -> Result<GroupOutcome, SettleError> {
    let mut outcome = GroupOutcome {
        organization,
        claimed: 0,
        duplicates: 0,
        pending: 0,
        quarantined: Vec::new(),
    };
    for message in messages {
        let fact = &message.request.fact;
        let fact_id = fact.fact_id.to_string();
        let category = category(fact.meter);
        let (interval_start_ms, interval_end_ms) = interval(fact);
        let claimed: Vec<FactRow> = transaction
            .query(Statement::with(
                sql::CLAIM_FACT,
                vec![
                    ("region", SqlValue::Text(fact.region.as_str().to_owned())),
                    ("category", SqlValue::Text(category.to_owned())),
                    ("fact_id", SqlValue::Text(fact_id.clone())),
                    (
                        "intent_hash",
                        SqlValue::Bytes(message.request.intent_hash.as_bytes().to_vec()),
                    ),
                    ("org_id", SqlValue::Uuid(org_uuid(organization))),
                    (
                        "workspace_id",
                        SqlValue::Uuid(uuid::Uuid::from_bytes(*fact.workspace.uuid7().as_bytes())),
                    ),
                    ("meter", SqlValue::Text(fact.meter.as_str().to_owned())),
                    ("basis", SqlValue::Text(basis(fact).to_owned())),
                    (
                        "quantity",
                        SqlValue::NumericText(fact.quantity.get().to_string()),
                    ),
                    ("interval_start_ms", SqlValue::I64(interval_start_ms)),
                    ("interval_end_ms", SqlValue::I64(interval_end_ms)),
                    ("corrects_fact_id", SqlValue::Null),
                    ("accepted_sequence", SqlValue::I64(accepted_sequence(fact))),
                    (
                        "scope_kind",
                        SqlValue::Text(scope_kind(fact.authority.kind).to_owned()),
                    ),
                    (
                        "scope_id",
                        SqlValue::Text(fact.authority.authority_id.to_string()),
                    ),
                    (
                        "pricing_version",
                        SqlValue::Text(fact.pricing_version.0.clone()),
                    ),
                ],
            ))
            .await
            .map_err(store)?;
        if claimed.is_empty() {
            let stored: Option<FactRow> = transaction
                .query_opt(Statement::with(
                    sql::READ_FACT,
                    vec![
                        ("region", SqlValue::Text(fact.region.as_str().to_owned())),
                        ("category", SqlValue::Text(category.to_owned())),
                        ("fact_id", SqlValue::Text(fact_id.clone())),
                    ],
                ))
                .await
                .map_err(store)?;
            match stored {
                // The insert selected no reservation, so the fact names one this
                // account does not hold. Guessing a reservation would rate
                // against the wrong pricing context.
                None => return Err(SettleError::UnknownReservation(fact_id)),
                Some(stored) if stored.intent_hash == *message.request.intent_hash.as_bytes() => {
                    outcome.duplicates += 1;
                    match stored.state {
                        FactState::Pending => outcome.pending += 1,
                        FactState::Rated => {}
                        FactState::Quarantined => outcome.quarantined.push(fact_id),
                    }
                }
                Some(_) => {
                    transaction
                        .execute(Statement::with(
                            sql::QUARANTINE_FACT,
                            vec![
                                ("region", SqlValue::Text(fact.region.as_str().to_owned())),
                                ("category", SqlValue::Text(category.to_owned())),
                                ("fact_id", SqlValue::Text(fact_id.clone())),
                            ],
                        ))
                        .await
                        .map_err(store)?;
                    outcome.quarantined.push(fact_id);
                }
            }
        } else {
            outcome.claimed += 1;
            // This composition pass has made the inbox claim durable, but it
            // has not loaded a trusted rate context or posted a receipt. Keep
            // the queue delivery live until those authorities exist; acking a
            // `pending` row would lose the only scheduled wake-up.
            outcome.pending += 1;
        }
    }
    Ok(outcome)
}

/// The durable basis spelling of one fact.
const fn basis(fact: &UsageFact) -> &'static str {
    use aex_internal_contracts::usage::FactBasis;
    match fact.basis {
        FactBasis::Consumed => "consumed",
        FactBasis::Reserved => "reserved",
    }
}

/// The contiguous per-authority sequence of one fact.
fn accepted_sequence(fact: &UsageFact) -> i64 {
    i64::try_from(fact.authority.segment_ordinal.get()).unwrap_or(i64::MAX)
}

#[cfg(test)]
mod tests {
    use aex_internal_contracts::usage::{AuthorityKind, Meter};

    use super::{SettleError, category, scope_kind, sql};

    #[test]
    fn every_statement_binds_and_never_touches_floating_point() {
        for statement in [
            sql::PROBE_ROLE,
            sql::CLAIM_FACT,
            sql::READ_FACT,
            sql::QUARANTINE_FACT,
        ] {
            assert!(statement.contains(':'), "{statement} binds no parameter");
            assert!(!statement.to_lowercase().contains("float"));
            assert!(!statement.to_lowercase().contains("double"));
        }
    }

    #[test]
    fn a_duplicate_fact_is_absorbed_by_the_inbox_key_rather_than_by_a_read() {
        assert!(sql::CLAIM_FACT.contains("ON CONFLICT (region, category, fact_id) DO NOTHING"));
        assert!(
            sql::CLAIM_FACT.contains("'pending'"),
            "a claimed fact starts pending"
        );
        for column in ["rated_microusd", "transaction_id"] {
            assert!(
                sql::READ_FACT.contains(column),
                "a rated duplicate cannot be acknowledged without `{column}`"
            );
        }
    }

    #[test]
    fn a_claim_resolves_its_pricing_context_from_the_reservation_it_names() {
        assert!(
            sql::CLAIM_FACT.contains("r.pricing_version"),
            "the pricing version is pinned by the reservation, never by the producer"
        );
        assert!(
            sql::CLAIM_FACT.contains("r.pricing_version = :pricing_version"),
            "a producer hint may agree with the pinned reservation but cannot replace it"
        );
    }

    #[test]
    fn a_claim_cannot_cross_workspace_region_or_the_reservation_closure_fence() {
        for fence in [
            "r.workspace_id = :workspace_id",
            "r.region = :region",
            "r.state IN ('open', 'closing')",
        ] {
            assert!(sql::CLAIM_FACT.contains(fence), "missing fence `{fence}`");
        }
    }

    #[test]
    fn only_a_serialization_failure_is_retried_inside_one_invocation() {
        assert!(SettleError::Serialization.is_retryable());
        assert!(
            !SettleError::OutcomeUnknown("lost".to_owned()).is_retryable(),
            "an unknown outcome goes back to the queue, never round the loop"
        );
        assert!(!SettleError::Unavailable("no route".to_owned()).is_retryable());
        assert!(!SettleError::UnknownReservation("usage_1".to_owned()).is_retryable());
    }

    #[test]
    fn the_four_meters_fold_onto_the_three_rating_categories() {
        assert_eq!(category(Meter::ComputeMillicpuMs), "compute");
        assert_eq!(category(Meter::MemoryByteMs), "compute");
        assert_eq!(category(Meter::StorageByteMin), "storage");
        assert_eq!(category(Meter::DataTransferEgressByte), "transfer");
    }

    #[test]
    fn every_authority_attributes_to_a_reservation_scope_the_ddl_admits() {
        let ddl = include_str!("../../../migrations/central/20260801000500_baseline_finance.sql");
        for kind in [
            AuthorityKind::Storage,
            AuthorityKind::Compute,
            AuthorityKind::Transfer,
        ] {
            assert!(
                ddl.contains(&format!("'{}'", scope_kind(kind))),
                "`{}` is not a declared reservation scope_kind",
                scope_kind(kind)
            );
        }
    }
}
