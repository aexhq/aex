//! Pending-upload expiry: the `HeadObject` oracle and the S3-first ordering.
//!
//! This is the first consumer of the `regional-work` due kind
//! `"registry.upload_expiry"`, which was declared and unused. Before it existed
//! there was no pending-upload expiry at all, because the durable row named
//! neither the provider multipart upload nor the object key.
//!
//! Two rules shape every branch here.
//!
//! **`HeadObject` on the durable object key is the sole oracle** (E D-3). It is
//! total, not probabilistic: the key is content-addressed and every write to it is
//! conditional, so "an object exists at this key with this digest and this length"
//! is exactly equivalent to "these bytes are committed for this workspace".
//!
//! **The irreversible provider effect goes first, and the write that records it is
//! conditional on the exact state and handle the decision was made under**
//! (E D-6). A committed `Aborted` row followed by a failed abort would leave a
//! multipart upload nothing but the bucket lifecycle rule could ever reclaim, and
//! would leave the durable state lying about the provider.
//!
//! This role holds **no object-delete verb** (E D-4). The worst outcome of any bug
//! here is an orphan object the content GC reclaims after its grace, at storage
//! cost — not a destroyed committed body.

use aex_session_dynamodb::error::StoreError;
use aex_session_dynamodb::paging::PageBudget;
use aex_wire::ids::{PrefixedId as _, UploadId, WorkspaceId};
use aex_wire::types::Timestamp;
use aex_work_dynamodb::codec::{ReconciliationCursor, WorkRecord};
use aex_work_dynamodb::claim::WorkClaim;
use aex_work_dynamodb::store::{DueEntry, DuePage, WorkAuthority, WorkStore};
use aex_workspace_domain::upload::{
    AmbiguityResolution, ExpiryOutcome, HeadOracle, Upload, UploadState, expire, resolve_completing,
};
use async_trait::async_trait;

/// The narrow object operations this role owns.
///
/// There is no delete verb, and there is nowhere in this trait to add one without
/// the reviewer seeing it.
#[async_trait]
pub trait UploadObjects: Send + Sync {
    /// Heads the upload's exact durable object key.
    ///
    /// Never fails: a failure is [`HeadOracle::Unavailable`], which is evidence of
    /// nothing and re-arms the due item rather than deciding anything.
    async fn head(&self, upload: &Upload) -> HeadOracle;

    /// Aborts the exact multipart upload the row names.
    ///
    /// # Errors
    ///
    /// Any provider failure, as text. `NoSuchUpload` is an idempotent success at
    /// the adapter and never reaches here.
    async fn abort_multipart(&self, upload: &Upload) -> Result<(), String>;
}

/// The narrow registry operations this role owns.
#[async_trait]
pub trait UploadRows: Send + Sync {
    /// Reads one upload with every spilled part block.
    async fn load(
        &self,
        workspace: WorkspaceId,
        upload: UploadId,
    ) -> Result<Option<Upload>, StoreError>;

    /// Applies a transition fenced on the state and the provider handle.
    async fn transition_fenced(
        &self,
        upload: &Upload,
        from: UploadState,
        to: UploadState,
    ) -> Result<(), StoreError>;

    /// Settles an upload as `Ready` with the evidence that proves its object.
    async fn settle_ready(&self, upload: &Upload) -> Result<(), StoreError>;

    /// Removes a row that has no remaining consumer.
    async fn delete_row(&self, upload: &Upload) -> Result<(), StoreError>;
}

/// What one due upload's sweep did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SweepOutcome {
    /// The row was gone before the sweep reached it.
    Absent,
    /// The row is not due, or nothing would change.
    Unchanged,
    /// The object turned out to exist, so the upload was settled `Ready`.
    ///
    /// Never aborted and never deleted: the bytes the caller asked for are
    /// committed, and that is the upload's entire purpose.
    Committed,
    /// The provider upload was aborted and the row lapsed.
    Expired,
    /// The row had no remaining consumer and was removed.
    Deleted,
    /// An object exists whose digest or length disagrees with the declaration.
    ///
    /// A permanent stall by design. The row stays visible and unswept rather than
    /// being cleaned up wrongly; this is impossible under content addressing and
    /// must be investigated.
    Integrity {
        /// What disagreed.
        detail: String,
    },
    /// Nothing could be established. Re-arm with backoff.
    Retry {
        /// Why.
        detail: String,
    },
}

impl SweepOutcome {
    /// Whether the due item should be re-armed rather than retired.
    #[must_use]
    pub const fn re_arms(&self) -> bool {
        matches!(self, Self::Integrity { .. } | Self::Retry { .. })
    }
}

/// Sweeps one due upload.
///
/// # Errors
///
/// [`StoreError`] from the durable reads and writes. A provider failure is an
/// outcome, not an error: it re-arms.
pub async fn sweep_one<R: UploadRows + ?Sized, O: UploadObjects + ?Sized>(
    rows: &R,
    objects: &O,
    workspace: WorkspaceId,
    upload_id: UploadId,
    now: Timestamp,
) -> Result<SweepOutcome, StoreError> {
    let Some(upload) = rows.load(workspace, upload_id).await? else {
        return Ok(SweepOutcome::Absent);
    };

    match expire(&upload, now) {
        ExpiryOutcome::Unchanged => Ok(SweepOutcome::Unchanged),
        ExpiryOutcome::DeleteRow => {
            rows.delete_row(&upload).await?;
            Ok(SweepOutcome::Deleted)
        }
        ExpiryOutcome::Expire(commit) => {
            // The row is due and pre-completion. Nothing is decided until the
            // oracle answers.
            let oracle = objects.head(&upload).await;
            match resolve_completing(&upload, &oracle, now) {
                AmbiguityResolution::AlreadySettled => Ok(SweepOutcome::Unchanged),
                AmbiguityResolution::Retry => Ok(SweepOutcome::Retry {
                    detail: "the head could not be taken".to_owned(),
                }),
                AmbiguityResolution::Integrity { detail } => Ok(SweepOutcome::Integrity { detail }),
                AmbiguityResolution::Committed(settled) => {
                    // The bytes are committed. Never abort, never delete.
                    rows.settle_ready(&settled.upload).await?;
                    Ok(SweepOutcome::Committed)
                }
                AmbiguityResolution::NotCommitted => {
                    // Only now is the abort provably safe. S3 first, then the
                    // conditional write that records it.
                    if let Err(detail) = objects.abort_multipart(&upload).await {
                        return Ok(SweepOutcome::Retry { detail });
                    }
                    rows.transition_fenced(&upload, upload.state, commit.upload.state)
                        .await?;
                    Ok(SweepOutcome::Expired)
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// The shard fan-out
// ---------------------------------------------------------------------------

/// The due kind this role consumes.
///
/// Declared in `aex-work-dynamodb` and unused until now; this is its first
/// consumer (E D-8).
pub const DUE_KIND: &str = "registry.upload_expiry";

/// How long a sweep holds a due item while it resolves one upload.
///
/// One `HeadObject`, at most one `AbortMultipartUpload` and one conditional
/// write. A minute is generous and still far under the invocation budget.
pub const CLAIM_LEASE_MILLIS: i64 = 60_000;

/// The narrow `regional-work` operations this role owns.
#[async_trait]
pub trait DueUploads: Send + Sync {
    /// Strongly reads one shard's durable scan cursor.
    async fn load_cursor(&self, shard: u16) -> Result<Option<ReconciliationCursor>, StoreError>;

    /// Queries one bounded due page after the exact durable position.
    async fn scan_due_after(
        &self,
        shard: u16,
        now: Timestamp,
        budget: PageBudget,
        after: Option<&ReconciliationCursor>,
    ) -> Result<DuePage, StoreError>;

    /// Reads one due record, for the payload the slim index does not project.
    async fn load(
        &self,
        workspace: WorkspaceId,
        work_id: &str,
    ) -> Result<Option<WorkRecord>, StoreError>;

    /// Takes one record's lease.
    async fn claim(
        &self,
        work_id: &str,
        owner: &str,
        now: Timestamp,
        lease_until: Timestamp,
    ) -> Result<WorkClaim, StoreError>;

    /// Retires one record under its fence.
    async fn complete(&self, hold: &WorkClaim, now: Timestamp) -> Result<(), StoreError>;

    /// Advances or wraps one shard's cursor under its optimistic revision.
    async fn advance_cursor(&self, cursor: &ReconciliationCursor) -> Result<(), StoreError>;
}

#[async_trait]
impl DueUploads for WorkStore {
    async fn load_cursor(&self, shard: u16) -> Result<Option<ReconciliationCursor>, StoreError> {
        WorkAuthority::load_cursor(self, shard).await
    }

    async fn scan_due_after(
        &self,
        shard: u16,
        now: Timestamp,
        budget: PageBudget,
        after: Option<&ReconciliationCursor>,
    ) -> Result<DuePage, StoreError> {
        WorkAuthority::scan_due_after(self, shard, now, budget, after).await
    }

    async fn load(
        &self,
        workspace: WorkspaceId,
        work_id: &str,
    ) -> Result<Option<WorkRecord>, StoreError> {
        WorkAuthority::load(self, workspace, work_id).await
    }

    async fn claim(
        &self,
        work_id: &str,
        owner: &str,
        now: Timestamp,
        lease_until: Timestamp,
    ) -> Result<WorkClaim, StoreError> {
        WorkAuthority::claim_work(self, work_id, owner, now, lease_until).await
    }

    async fn complete(&self, hold: &WorkClaim, now: Timestamp) -> Result<(), StoreError> {
        WorkAuthority::complete_work(self, hold, now).await
    }

    async fn advance_cursor(&self, cursor: &ReconciliationCursor) -> Result<(), StoreError> {
        WorkAuthority::advance_cursor(self, cursor).await
    }
}

/// Exact settled counts for one scheduled upload-expiry invocation.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UploadExpiryReport {
    /// Shard pipelines the invocation scheduled.
    pub shards_attempted: u16,
    /// Shard queries that returned a valid page.
    pub shards_scanned: u16,
    /// Due rows of this kind the pages named.
    pub selected: u64,
    /// Uploads settled `Ready` because the object turned out to exist.
    pub committed: u64,
    /// Uploads whose provider upload was aborted and whose row lapsed.
    pub expired: u64,
    /// Rows removed because nothing was left to consume them.
    pub deleted: u64,
    /// Rows left alone: not due, already settled, or held by another worker.
    pub unchanged: u64,
    /// Rows left visible and unswept because the object disagrees.
    pub stalled: u64,
    /// Rows re-armed because nothing could be established.
    pub re_armed: u64,
    /// Cursor advances that committed.
    pub cursors_advanced: u64,
}

/// A scheduled invocation that settled one or more failures.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error(
    "upload expiry settled with {failures} failure(s); committed {committed}, expired {expired}, \
     deleted {deleted}, stalled {stalled}, re-armed {rearmed} of {selected} selected row(s) \
     across {scanned}/{attempted} shard(s); first failure: {first_failure}",
    committed = .report.committed,
    expired = .report.expired,
    deleted = .report.deleted,
    stalled = .report.stalled,
    rearmed = .report.re_armed,
    selected = .report.selected,
    scanned = .report.shards_scanned,
    attempted = .report.shards_attempted,
)]
pub struct UploadExpiryFailure {
    /// Counts that succeeded before the invocation failed loud.
    pub report: UploadExpiryReport,
    /// How many operations failed.
    pub failures: u64,
    /// The deterministic first failure.
    pub first_failure: String,
}

/// Walks every admitted shard of the `registry.upload_expiry` due index.
///
/// This copies the grant-expiry shape: a per-shard durable cursor, a bounded
/// page, every write settled before the cursor advances, and a query failure that
/// never advances that shard. A row this pass could not resolve stays due and is
/// seen again, which is what makes "re-arm with backoff" a real outcome rather
/// than a comment.
///
/// # Errors
///
/// [`UploadExpiryFailure`] after every operation has settled.
pub async fn expire_due_uploads<D, R, O>(
    due: &D,
    rows: &R,
    objects: &O,
    owner: &str,
    shards: u16,
    now: Timestamp,
    budget: PageBudget,
) -> Result<UploadExpiryReport, UploadExpiryFailure>
where
    D: DueUploads + ?Sized,
    R: UploadRows + ?Sized,
    O: UploadObjects + ?Sized,
{
    let mut report = UploadExpiryReport {
        shards_attempted: shards,
        ..UploadExpiryReport::default()
    };
    let mut failures: Vec<String> = Vec::new();
    let lease_until = Timestamp::from_unix_millis(now.unix_millis() + CLAIM_LEASE_MILLIS)
        .unwrap_or(now);

    for shard in 0..shards {
        let cursor = match due.load_cursor(shard).await {
            Ok(cursor) => cursor,
            Err(error) => {
                failures.push(error.to_string());
                continue;
            }
        };
        let page = match due.scan_due_after(shard, now, budget, cursor.as_ref()).await {
            Ok(page) => page,
            // A query failure never advances that shard, so nothing is skipped.
            Err(error) => {
                failures.push(error.to_string());
                continue;
            }
        };
        report.shards_scanned += 1;

        let mut last_work_id = None;
        for entry in &page.items {
            if entry.kind != DUE_KIND {
                continue;
            }
            report.selected += 1;
            last_work_id = Some(entry.work_id.clone());
            match sweep_due_row(due, rows, objects, entry, owner, now, lease_until).await {
                Ok(outcome) => match outcome {
                    SweepOutcome::Committed => report.committed += 1,
                    SweepOutcome::Expired => report.expired += 1,
                    SweepOutcome::Deleted => report.deleted += 1,
                    SweepOutcome::Absent | SweepOutcome::Unchanged => report.unchanged += 1,
                    SweepOutcome::Integrity { .. } => report.stalled += 1,
                    SweepOutcome::Retry { .. } => report.re_armed += 1,
                },
                Err(error) => failures.push(error),
            }
        }

        // Every write of this page has settled. Only now does the cursor move,
        // and a page with nothing after it wraps to the shard start so rows this
        // pass left behind become visible again.
        let advanced = ReconciliationCursor {
            shard,
            scanned_through_effective_due_at: page.scanned_through.unwrap_or(now),
            last_work_id: if page.has_more { last_work_id } else { None },
            revision: cursor.as_ref().map_or(0, |cursor| cursor.revision) + 1,
            updated_at: now,
        };
        match due.advance_cursor(&advanced).await {
            Ok(()) => report.cursors_advanced += 1,
            Err(error) => failures.push(error.to_string()),
        }
    }

    if failures.is_empty() {
        return Ok(report);
    }
    let first_failure = failures
        .first()
        .cloned()
        .unwrap_or_else(|| "upload expiry failed without a recorded cause".to_owned());
    Err(UploadExpiryFailure {
        report,
        failures: failures.len() as u64,
        first_failure,
    })
}

async fn sweep_due_row<D, R, O>(
    due: &D,
    rows: &R,
    objects: &O,
    entry: &DueEntry,
    owner: &str,
    now: Timestamp,
    lease_until: Timestamp,
) -> Result<SweepOutcome, String>
where
    D: DueUploads + ?Sized,
    R: UploadRows + ?Sized,
    O: UploadObjects + ?Sized,
{
    let record = due
        .load(entry.workspace, &entry.work_id)
        .await
        .map_err(|error| error.to_string())?;
    let Some(record) = record else {
        return Ok(SweepOutcome::Absent);
    };
    // The upload identity comes from the typed payload, never parsed back out of
    // the work id: a parsed identifier would be one naming change away from
    // sweeping the wrong row.
    let upload_id = record
        .payload
        .members()
        .get("uploadId")
        .ok_or_else(|| format!("`{DUE_KIND}` payload carries no uploadId"))?;
    let upload_id = UploadId::parse(upload_id).map_err(|error| error.to_string())?;

    let hold = match due.claim(&entry.work_id, owner, now, lease_until).await {
        Ok(hold) => hold,
        // Another worker holds the lease, or the attempt budget is spent. Both
        // are acked without work rather than raced.
        Err(StoreError::PreconditionFailed { .. }) => return Ok(SweepOutcome::Unchanged),
        Err(error) => return Err(error.to_string()),
    };

    let outcome = sweep_one(rows, objects, entry.workspace, upload_id, now)
        .await
        .map_err(|error| error.to_string())?;

    // A row that re-arms keeps its due item: the sweep established nothing, and
    // retiring the item would leave that upload unswept forever.
    if !outcome.re_arms() {
        due.complete(&hold, now)
            .await
            .map_err(|error| error.to_string())?;
    }
    Ok(outcome)
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use aex_wire::ids::ContentHash as ContentDigest;
    use aex_wire::ids::{PrefixedId as _, UploadId, Uuid7, WorkspaceId};
    use aex_wire::types::Timestamp;
    use aex_workspace_domain::upload::{
        CompletionEvidence, PartGrantRequest, PartPlan, PlannedPart, Upload, UploadState,
        grant_parts,
    };

    use super::{HeadOracle, StoreError, SweepOutcome, UploadObjects, UploadRows, sweep_one};

    fn moment(millis: i64) -> Timestamp {
        Timestamp::from_unix_millis(millis).expect("in range")
    }

    fn workspace() -> WorkspaceId {
        WorkspaceId::from_uuid7(Uuid7::compose(1, [2; 10]))
    }

    fn upload_id() -> UploadId {
        UploadId::from_uuid7(Uuid7::compose(1, [1; 10]))
    }

    fn evidence() -> CompletionEvidence {
        CompletionEvidence {
            etag: "\"assembled\"".to_owned(),
            checksum_sha256: None,
            checksum_crc64_nvme: None,
            part_count: Some(1),
        }
    }

    fn upload(state: UploadState) -> Upload {
        let staged = Upload {
            id: upload_id(),
            workspace: workspace(),
            state: UploadState::Created,
            provider_upload_id: "provider-mpu-1".to_owned(),
            object_key: "wks/ab/cd/abcd".to_owned(),
            declared_size: 1_024,
            declared_sha256: ContentDigest::of(b"body"),
            content_type: None,
            parts: PartPlan {
                parts: vec![PlannedPart {
                    number: 1,
                    bytes: 1_024,
                    sha256: None,
                }],
            },
            completion_manifest: Vec::new(),
            completion: None,
            consumed_by: None,
            created_at: moment(0),
            expires_at: moment(86_400_000),
        };
        let mut granted = grant_parts(
            &staged,
            &[PartGrantRequest {
                number: 1,
                sha256: ContentDigest::of(b"part one"),
                size_bytes: 1_024,
            }],
            moment(1),
        )
        .expect("grants")
        .upload;
        granted.state = state;
        if state == UploadState::Ready {
            granted.completion = Some(evidence());
        }
        granted
    }

    #[derive(Default)]
    struct Recorder {
        row: Option<Upload>,
        writes: Vec<String>,
    }

    struct Rows(Mutex<Recorder>);

    #[async_trait::async_trait]
    impl UploadRows for Rows {
        async fn load(
            &self,
            _workspace: WorkspaceId,
            _upload: UploadId,
        ) -> Result<Option<Upload>, StoreError> {
            Ok(self.0.lock().expect("not poisoned").row.clone())
        }

        async fn transition_fenced(
            &self,
            _upload: &Upload,
            from: UploadState,
            to: UploadState,
        ) -> Result<(), StoreError> {
            self.0
                .lock()
                .expect("not poisoned")
                .writes
                .push(format!("transition {from:?}->{to:?}"));
            Ok(())
        }

        async fn settle_ready(&self, _upload: &Upload) -> Result<(), StoreError> {
            self.0
                .lock()
                .expect("not poisoned")
                .writes
                .push("settle_ready".to_owned());
            Ok(())
        }

        async fn delete_row(&self, _upload: &Upload) -> Result<(), StoreError> {
            self.0
                .lock()
                .expect("not poisoned")
                .writes
                .push("delete_row".to_owned());
            Ok(())
        }
    }

    struct Objects {
        oracle: HeadOracle,
        aborts: Mutex<Vec<String>>,
        abort_fails: bool,
    }

    #[async_trait::async_trait]
    impl UploadObjects for Objects {
        async fn head(&self, _upload: &Upload) -> HeadOracle {
            self.oracle.clone()
        }

        async fn abort_multipart(&self, upload: &Upload) -> Result<(), String> {
            self.aborts
                .lock()
                .expect("not poisoned")
                .push(upload.provider_upload_id.clone());
            if self.abort_fails {
                return Err("the provider refused".to_owned());
            }
            Ok(())
        }
    }

    fn objects(oracle: HeadOracle) -> Objects {
        Objects {
            oracle,
            aborts: Mutex::new(Vec::new()),
            abort_fails: false,
        }
    }

    async fn sweep(row: Option<Upload>, objects: &Objects, at: i64) -> (SweepOutcome, Recorder) {
        let rows = Rows(Mutex::new(Recorder {
            row,
            writes: Vec::new(),
        }));
        let outcome = sweep_one(&rows, objects, workspace(), upload_id(), moment(at))
            .await
            .expect("sweeps");
        let recorded = std::mem::take(&mut *rows.0.lock().expect("not poisoned"));
        (outcome, recorded)
    }

    #[tokio::test]
    async fn an_object_that_exists_settles_ready_and_is_never_aborted() {
        let store = objects(HeadOracle::Present {
            content_length: 1_024,
            declared_digest: Some(ContentDigest::of(b"body")),
            evidence: evidence(),
        });
        let (outcome, recorded) = sweep(
            Some(upload(UploadState::Completing)),
            &store,
            86_400_000,
        )
        .await;
        assert_eq!(outcome, SweepOutcome::Committed);
        assert_eq!(recorded.writes, vec!["settle_ready".to_owned()]);
        assert!(
            store.aborts.lock().expect("not poisoned").is_empty(),
            "aborting an upload that completed destroys a committed body"
        );
    }

    #[tokio::test]
    async fn an_absent_object_is_aborted_at_the_provider_before_the_row_moves() {
        let store = objects(HeadOracle::Absent);
        let (outcome, recorded) = sweep(
            Some(upload(UploadState::Completing)),
            &store,
            86_400_000,
        )
        .await;
        assert_eq!(outcome, SweepOutcome::Expired);
        assert_eq!(
            store.aborts.lock().expect("not poisoned").as_slice(),
            ["provider-mpu-1".to_owned()]
        );
        assert_eq!(
            recorded.writes,
            vec!["transition Completing->Expired".to_owned()]
        );
    }

    #[tokio::test]
    async fn a_failed_abort_writes_nothing_and_re_arms() {
        let mut store = objects(HeadOracle::Absent);
        store.abort_fails = true;
        let (outcome, recorded) = sweep(
            Some(upload(UploadState::PartsGranted)),
            &store,
            86_400_000,
        )
        .await;
        assert!(matches!(outcome, SweepOutcome::Retry { .. }));
        assert!(outcome.re_arms());
        assert!(
            recorded.writes.is_empty(),
            "a committed Aborted row after a failed abort leaves the state lying \
             about the provider"
        );
    }

    #[tokio::test]
    async fn a_disagreeing_object_stalls_loudly_rather_than_being_swept() {
        let store = objects(HeadOracle::Present {
            content_length: 999,
            declared_digest: Some(ContentDigest::of(b"body")),
            evidence: evidence(),
        });
        let (outcome, recorded) = sweep(
            Some(upload(UploadState::Completing)),
            &store,
            86_400_000,
        )
        .await;
        assert!(matches!(outcome, SweepOutcome::Integrity { .. }));
        assert!(outcome.re_arms());
        assert!(recorded.writes.is_empty());
        assert!(store.aborts.lock().expect("not poisoned").is_empty());
    }

    #[tokio::test]
    async fn an_unavailable_head_decides_nothing() {
        let store = objects(HeadOracle::Unavailable);
        let (outcome, recorded) = sweep(
            Some(upload(UploadState::Completing)),
            &store,
            86_400_000,
        )
        .await;
        assert!(matches!(outcome, SweepOutcome::Retry { .. }));
        assert!(recorded.writes.is_empty());
        assert!(store.aborts.lock().expect("not poisoned").is_empty());
    }

    #[tokio::test]
    async fn a_ready_row_past_its_grace_is_reclaimed_without_touching_the_provider() {
        let store = objects(HeadOracle::Absent);
        let (outcome, recorded) =
            sweep(Some(upload(UploadState::Ready)), &store, 86_400_000).await;
        assert_eq!(outcome, SweepOutcome::Deleted);
        assert_eq!(recorded.writes, vec!["delete_row".to_owned()]);
        assert!(store.aborts.lock().expect("not poisoned").is_empty());
    }

    #[tokio::test]
    async fn a_row_that_is_not_due_yet_writes_nothing() {
        let store = objects(HeadOracle::Absent);
        let (outcome, recorded) = sweep(
            Some(upload(UploadState::PartsGranted)),
            &store,
            86_399_999,
        )
        .await;
        assert_eq!(outcome, SweepOutcome::Unchanged);
        assert!(recorded.writes.is_empty());
    }

    #[tokio::test]
    async fn a_row_that_is_already_gone_is_an_idempotent_success() {
        let store = objects(HeadOracle::Absent);
        let (outcome, _) = sweep(None, &store, 86_400_000).await;
        assert_eq!(outcome, SweepOutcome::Absent);
    }
}
