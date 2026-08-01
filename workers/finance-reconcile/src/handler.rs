//! One scheduled invocation: sweep, then report every finding.

use std::sync::Arc;

use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

use crate::sweep::{OperationsAlarm, ReconcileAuthority, SweepError, SweepReport};

/// What the schedule may ask for.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(tag = "request", rename_all = "snake_case")]
pub enum ReconcileRequest {
    /// The five-minute sweep.
    Sweep,
    /// Readiness: this deployable has proved its own database grants.
    Readyz,
}

/// What one invocation answered.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(tag = "result", rename_all = "snake_case")]
pub enum ReconcileResponse {
    /// The sweep completed.
    Swept {
        /// Accounts whose projection disagrees with the journal.
        divergent_accounts: usize,
        /// The global signed posting sum, which must be zero.
        global_imbalance_microusd: i64,
        /// Effects still inside their exact-key replay window.
        replayable: usize,
        /// Effects that need a provider lookup.
        lookup_required: usize,
        /// Effects escalated to an operator.
        escalated: usize,
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

/// The operations subject one report carries.
pub const FINDING_SUBJECT: &str = "aex finance reconciliation finding";

/// Handles one invocation.
///
/// # Errors
///
/// Returns the sweep failure, which fails the scheduled invocation. A sweep that
/// cannot read the authority must not report "nothing found".
pub async fn handle<A: ReconcileAuthority, R: OperationsAlarm>(
    authority: &Arc<A>,
    alarm: &Arc<R>,
    request: ReconcileRequest,
    page_limit: u32,
    now: OffsetDateTime,
) -> Result<ReconcileResponse, SweepError> {
    match request {
        ReconcileRequest::Readyz => match authority.probe_role().await {
            Ok(()) => Ok(ReconcileResponse::Ready {
                ready: true,
                reason: None,
            }),
            Err(error) => Ok(ReconcileResponse::Ready {
                ready: false,
                reason: Some(error.to_string()),
            }),
        },
        ReconcileRequest::Sweep => {
            let report = authority.sweep(page_limit, now).await?;
            if report.has_findings() {
                alarm.report(FINDING_SUBJECT, &render(&report)).await?;
            }
            Ok(ReconcileResponse::Swept {
                divergent_accounts: report.divergent_accounts.len(),
                global_imbalance_microusd: report.global_imbalance_microusd,
                replayable: report.replayable.len(),
                lookup_required: report.lookup_required.len(),
                escalated: report.escalated.len(),
            })
        }
    }
}

/// Renders a finding for the operations topic.
///
/// Counts and identities only: no amount beyond the global imbalance, and never
/// an account holder, an email or a provider payload.
#[must_use]
pub fn render(report: &SweepReport) -> String {
    format!(
        "divergentAccounts={} globalImbalanceMicrousd={} escalatedEffects={}",
        report.divergent_accounts.len(),
        report.global_imbalance_microusd,
        report.escalated.len()
    )
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use parking_lot::Mutex;
    use time::OffsetDateTime;

    use super::{ReconcileRequest, ReconcileResponse, handle, render};
    use crate::sweep::{OperationsAlarm, ReconcileAuthority, SweepError, SweepReport};

    #[derive(Debug)]
    struct Scripted {
        answer: Result<SweepReport, SweepError>,
    }

    #[async_trait::async_trait]
    impl ReconcileAuthority for Scripted {
        async fn probe_role(&self) -> Result<(), SweepError> {
            Ok(())
        }

        async fn sweep(
            &self,
            _page_limit: u32,
            _now: OffsetDateTime,
        ) -> Result<SweepReport, SweepError> {
            self.answer.clone()
        }
    }

    #[derive(Debug, Default)]
    struct RecordingAlarm {
        published: Mutex<Vec<String>>,
        calls: AtomicUsize,
    }

    #[async_trait::async_trait]
    impl OperationsAlarm for RecordingAlarm {
        async fn report(&self, _subject: &str, body: &str) -> Result<(), SweepError> {
            self.calls.fetch_add(1, Ordering::Relaxed);
            self.published.lock().push(body.to_owned());
            Ok(())
        }
    }

    #[tokio::test]
    async fn a_clean_sweep_reports_nothing_to_the_operations_topic() {
        let authority = Arc::new(Scripted {
            answer: Ok(SweepReport::default()),
        });
        let alarm = Arc::new(RecordingAlarm::default());
        let answer = handle(
            &authority,
            &alarm,
            ReconcileRequest::Sweep,
            500,
            OffsetDateTime::UNIX_EPOCH,
        )
        .await
        .expect("the sweep completes");
        assert!(matches!(
            answer,
            ReconcileResponse::Swept {
                divergent_accounts: 0,
                global_imbalance_microusd: 0,
                ..
            }
        ));
        assert_eq!(alarm.calls.load(Ordering::Relaxed), 0);
    }

    #[tokio::test]
    async fn a_conservation_break_is_reported_and_never_repaired() {
        let authority = Arc::new(Scripted {
            answer: Ok(SweepReport {
                divergent_accounts: vec!["acct-1".to_owned()],
                global_imbalance_microusd: -25,
                ..SweepReport::default()
            }),
        });
        let alarm = Arc::new(RecordingAlarm::default());
        handle(
            &authority,
            &alarm,
            ReconcileRequest::Sweep,
            500,
            OffsetDateTime::UNIX_EPOCH,
        )
        .await
        .expect("the sweep completes");
        assert_eq!(alarm.calls.load(Ordering::Relaxed), 1);
        let published = alarm.published.lock().clone();
        assert!(published[0].contains("globalImbalanceMicrousd=-25"));
    }

    #[tokio::test]
    async fn an_unreadable_authority_fails_the_invocation_rather_than_reporting_all_clear() {
        let authority = Arc::new(Scripted {
            answer: Err(SweepError::Unavailable("no route".to_owned())),
        });
        let alarm = Arc::new(RecordingAlarm::default());
        let error = handle(
            &authority,
            &alarm,
            ReconcileRequest::Sweep,
            500,
            OffsetDateTime::UNIX_EPOCH,
        )
        .await
        .expect_err("a sweep that cannot read must not answer all-clear");
        assert!(matches!(error, SweepError::Unavailable(_)));
        assert_eq!(alarm.calls.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn a_rendered_finding_carries_counts_and_no_account_detail() {
        let rendered = render(&SweepReport {
            divergent_accounts: vec!["acct-1".to_owned()],
            global_imbalance_microusd: 7,
            escalated: vec!["eff-1".to_owned()],
            ..SweepReport::default()
        });
        assert!(rendered.contains("divergentAccounts=1"));
        assert!(rendered.contains("escalatedEffects=1"));
        assert!(
            !rendered.contains("acct-1"),
            "an operations record carries counts, not account identities"
        );
    }
}
