//! One scheduled invocation: drain each category's backlog within the deadline.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::Instant;

use serde::{Deserialize, Serialize};

use crate::outbox::{DispatchError, ReceiptOutbox, ReceiptPublisher};

/// What the schedule may ask for.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(tag = "request", rename_all = "snake_case")]
pub enum DispatchRequest {
    /// Drain one page per category.
    Drain,
    /// Readiness: this deployable has proved its own database grants.
    Readyz,
}

/// What one invocation did.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize, Serialize)]
pub struct DrainReport {
    /// Receipts the regional queue accepted.
    pub dispatched: usize,
    /// Distinct receipts that stayed pending because the queue refused them.
    pub deferred: usize,
    /// Distinct receipts over the configured attempt bound.
    pub over_attempt_bound: usize,
    /// Whether paging stopped before every category read an empty page: the
    /// invocation deadline arrived, or a full page moved nothing. Unread
    /// backlog remains for the next scheduled invocation.
    #[serde(default)]
    pub truncated: bool,
}

/// Drains every bound category to an empty page, inside the deadline.
///
/// Every category is guaranteed one page per invocation; remaining time is
/// then spent round-robin on the categories whose last page was full, so one
/// deep backlog cannot starve the other queues and a backlog deeper than one
/// page no longer needs one scheduled tick per page. `stop` is where paging
/// ends — the caller derives it from the Lambda deadline minus
/// [`crate::config::DEADLINE_SAFETY_MARGIN`] — and a drain that stops early
/// says so in [`DrainReport::truncated`] rather than pretending the backlog
/// was one page deep.
///
/// # Errors
///
/// Returns [`DispatchError`] when the outbox itself cannot be read. A queue
/// failure is *not* an error: the receipt stays pending and the next invocation
/// replays it, which is the whole point of an outbox.
pub async fn drain<O: ReceiptOutbox, P: ReceiptPublisher>(
    outbox: &Arc<O>,
    publisher: &Arc<P>,
    categories: &[String],
    page_limit: u32,
    max_attempts: u32,
    stop: Instant,
) -> Result<DrainReport, DispatchError> {
    let mut report = DrainReport::default();
    let mut deferred = BTreeSet::new();
    let mut over_bound = BTreeSet::new();
    let mut open: Vec<&String> = categories.iter().collect();
    let mut first_round = true;
    while !open.is_empty() {
        let mut still_open = Vec::new();
        for category in open {
            if !first_round && Instant::now() >= stop {
                // The deadline margin is spent. Whatever is still open keeps
                // its backlog until the next scheduled invocation, and the
                // report says so instead of ending on a clean-looking count.
                report.truncated = true;
                still_open.clear();
                break;
            }
            let page = outbox.pending(category, page_limit).await?;
            let full = page.len() >= page_limit as usize;
            let mut progressed = false;
            for receipt in page {
                if u32::try_from(receipt.attempts).unwrap_or(u32::MAX) >= max_attempts {
                    over_bound.insert(receipt.receipt_id);
                }
                if publisher.publish(&receipt).await.is_ok() {
                    outbox.mark_dispatched(receipt.receipt_id).await?;
                    report.dispatched += 1;
                    progressed = true;
                } else {
                    outbox.count_attempt(receipt.receipt_id).await?;
                    deferred.insert(receipt.receipt_id);
                }
            }
            if full {
                if progressed {
                    still_open.push(category);
                } else {
                    // A full page the queue refused entirely: the next page
                    // would return these same pending rows, so paging on is a
                    // spin, not progress. The backlog stays, visibly.
                    report.truncated = true;
                }
            }
        }
        open = still_open;
        first_round = false;
    }
    report.deferred = deferred.len();
    report.over_attempt_bound = over_bound.len();
    Ok(report)
}

/// What one invocation answered.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(tag = "result", rename_all = "snake_case")]
pub enum DispatchResponse {
    /// The drain completed.
    Drained {
        /// What it moved.
        report: DrainReport,
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

/// Handles one invocation.
///
/// # Errors
///
/// Returns the outbox failure so the scheduled invocation fails visibly.
pub async fn handle<O: ReceiptOutbox, P: ReceiptPublisher>(
    outbox: &Arc<O>,
    publisher: &Arc<P>,
    request: DispatchRequest,
    categories: &[String],
    page_limit: u32,
    max_attempts: u32,
    stop: Instant,
) -> Result<DispatchResponse, DispatchError> {
    match request {
        DispatchRequest::Readyz => match outbox.probe_role().await {
            Ok(()) => Ok(DispatchResponse::Ready {
                ready: true,
                reason: None,
            }),
            Err(error) => Ok(DispatchResponse::Ready {
                ready: false,
                reason: Some(error.to_string()),
            }),
        },
        DispatchRequest::Drain => {
            let report = drain(
                outbox,
                publisher,
                categories,
                page_limit,
                max_attempts,
                stop,
            )
            .await?;
            Ok(DispatchResponse::Drained { report })
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::{Duration, Instant};

    use parking_lot::Mutex;

    use super::{DispatchRequest, DispatchResponse, drain, handle};
    use crate::outbox::{DispatchError, PendingReceipt, ReceiptOutbox, ReceiptPublisher};

    /// A stop instant no test drain reaches on its own.
    fn no_deadline() -> Instant {
        Instant::now() + Duration::from_mins(1)
    }

    /// A stop instant that has already passed when the drain starts.
    fn expired_deadline() -> Instant {
        Instant::now()
    }

    #[derive(Debug, Default)]
    struct Fake {
        pending: Mutex<Vec<PendingReceipt>>,
        dispatched: Mutex<Vec<uuid::Uuid>>,
        attempts: Mutex<Vec<uuid::Uuid>>,
    }

    #[async_trait::async_trait]
    impl ReceiptOutbox for Fake {
        async fn probe_role(&self) -> Result<(), DispatchError> {
            Ok(())
        }

        async fn pending(
            &self,
            category: &str,
            page_limit: u32,
        ) -> Result<Vec<PendingReceipt>, DispatchError> {
            // The real outbox pages: it answers at most `page_limit` rows and a
            // dispatched receipt leaves the pending set. A fake that returned
            // everything forever would make every paging assertion vacuous.
            Ok(self
                .pending
                .lock()
                .iter()
                .filter(|receipt| receipt.category == category)
                .take(usize::try_from(page_limit).unwrap_or(usize::MAX))
                .cloned()
                .collect())
        }

        async fn mark_dispatched(&self, receipt_id: uuid::Uuid) -> Result<(), DispatchError> {
            self.dispatched.lock().push(receipt_id);
            self.pending
                .lock()
                .retain(|receipt| receipt.receipt_id != receipt_id);
            Ok(())
        }

        async fn count_attempt(&self, receipt_id: uuid::Uuid) -> Result<(), DispatchError> {
            self.attempts.lock().push(receipt_id);
            if let Some(receipt) = self
                .pending
                .lock()
                .iter_mut()
                .find(|receipt| receipt.receipt_id == receipt_id)
            {
                receipt.attempts += 1;
            }
            Ok(())
        }
    }

    #[derive(Debug, Default)]
    struct Publisher {
        refuse: bool,
        calls: AtomicUsize,
        seen: Mutex<Vec<String>>,
    }

    #[async_trait::async_trait]
    impl ReceiptPublisher for Publisher {
        async fn publish(&self, receipt: &PendingReceipt) -> Result<(), DispatchError> {
            self.calls.fetch_add(1, Ordering::Relaxed);
            self.seen.lock().push(receipt.deduplication_id());
            if self.refuse {
                Err(DispatchError::QueueUnavailable("no route".to_owned()))
            } else {
                Ok(())
            }
        }
    }

    fn receipt(index: u128, category: &str, attempts: i64) -> PendingReceipt {
        PendingReceipt {
            receipt_id: uuid::Uuid::from_u128(index),
            region: "eu-west-1".to_owned(),
            category: category.to_owned(),
            fact_id: format!("usage_{index}"),
            payload: "{}".to_owned(),
            attempts,
        }
    }

    fn categories() -> Vec<String> {
        vec![
            "compute".to_owned(),
            "storage".to_owned(),
            "transfer".to_owned(),
        ]
    }

    #[tokio::test]
    async fn every_pending_receipt_is_replayed_to_its_own_category_queue() {
        let outbox = Arc::new(Fake::default());
        outbox.pending.lock().extend([
            receipt(1, "compute", 0),
            receipt(2, "storage", 0),
            receipt(3, "transfer", 0),
        ]);
        let publisher = Arc::new(Publisher::default());
        let report = drain(&outbox, &publisher, &categories(), 10, 25, no_deadline())
            .await
            .expect("the drain completes");
        assert_eq!(report.dispatched, 3);
        assert_eq!(report.deferred, 0);
        assert!(!report.truncated, "the whole backlog was read");
        assert_eq!(outbox.dispatched.lock().len(), 3);
    }

    #[tokio::test]
    async fn a_backlog_deeper_than_one_page_is_paged_to_empty() {
        let outbox = Arc::new(Fake::default());
        outbox
            .pending
            .lock()
            .extend((1..=25).map(|index| receipt(index, "compute", 0)));
        let publisher = Arc::new(Publisher::default());
        let report = drain(&outbox, &publisher, &categories(), 10, 25, no_deadline())
            .await
            .expect("the drain completes");
        assert_eq!(
            report.dispatched, 25,
            "one invocation drains the whole backlog, not one page of it"
        );
        assert!(!report.truncated);
        assert!(outbox.pending.lock().is_empty());
    }

    #[tokio::test]
    async fn an_expired_deadline_stops_after_the_guaranteed_page_and_says_so() {
        let outbox = Arc::new(Fake::default());
        outbox
            .pending
            .lock()
            .extend((1..=25).map(|index| receipt(index, "compute", 0)));
        let publisher = Arc::new(Publisher::default());
        let report = drain(
            &outbox,
            &publisher,
            &categories(),
            10,
            25,
            expired_deadline(),
        )
        .await
        .expect("the drain completes");
        assert_eq!(
            report.dispatched, 10,
            "every category still gets its one guaranteed page"
        );
        assert!(
            report.truncated,
            "the report must not pretend the backlog ended at the deadline"
        );
        assert_eq!(outbox.pending.lock().len(), 15);
    }

    #[tokio::test]
    async fn a_regional_outage_grows_the_outbox_and_retires_nothing() {
        let outbox = Arc::new(Fake::default());
        outbox.pending.lock().push(receipt(1, "compute", 0));
        let publisher = Arc::new(Publisher {
            refuse: true,
            ..Publisher::default()
        });
        let report = drain(&outbox, &publisher, &categories(), 10, 25, no_deadline())
            .await
            .expect("a queue failure is not an outbox failure");
        assert_eq!(report.dispatched, 0);
        assert_eq!(report.deferred, 1);
        assert!(
            !report.truncated,
            "a short page was read to its end; nothing is unread"
        );
        assert!(
            outbox.dispatched.lock().is_empty(),
            "a receipt the queue never accepted stays pending"
        );
        assert_eq!(outbox.attempts.lock().len(), 1);
    }

    #[tokio::test]
    async fn a_full_page_the_queue_refuses_entirely_stops_paging_rather_than_spinning() {
        let outbox = Arc::new(Fake::default());
        outbox
            .pending
            .lock()
            .extend((1..=15).map(|index| receipt(index, "compute", 0)));
        let publisher = Arc::new(Publisher {
            refuse: true,
            ..Publisher::default()
        });
        let report = drain(&outbox, &publisher, &categories(), 10, 25, no_deadline())
            .await
            .expect("a queue failure is not an outbox failure");
        assert_eq!(
            publisher.calls.load(Ordering::Relaxed),
            10,
            "a page that moved nothing is not fetched again: the next page is the same rows"
        );
        assert_eq!(report.deferred, 10);
        assert!(report.truncated, "unread backlog is reported, not hidden");
        assert_eq!(outbox.pending.lock().len(), 15);
    }

    #[tokio::test]
    async fn a_receipt_over_its_attempt_bound_is_reported_but_still_replayed() {
        let outbox = Arc::new(Fake::default());
        outbox.pending.lock().push(receipt(1, "compute", 99));
        let publisher = Arc::new(Publisher::default());
        let report = drain(&outbox, &publisher, &categories(), 10, 25, no_deadline())
            .await
            .expect("the drain completes");
        assert_eq!(report.over_attempt_bound, 1);
        assert_eq!(
            report.dispatched, 1,
            "a receipt is never dropped: the frontier still needs it"
        );
    }

    #[tokio::test]
    async fn an_unreadable_outbox_fails_the_invocation() {
        #[derive(Debug)]
        struct Broken;

        #[async_trait::async_trait]
        impl ReceiptOutbox for Broken {
            async fn probe_role(&self) -> Result<(), DispatchError> {
                Ok(())
            }

            async fn pending(
                &self,
                _category: &str,
                _page_limit: u32,
            ) -> Result<Vec<PendingReceipt>, DispatchError> {
                Err(DispatchError::Unavailable("no route".to_owned()))
            }

            async fn mark_dispatched(&self, _receipt_id: uuid::Uuid) -> Result<(), DispatchError> {
                unreachable!("the page is never read")
            }

            async fn count_attempt(&self, _receipt_id: uuid::Uuid) -> Result<(), DispatchError> {
                unreachable!("the page is never read")
            }
        }

        let error = handle(
            &Arc::new(Broken),
            &Arc::new(Publisher::default()),
            DispatchRequest::Drain,
            &categories(),
            10,
            25,
            no_deadline(),
        )
        .await
        .expect_err("an unreadable outbox is visible");
        assert!(matches!(error, DispatchError::Unavailable(_)));
    }

    #[tokio::test]
    async fn readiness_reports_the_grant_probe() {
        let answer = handle(
            &Arc::new(Fake::default()),
            &Arc::new(Publisher::default()),
            DispatchRequest::Readyz,
            &categories(),
            10,
            25,
            no_deadline(),
        )
        .await
        .expect("readiness answers");
        assert_eq!(
            answer,
            DispatchResponse::Ready {
                ready: true,
                reason: None
            }
        );
    }
}
