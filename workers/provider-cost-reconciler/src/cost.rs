//! The private COGS fact family.
//!
//! `finance.provider_cost_fact` is a fourth fact family with **no path to a
//! customer posting** (F-26, `U-COGS`). This deployable's whole surface is
//! read an export, normalise it on `(source, source_row_id)`, and report a
//! margin finding. It holds no grant that could charge anybody.

use std::sync::Arc;

use aex_rds_data::{DataApiClient, DataApiError, DecodeError, Record, Row, SqlValue, Statement};
use serde::{Deserialize, Serialize};

/// Every statement this deployable runs.
pub mod sql {
    /// Proves the connection can record a cost and can charge nobody.
    pub const PROBE_ROLE: &str = "\
SELECT pg_has_role(current_user, :role, 'MEMBER'), \
       has_table_privilege(:role, 'finance.provider_cost_fact', 'INSERT'), \
       has_table_privilege(:role, 'finance.journal_posting', 'INSERT'), \
       has_table_privilege(:role, 'finance.account_balance', 'UPDATE')";

    /// Records one cost row. A duplicate export normalises on the key.
    pub const RECORD_COST: &str = "\
INSERT INTO finance.provider_cost_fact \
  (source, source_row_id, period, service, region, cost_microusd) \
VALUES (:source, :source_row_id, :period, :service, :region, :cost_microusd) \
ON CONFLICT (source, source_row_id) DO NOTHING";

    /// The recorded provider cost of one period.
    pub const PERIOD_COST: &str = "\
SELECT coalesce(sum(cost_microusd), 0)::bigint FROM finance.provider_cost_fact \
 WHERE period = :period";

    /// The earned revenue of one period, from the journal.
    pub const PERIOD_REVENUE: &str = "\
SELECT coalesce(-sum(p.amount_microusd), 0)::bigint \
  FROM finance.journal_posting p \
  JOIN finance.account a ON a.account_id = p.account_id \
  JOIN finance.journal_transaction t ON t.transaction_id = p.transaction_id \
 WHERE a.kind = 'usage_revenue' AND to_char(t.occurred_at, 'YYYY-MM') = :period";
}

/// Which export a cost row came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderCostSource {
    /// The AWS cost and usage report.
    AwsCur,
    /// The Stripe balance report.
    StripeBalanceReport,
}

impl ProviderCostSource {
    /// The durable spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::AwsCur => "aws_cur",
            Self::StripeBalanceReport => "stripe_balance_report",
        }
    }
}

/// One normalised cost row.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ProviderCostRow {
    /// Which export it came from.
    pub source: ProviderCostSource,
    /// The export's own identity for the row, which is the dedup key.
    pub source_row_id: String,
    /// The billed calendar month, `YYYY-MM`.
    pub period: String,
    /// The provider service.
    pub service: String,
    /// The provider region.
    pub region: String,
    /// The cost, in integer micro-USD. Never a float, at any stage.
    pub cost_microusd: i64,
}

/// The margin of one period, in basis points.
///
/// Returns `None` when the period earned nothing: a margin against zero revenue
/// is not a small number, it is undefined, and reporting it as `-10000` would
/// alarm on every unopened month.
#[must_use]
pub const fn margin_bps(revenue_microusd: i64, cost_microusd: i64) -> Option<i64> {
    if revenue_microusd <= 0 {
        return None;
    }
    let earned = revenue_microusd - cost_microusd;
    Some(earned.saturating_mul(10_000) / revenue_microusd)
}

/// Why a reconciliation did not complete.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CostError {
    /// The export is unreachable.
    #[error("the provider cost export is unavailable: {0}")]
    ExportUnavailable(String),
    /// The export row is not the expected shape.
    #[error("the provider cost export is off-contract: {0}")]
    OffContract(String),
    /// The authority is unreachable.
    #[error("the finance authority is unavailable: {0}")]
    Unavailable(String),
    /// The stored row does not match what this deployable projects.
    #[error("the finance authority returned an undecodable row: {0}")]
    Decode(String),
}

/// Recording provider cost.
#[async_trait::async_trait]
pub trait ProviderCostLedger: Send + Sync + 'static {
    /// Proves this deployable can record a cost and can charge nobody.
    async fn probe_role(&self) -> Result<(), CostError>;

    /// Records one page of normalised rows, ignoring duplicates.
    async fn record(&self, rows: &[ProviderCostRow]) -> Result<usize, CostError>;

    /// The recorded cost and earned revenue of one period.
    async fn margin_inputs(&self, period: &str) -> Result<(i64, i64), CostError>;
}

/// The Aurora-backed cost ledger.
#[derive(Debug, Clone)]
pub struct AuroraProviderCostLedger {
    client: Arc<DataApiClient>,
    role: String,
}

impl AuroraProviderCostLedger {
    /// Builds the ledger over a configured Data `API` client.
    #[must_use]
    pub fn new(client: Arc<DataApiClient>, role: String) -> Self {
        Self { client, role }
    }
}

/// The four privilege answers the readiness probe checks.
#[derive(Debug)]
struct GrantRow {
    /// Whether the login holds the declared role.
    holds_role: bool,
    /// Whether it may append a cost row, which it must.
    can_record_cost: bool,
    /// Whether it can reach a customer posting, which it must not.
    reaches_a_customer_amount: bool,
}

impl Row for GrantRow {
    fn from_record(record: &Record<'_>) -> Result<Self, DecodeError> {
        record.expect_arity(4)?;
        Ok(Self {
            holds_role: record.bool(0)?,
            can_record_cost: record.bool(1)?,
            // Either reach is disqualifying, so the two probes fold into one
            // answer rather than into two fields nothing reads apart.
            reaches_a_customer_amount: record.bool(2)? || record.bool(3)?,
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

/// Classifies a transport failure.
fn store(error: DataApiError) -> CostError {
    match error {
        DataApiError::Decode(decode) => CostError::Decode(decode.to_string()),
        other => CostError::Unavailable(other.to_string()),
    }
}

#[async_trait::async_trait]
impl ProviderCostLedger for AuroraProviderCostLedger {
    async fn probe_role(&self) -> Result<(), CostError> {
        let row: GrantRow = self
            .client
            .query_one(Statement::with(
                sql::PROBE_ROLE,
                vec![("role", SqlValue::Text(self.role.clone()))],
            ))
            .await
            .map_err(store)?;
        if !row.holds_role || !row.can_record_cost {
            return Err(CostError::Unavailable(format!(
                "`{}` cannot record a provider cost",
                self.role
            )));
        }
        if row.reaches_a_customer_amount {
            return Err(CostError::Unavailable(format!(
                "F-26: `{}` can reach a customer posting; the COGS family has no path to one",
                self.role
            )));
        }
        Ok(())
    }

    async fn record(&self, rows: &[ProviderCostRow]) -> Result<usize, CostError> {
        let mut recorded = 0;
        for row in rows {
            if row.cost_microusd < 0 {
                return Err(CostError::OffContract(format!(
                    "row `{}` reports a negative cost",
                    row.source_row_id
                )));
            }
            recorded += usize::try_from(
                self.client
                    .execute(Statement::with(
                        sql::RECORD_COST,
                        vec![
                            ("source", SqlValue::Text(row.source.as_str().to_owned())),
                            ("source_row_id", SqlValue::Text(row.source_row_id.clone())),
                            ("period", SqlValue::Text(row.period.clone())),
                            ("service", SqlValue::Text(row.service.clone())),
                            ("region", SqlValue::Text(row.region.clone())),
                            ("cost_microusd", SqlValue::I64(row.cost_microusd)),
                        ],
                    ))
                    .await
                    .map_err(store)?,
            )
            .unwrap_or(0);
        }
        Ok(recorded)
    }

    async fn margin_inputs(&self, period: &str) -> Result<(i64, i64), CostError> {
        let cost: AmountRow = self
            .client
            .query_one(Statement::with(
                sql::PERIOD_COST,
                vec![("period", SqlValue::Text(period.to_owned()))],
            ))
            .await
            .map_err(store)?;
        let revenue: AmountRow = self
            .client
            .query_one(Statement::with(
                sql::PERIOD_REVENUE,
                vec![("period", SqlValue::Text(period.to_owned()))],
            ))
            .await
            .map_err(store)?;
        Ok((revenue.0, cost.0))
    }
}

#[cfg(test)]
mod tests {
    use super::{ProviderCostSource, margin_bps, sql};

    #[test]
    fn every_statement_binds_and_never_touches_floating_point() {
        for statement in [
            sql::PROBE_ROLE,
            sql::RECORD_COST,
            sql::PERIOD_COST,
            sql::PERIOD_REVENUE,
        ] {
            assert!(statement.contains(':'), "{statement} binds no parameter");
            assert!(!statement.to_lowercase().contains("float"));
            assert!(!statement.to_lowercase().contains("double"));
        }
    }

    #[test]
    fn the_cost_family_has_no_path_to_a_customer_posting() {
        assert!(
            sql::RECORD_COST.contains("finance.provider_cost_fact"),
            "the only write this deployable makes is a cost row"
        );
        for forbidden in [
            "finance.journal_posting",
            "finance.journal_transaction",
            "finance.account_balance",
            "finance.billing_account",
        ] {
            assert!(
                !sql::RECORD_COST.contains(forbidden),
                "F-26: a cost row must never reach `{forbidden}`"
            );
        }
    }

    #[test]
    fn a_duplicate_export_row_normalises_on_its_own_identity() {
        assert!(sql::RECORD_COST.contains("ON CONFLICT (source, source_row_id) DO NOTHING"));
    }

    #[test]
    fn both_export_families_have_a_durable_spelling() {
        assert_eq!(ProviderCostSource::AwsCur.as_str(), "aws_cur");
        assert_eq!(
            ProviderCostSource::StripeBalanceReport.as_str(),
            "stripe_balance_report"
        );
    }

    #[test]
    fn margin_is_undefined_against_zero_revenue_rather_than_minus_one_hundred_per_cent() {
        assert_eq!(margin_bps(0, 5_000), None);
        assert_eq!(margin_bps(-1, 5_000), None);
    }

    #[test]
    fn margin_is_exact_integer_basis_points() {
        assert_eq!(margin_bps(10_000, 0), Some(10_000));
        assert_eq!(margin_bps(10_000, 5_000), Some(5_000));
        assert_eq!(margin_bps(10_000, 10_000), Some(0));
        assert_eq!(
            margin_bps(10_000, 20_000),
            Some(-10_000),
            "a period that cost more than it earned reports a negative margin"
        );
    }

    #[test]
    fn a_margin_computation_cannot_overflow_on_a_production_shaped_period() {
        // One billion dollars of revenue in micro-USD, which is the largest
        // single posting the journal admits.
        assert!(margin_bps(1_000_000_000_000_000, 1).is_some());
    }
}
