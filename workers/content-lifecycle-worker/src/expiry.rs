//! Bounded download-grant expiry orchestration.

use aex_content_dynamodb::store::{ContentStore, GrantExpiry, GrantExpiryPage};
use aex_session_dynamodb::error::StoreError;
use aex_session_dynamodb::paging::PageBudget;
use aex_wire::types::Timestamp;
use async_trait::async_trait;
use tokio::task::JoinSet;

use crate::{MAX_EXPIRY_SCANS_IN_FLIGHT, MAX_EXPIRY_WRITES_IN_FLIGHT};

/// The narrow regional-content operations the expiry role owns.
#[async_trait]
pub trait ExpiryStore: Clone + Send + Sync + 'static {
    /// Queries one bounded due-index page.
    ///
    /// # Errors
    ///
    /// [`StoreError`] when the query or projected-row decode fails.
    async fn scan_expired_grants(
        &self,
        shard: u16,
        now: Timestamp,
        budget: PageBudget,
    ) -> Result<GrantExpiryPage, StoreError>;

    /// Atomically removes one expired grant and its exact pin.
    ///
    /// # Errors
    ///
    /// [`StoreError`] when the transaction transport or condition fails.
    async fn expire_grant(&self, grant: &GrantExpiry, now: Timestamp) -> Result<(), StoreError>;
}

#[async_trait]
impl ExpiryStore for ContentStore {
    async fn scan_expired_grants(
        &self,
        shard: u16,
        now: Timestamp,
        budget: PageBudget,
    ) -> Result<GrantExpiryPage, StoreError> {
        aex_content_dynamodb::store::ContentMetadataStore::scan_expired_grants(
            self, shard, now, budget,
        )
        .await
    }

    async fn expire_grant(&self, grant: &GrantExpiry, now: Timestamp) -> Result<(), StoreError> {
        aex_content_dynamodb::store::ContentMetadataStore::expire_grant(self, grant, now).await
    }
}

/// Exact settled counts for one scheduled expiry invocation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExpiryReport {
    /// Shard queries the invocation scheduled.
    pub shards_attempted: u16,
    /// Shard queries that returned a valid page.
    pub shards_scanned: u16,
    /// Successful pages whose last-evaluated key showed more due rows.
    pub shards_with_more: u64,
    /// Grants returned by every successful shard page.
    pub grants_selected: u64,
    /// Grant-and-pin transactions that committed or replayed successfully.
    pub expired_grants: u64,
}

/// A scheduled invocation that settled one or more failures.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error(
    "grant expiry settled with {scan_failures} scan failure(s), {write_failures} write failure(s) and {task_failures} task failure(s); expired {expired}/{selected} selected grant(s) after {scanned}/{attempted} shard scan(s), with {more} successful page(s) reporting more; first failure: {first_failure}",
    expired = .report.expired_grants,
    selected = .report.grants_selected,
    scanned = .report.shards_scanned,
    attempted = .report.shards_attempted,
    more = .report.shards_with_more,
)]
pub struct ExpiryFailure {
    /// Counts that succeeded before the invocation failed loud.
    pub report: ExpiryReport,
    /// Store failures returned by shard queries.
    pub scan_failures: u64,
    /// Store failures returned by grant-and-pin transactions.
    pub write_failures: u64,
    /// Panicked or cancelled bounded tasks.
    pub task_failures: u64,
    /// Deterministic first store failure, or the first task failure when no
    /// store operation returned an error.
    pub first_failure: String,
}

/// Queries every admitted shard with bounded concurrency, then attempts every
/// grant returned by the successful pages with an independent write bound.
///
/// A failure never cancels another selected operation. The invocation returns
/// an error only after every possible scan and write has settled, preserving
/// successful idempotent progress for a scheduler retry.
///
/// # Errors
///
/// [`ExpiryFailure`] after any shard query, expiry transaction or task fails.
pub async fn expire_due_grants<S: ExpiryStore>(
    store: S,
    shards: u16,
    now: Timestamp,
    budget: PageBudget,
) -> Result<ExpiryReport, ExpiryFailure> {
    let mut scans = JoinSet::new();
    let mut pages = Vec::with_capacity(usize::from(shards));
    let mut scan_errors = Vec::new();
    let mut task_errors = Vec::new();

    for shard in 0..shards {
        while scans.len() >= MAX_EXPIRY_SCANS_IN_FLIGHT {
            settle_scan(&mut scans, &mut pages, &mut scan_errors, &mut task_errors).await;
        }
        let store = store.clone();
        scans.spawn(async move { (shard, store.scan_expired_grants(shard, now, budget).await) });
    }
    while !scans.is_empty() {
        settle_scan(&mut scans, &mut pages, &mut scan_errors, &mut task_errors).await;
    }

    pages.sort_unstable_by_key(|(shard, _)| *shard);
    let shards_with_more = pages
        .iter()
        .filter(|(_, page)| page.more)
        .fold(0_u64, |count, _| count + 1);
    let grants: Vec<_> = pages
        .iter_mut()
        .flat_map(|(_, page)| page.grants.drain(..))
        .collect();
    let grants_selected = grants.iter().fold(0_u64, |count, _| count + 1);

    let mut writes = JoinSet::new();
    let mut expired_grants = 0_u64;
    let mut write_errors = Vec::new();
    for (ordinal, grant) in grants.into_iter().enumerate() {
        while writes.len() >= MAX_EXPIRY_WRITES_IN_FLIGHT {
            settle_write(
                &mut writes,
                &mut expired_grants,
                &mut write_errors,
                &mut task_errors,
            )
            .await;
        }
        let store = store.clone();
        writes.spawn(async move {
            let result = store.expire_grant(&grant, now).await;
            (ordinal, result)
        });
    }
    while !writes.is_empty() {
        settle_write(
            &mut writes,
            &mut expired_grants,
            &mut write_errors,
            &mut task_errors,
        )
        .await;
    }

    let report = ExpiryReport {
        shards_attempted: shards,
        shards_scanned: pages.iter().fold(0_u16, |count, _| count + 1),
        shards_with_more,
        grants_selected,
        expired_grants,
    };
    if scan_errors.is_empty() && write_errors.is_empty() && task_errors.is_empty() {
        return Ok(report);
    }

    scan_errors.sort_unstable_by_key(|(shard, _)| *shard);
    write_errors.sort_unstable_by_key(|(ordinal, _)| *ordinal);
    let first_failure = scan_errors
        .first()
        .map(|(_, error)| error.clone())
        .or_else(|| write_errors.first().map(|(_, error)| error.clone()))
        .or_else(|| task_errors.first().cloned())
        .unwrap_or_else(|| "grant expiry failed without a recorded cause".to_owned());
    Err(ExpiryFailure {
        report,
        scan_failures: scan_errors.iter().fold(0_u64, |count, _| count + 1),
        write_failures: write_errors.iter().fold(0_u64, |count, _| count + 1),
        task_failures: task_errors.iter().fold(0_u64, |count, _| count + 1),
        first_failure,
    })
}

async fn settle_scan(
    tasks: &mut JoinSet<(u16, Result<GrantExpiryPage, StoreError>)>,
    pages: &mut Vec<(u16, GrantExpiryPage)>,
    store_errors: &mut Vec<(u16, String)>,
    task_errors: &mut Vec<String>,
) {
    match tasks.join_next().await {
        Some(Ok((shard, Ok(page)))) => pages.push((shard, page)),
        Some(Ok((shard, Err(error)))) => store_errors.push((shard, error.to_string())),
        Some(Err(error)) => task_errors.push(format!("grant expiry scan task failed: {error}")),
        None => {}
    }
}

async fn settle_write(
    tasks: &mut JoinSet<(usize, Result<(), StoreError>)>,
    completed: &mut u64,
    store_errors: &mut Vec<(usize, String)>,
    task_errors: &mut Vec<String>,
) {
    match tasks.join_next().await {
        Some(Ok((_, Ok(())))) => *completed += 1,
        Some(Ok((ordinal, Err(error)))) => store_errors.push((ordinal, error.to_string())),
        Some(Err(error)) => task_errors.push(format!("grant expiry write task failed: {error}")),
        None => {}
    }
}
