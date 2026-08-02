//! Bounded, durable download-grant expiry orchestration.

use std::sync::Arc;

use aex_content_dynamodb::codec::{GrantExpiryCursor, GrantExpiryPosition};
use aex_content_dynamodb::store::{ContentStore, GrantExpiry, GrantExpiryPage};
use aex_session_dynamodb::error::StoreError;
use aex_session_dynamodb::paging::PageBudget;
use aex_wire::types::Timestamp;
use async_trait::async_trait;
use tokio::sync::Semaphore;
use tokio::task::JoinSet;

use crate::{MAX_EXPIRY_SCANS_IN_FLIGHT, MAX_EXPIRY_WRITES_IN_FLIGHT};

/// The narrow regional-content operations the expiry role owns.
#[async_trait]
pub trait ExpiryStore: Clone + Send + Sync + 'static {
    /// Strongly reads one shard's durable scan cursor.
    async fn load_cursor(&self, shard: u16) -> Result<Option<GrantExpiryCursor>, StoreError>;

    /// Queries one bounded due-index page after the exact durable provider key.
    async fn scan_expired_grants(
        &self,
        shard: u16,
        now: Timestamp,
        budget: PageBudget,
        after: Option<&GrantExpiryPosition>,
    ) -> Result<GrantExpiryPage, StoreError>;

    /// Atomically removes one expired grant and its exact pin.
    async fn expire_grant(&self, grant: &GrantExpiry, now: Timestamp) -> Result<(), StoreError>;

    /// Advances or wraps the cursor under its optimistic revision.
    async fn advance_cursor(
        &self,
        current: Option<&GrantExpiryCursor>,
        shard: u16,
        position: Option<&GrantExpiryPosition>,
        now: Timestamp,
    ) -> Result<(), StoreError>;
}

#[async_trait]
impl ExpiryStore for ContentStore {
    async fn load_cursor(&self, shard: u16) -> Result<Option<GrantExpiryCursor>, StoreError> {
        aex_content_dynamodb::store::ContentMetadataStore::load_grant_expiry_cursor(self, shard)
            .await
    }

    async fn scan_expired_grants(
        &self,
        shard: u16,
        now: Timestamp,
        budget: PageBudget,
        after: Option<&GrantExpiryPosition>,
    ) -> Result<GrantExpiryPage, StoreError> {
        aex_content_dynamodb::store::ContentMetadataStore::scan_expired_grants(
            self, shard, now, budget, after,
        )
        .await
    }

    async fn expire_grant(&self, grant: &GrantExpiry, now: Timestamp) -> Result<(), StoreError> {
        aex_content_dynamodb::store::ContentMetadataStore::expire_grant(self, grant, now).await
    }

    async fn advance_cursor(
        &self,
        current: Option<&GrantExpiryCursor>,
        shard: u16,
        position: Option<&GrantExpiryPosition>,
        now: Timestamp,
    ) -> Result<(), StoreError> {
        aex_content_dynamodb::store::ContentMetadataStore::advance_grant_expiry_cursor(
            self, current, shard, position, now,
        )
        .await
    }
}

/// Exact settled counts for one scheduled expiry invocation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExpiryReport {
    /// Shard pipelines the invocation scheduled.
    pub shards_attempted: u16,
    /// Shard queries that returned a valid page.
    pub shards_scanned: u16,
    /// Successful pages with a durable continuation.
    pub shards_with_more: u64,
    /// Grants returned by every successful shard page.
    pub grants_selected: u64,
    /// Grant-and-pin transactions that committed or replayed successfully.
    pub expired_grants: u64,
    /// Cursor advances or wraps that committed or replayed successfully.
    pub cursors_advanced: u64,
    /// Successful cursor advances that wrapped a shard to its start.
    pub cursors_wrapped: u64,
}

/// A scheduled invocation that settled one or more failures.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error(
    "grant expiry settled with {scan_failures} scan failure(s), {write_failures} write failure(s), {cursor_failures} cursor failure(s) and {task_failures} task failure(s); expired {expired}/{selected} selected grant(s), advanced {advanced} cursor(s), and scanned {scanned}/{attempted} shard(s); first failure: {first_failure}",
    expired = .report.expired_grants,
    selected = .report.grants_selected,
    advanced = .report.cursors_advanced,
    scanned = .report.shards_scanned,
    attempted = .report.shards_attempted,
)]
pub struct ExpiryFailure {
    /// Counts that succeeded before the invocation failed loud.
    pub report: ExpiryReport,
    /// Store failures returned by cursor reads or shard queries.
    pub scan_failures: u64,
    /// Store failures returned by grant-and-pin transactions.
    pub write_failures: u64,
    /// Store failures returned by cursor advances.
    pub cursor_failures: u64,
    /// Panicked or cancelled bounded tasks.
    pub task_failures: u64,
    /// Deterministic first store failure, or the first task failure when no
    /// store operation returned an error.
    pub first_failure: String,
}

#[derive(Debug, Default)]
struct ShardOutcome {
    scanned: u16,
    has_more: u64,
    selected: u64,
    expired: u64,
    cursor_advanced: u64,
    cursor_wrapped: u64,
    scan_error: Option<String>,
    write_errors: Vec<(usize, String)>,
    cursor_error: Option<String>,
    task_errors: Vec<String>,
}

/// Walks every admitted shard through its durable position. At most sixteen
/// shard pipelines are active. Every selected grant transaction shares one
/// global sixteen-operation semaphore with cursor writes, so an advance never
/// races ahead of an unsettled write and concurrent shards cannot multiply the
/// provider concurrency bound.
///
/// A query failure never advances that shard. After a successful query, every
/// launched expiry transaction settles before the cursor advances, even when
/// some transactions fail. The final page writes a wrapped `None` position, so
/// failed rows become visible again on a later pass.
///
/// # Errors
///
/// [`ExpiryFailure`] after every launched pipeline and operation has settled.
pub async fn expire_due_grants<S: ExpiryStore>(
    store: S,
    shards: u16,
    now: Timestamp,
    budget: PageBudget,
) -> Result<ExpiryReport, ExpiryFailure> {
    let write_limit = Arc::new(Semaphore::new(MAX_EXPIRY_WRITES_IN_FLIGHT));
    let mut pipelines = JoinSet::new();
    let mut outcomes = Vec::with_capacity(usize::from(shards));
    let mut pipeline_errors = Vec::new();

    for shard in 0..shards {
        while pipelines.len() >= MAX_EXPIRY_SCANS_IN_FLIGHT {
            settle_pipeline(&mut pipelines, &mut outcomes, &mut pipeline_errors).await;
        }
        let store = store.clone();
        let write_limit = Arc::clone(&write_limit);
        pipelines.spawn(async move {
            (
                shard,
                expire_shard(store, shard, now, budget, write_limit).await,
            )
        });
    }
    while !pipelines.is_empty() {
        settle_pipeline(&mut pipelines, &mut outcomes, &mut pipeline_errors).await;
    }
    outcomes.sort_unstable_by_key(|(shard, _)| *shard);

    let mut report = ExpiryReport {
        shards_attempted: shards,
        shards_scanned: 0,
        shards_with_more: 0,
        grants_selected: 0,
        expired_grants: 0,
        cursors_advanced: 0,
        cursors_wrapped: 0,
    };
    let mut scan_errors = Vec::new();
    let mut write_errors = Vec::new();
    let mut cursor_errors = Vec::new();
    let mut task_errors = pipeline_errors;
    for (shard, mut outcome) in outcomes {
        report.shards_scanned += outcome.scanned;
        report.shards_with_more += outcome.has_more;
        report.grants_selected += outcome.selected;
        report.expired_grants += outcome.expired;
        report.cursors_advanced += outcome.cursor_advanced;
        report.cursors_wrapped += outcome.cursor_wrapped;
        if let Some(error) = outcome.scan_error {
            scan_errors.push((shard, error));
        }
        write_errors.extend(
            outcome
                .write_errors
                .drain(..)
                .map(|(ordinal, error)| ((shard, ordinal), error)),
        );
        if let Some(error) = outcome.cursor_error {
            cursor_errors.push((shard, error));
        }
        task_errors.extend(outcome.task_errors);
    }
    if scan_errors.is_empty()
        && write_errors.is_empty()
        && cursor_errors.is_empty()
        && task_errors.is_empty()
    {
        return Ok(report);
    }
    scan_errors.sort_unstable_by_key(|(shard, _)| *shard);
    write_errors.sort_unstable_by_key(|(position, _)| *position);
    cursor_errors.sort_unstable_by_key(|(shard, _)| *shard);
    let first_failure = scan_errors
        .first()
        .map(|(_, error)| error.clone())
        .or_else(|| write_errors.first().map(|(_, error)| error.clone()))
        .or_else(|| cursor_errors.first().map(|(_, error)| error.clone()))
        .or_else(|| task_errors.first().cloned())
        .unwrap_or_else(|| "grant expiry failed without a recorded cause".to_owned());
    Err(ExpiryFailure {
        report,
        scan_failures: scan_errors.len() as u64,
        write_failures: write_errors.len() as u64,
        cursor_failures: cursor_errors.len() as u64,
        task_failures: task_errors.len() as u64,
        first_failure,
    })
}

async fn expire_shard<S: ExpiryStore>(
    store: S,
    shard: u16,
    now: Timestamp,
    budget: PageBudget,
    write_limit: Arc<Semaphore>,
) -> ShardOutcome {
    let mut outcome = ShardOutcome::default();
    let cursor = match store.load_cursor(shard).await {
        Ok(cursor) => cursor,
        Err(error) => {
            outcome.scan_error = Some(error.to_string());
            return outcome;
        }
    };
    let page = match store
        .scan_expired_grants(
            shard,
            now,
            budget,
            cursor.as_ref().and_then(|cursor| cursor.position.as_ref()),
        )
        .await
    {
        Ok(page) => page,
        Err(error) => {
            outcome.scan_error = Some(error.to_string());
            return outcome;
        }
    };
    outcome.scanned = 1;
    outcome.has_more = u64::from(page.next.is_some());
    outcome.selected = page.grants.len() as u64;

    let mut writes = JoinSet::new();
    for (ordinal, grant) in page.grants.into_iter().enumerate() {
        let store = store.clone();
        let write_limit = Arc::clone(&write_limit);
        writes.spawn(async move {
            let permit = write_limit
                .acquire_owned()
                .await
                .expect("the invocation owns its write semaphore");
            let result = store.expire_grant(&grant, now).await;
            drop(permit);
            (ordinal, result)
        });
    }
    while let Some(result) = writes.join_next().await {
        match result {
            Ok((_, Ok(()))) => outcome.expired += 1,
            Ok((ordinal, Err(error))) => outcome.write_errors.push((ordinal, error.to_string())),
            Err(error) => outcome
                .task_errors
                .push(format!("grant expiry write task failed: {error}")),
        }
    }

    let permit = write_limit
        .acquire_owned()
        .await
        .expect("the invocation owns its write semaphore");
    let advance = store
        .advance_cursor(cursor.as_ref(), shard, page.next.as_ref(), now)
        .await;
    drop(permit);
    match advance {
        Ok(()) => {
            outcome.cursor_advanced = 1;
            outcome.cursor_wrapped = u64::from(page.next.is_none());
        }
        Err(error) => outcome.cursor_error = Some(error.to_string()),
    }
    outcome
}

async fn settle_pipeline(
    tasks: &mut JoinSet<(u16, ShardOutcome)>,
    outcomes: &mut Vec<(u16, ShardOutcome)>,
    task_errors: &mut Vec<String>,
) {
    match tasks.join_next().await {
        Some(Ok(outcome)) => outcomes.push(outcome),
        Some(Err(error)) => task_errors.push(format!("grant expiry shard task failed: {error}")),
        None => {}
    }
}
