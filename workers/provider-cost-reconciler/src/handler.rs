//! One daily invocation: record the export, then report the margin.

use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::cost::{CostError, ProviderCostLedger, ProviderCostRow, margin_bps};

/// What the schedule may ask for.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(tag = "request", rename_all = "snake_case")]
pub enum CostRequest {
    /// Reconcile one billed period.
    Reconcile {
        /// The `YYYY-MM` period to reconcile.
        period: String,
    },
    /// Readiness: this deployable has proved its own database grants.
    Readyz,
}

/// What one invocation answered.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(tag = "result", rename_all = "snake_case")]
pub enum CostResponse {
    /// The period was reconciled.
    Reconciled {
        /// The period.
        period: String,
        /// Rows this run made durable for the first time.
        recorded: usize,
        /// The margin in basis points, absent when the period earned nothing.
        #[serde(skip_serializing_if = "Option::is_none")]
        margin_bps: Option<i64>,
        /// Whether the margin is below the configured threshold.
        below_threshold: bool,
    },
    /// The process has proved its own grants.
    Ready {
        /// Whether the grants hold.
        ready: bool,
        /// Why they do not, when they do not.
        #[serde(skip_serializing_if = "Option::is_none")]
        reason: Option<String>,
    },
}

/// Reading one provider cost export.
#[async_trait::async_trait]
pub trait CostExport: Send + Sync + 'static {
    /// The normalised rows of one period, bounded by scanned bytes.
    async fn rows(
        &self,
        period: &str,
        max_scan_bytes: u64,
    ) -> Result<Vec<ProviderCostRow>, CostError>;
}

/// Whether a `YYYY-MM` period is well formed.
#[must_use]
pub fn is_period(value: &str) -> bool {
    let Some((year, month)) = value.split_once('-') else {
        return false;
    };
    year.len() == 4
        && year.bytes().all(|byte| byte.is_ascii_digit())
        && month.len() == 2
        && month
            .parse::<u8>()
            .is_ok_and(|month| (1..=12).contains(&month))
}

/// Handles one invocation.
///
/// # Errors
///
/// Returns the export or ledger failure. Missing COGS degrades margin
/// visibility and must be visible, so it fails the invocation rather than
/// reporting a margin computed from half an export.
pub async fn handle<L: ProviderCostLedger, E: CostExport>(
    ledger: &Arc<L>,
    export: &Arc<E>,
    request: CostRequest,
    max_scan_bytes: u64,
    threshold_bps: u32,
) -> Result<CostResponse, CostError> {
    match request {
        CostRequest::Readyz => match ledger.probe_role().await {
            Ok(()) => Ok(CostResponse::Ready {
                ready: true,
                reason: None,
            }),
            Err(error) => Ok(CostResponse::Ready {
                ready: false,
                reason: Some(error.to_string()),
            }),
        },
        CostRequest::Reconcile { period } => {
            if !is_period(&period) {
                return Err(CostError::OffContract(format!(
                    "`{period}` is not a YYYY-MM period"
                )));
            }
            let rows = export.rows(&period, max_scan_bytes).await?;
            let recorded = ledger.record(&rows).await?;
            let (revenue, cost) = ledger.margin_inputs(&period).await?;
            let margin = margin_bps(revenue, cost);
            Ok(CostResponse::Reconciled {
                period,
                recorded,
                below_threshold: margin.is_some_and(|margin| margin < i64::from(threshold_bps)),
                margin_bps: margin,
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use parking_lot::Mutex;

    use super::{CostExport, CostRequest, CostResponse, handle, is_period};
    use crate::cost::{CostError, ProviderCostLedger, ProviderCostRow, ProviderCostSource};

    #[derive(Debug, Default)]
    struct Fake {
        recorded: Mutex<Vec<ProviderCostRow>>,
        revenue: i64,
        cost: i64,
    }

    #[async_trait::async_trait]
    impl ProviderCostLedger for Fake {
        async fn probe_role(&self) -> Result<(), CostError> {
            Ok(())
        }

        async fn record(&self, rows: &[ProviderCostRow]) -> Result<usize, CostError> {
            let mut recorded = self.recorded.lock();
            let mut fresh = 0;
            for row in rows {
                if !recorded
                    .iter()
                    .any(|seen| seen.source_row_id == row.source_row_id)
                {
                    recorded.push(row.clone());
                    fresh += 1;
                }
            }
            Ok(fresh)
        }

        async fn margin_inputs(&self, _period: &str) -> Result<(i64, i64), CostError> {
            Ok((self.revenue, self.cost))
        }
    }

    #[derive(Debug, Default)]
    struct Export {
        rows: Vec<ProviderCostRow>,
    }

    #[async_trait::async_trait]
    impl CostExport for Export {
        async fn rows(
            &self,
            _period: &str,
            _max_scan_bytes: u64,
        ) -> Result<Vec<ProviderCostRow>, CostError> {
            Ok(self.rows.clone())
        }
    }

    fn row(id: &str) -> ProviderCostRow {
        ProviderCostRow {
            source: ProviderCostSource::AwsCur,
            source_row_id: id.to_owned(),
            period: "2026-07".to_owned(),
            service: "AmazonECS".to_owned(),
            region: "eu-west-1".to_owned(),
            cost_microusd: 1_000_000,
        }
    }

    #[tokio::test]
    async fn a_duplicate_export_row_is_recorded_once() {
        let ledger = Arc::new(Fake {
            revenue: 10_000_000,
            cost: 1_000_000,
            ..Fake::default()
        });
        let export = Arc::new(Export {
            rows: vec![row("a"), row("a"), row("b")],
        });
        let answer = handle(
            &ledger,
            &export,
            CostRequest::Reconcile {
                period: "2026-07".to_owned(),
            },
            1_000,
            1_500,
        )
        .await
        .expect("the reconciliation completes");
        match answer {
            CostResponse::Reconciled {
                recorded,
                margin_bps,
                below_threshold,
                ..
            } => {
                assert_eq!(recorded, 2, "a duplicate export row normalises");
                assert_eq!(margin_bps, Some(9_000));
                assert!(!below_threshold);
            }
            other @ CostResponse::Ready { .. } => {
                panic!("expected a reconciliation, got {other:?}")
            }
        }
    }

    #[tokio::test]
    async fn a_thin_margin_is_reported_and_charges_nobody() {
        let ledger = Arc::new(Fake {
            revenue: 10_000_000,
            cost: 9_500_000,
            ..Fake::default()
        });
        let export = Arc::new(Export { rows: vec![] });
        let answer = handle(
            &ledger,
            &export,
            CostRequest::Reconcile {
                period: "2026-07".to_owned(),
            },
            1_000,
            1_500,
        )
        .await
        .expect("the reconciliation completes");
        assert!(matches!(
            answer,
            CostResponse::Reconciled {
                below_threshold: true,
                ..
            }
        ));
        assert!(
            ledger.recorded.lock().is_empty(),
            "a margin finding writes no cost row and no customer amount"
        );
    }

    #[tokio::test]
    async fn a_period_that_is_not_a_calendar_month_is_refused() {
        let error = handle(
            &Arc::new(Fake::default()),
            &Arc::new(Export::default()),
            CostRequest::Reconcile {
                period: "2026-13".to_owned(),
            },
            1_000,
            1_500,
        )
        .await
        .expect_err("a period is YYYY-MM");
        assert!(matches!(error, CostError::OffContract(_)));
    }

    #[test]
    fn a_period_is_exactly_four_digits_a_dash_and_a_month() {
        assert!(is_period("2026-01"));
        assert!(!is_period("2026-13"));
        assert!(!is_period("2026"));
    }
}
