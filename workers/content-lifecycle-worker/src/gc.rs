//! The production call sites for the content garbage collector.
//!
//! `reconcile_staged`, `plan_delete` and `apply_delete_result` have existed as
//! pure, tested decision code with **no caller**. `Mode::Delete` reported every
//! SQS record as a batch-item failure without ever touching S3, so the delete
//! queue drained to its dead-letter queue and no object was ever reclaimed.
//!
//! This matters beyond tidiness. Cluster E's whole safety argument for putting the
//! provider effect before the durable write, and for giving the upload path no
//! object-delete verb at all, is that *garbage collection reclaims the orphans*.
//! Without a running GC that argument is an unbacked promise: every orphaned
//! object would accumulate forever.
//!
//! The one destructive effect stays exactly where it was: `Mode::Delete`, holding
//! `ContentObjectDelete`, issuing a delete fenced on the `ETag` the sweep recorded.

use aex_content_dynamodb::store::{ContentMetadataStore, GcScanEntry};
use aex_content_dynamodb::wire_pending::GcSweepPlan;
use aex_session_dynamodb::error::StoreError;
use aex_session_dynamodb::paging::PageBudget;
use aex_wire::ids::{ContentHash, OrganizationId, PrefixedId as _, WorkspaceId};
use aex_wire::types::Timestamp;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::{
    ContentItem, ContentState, DeleteIntent, DeleteOutcome, DeletionDenial, LifecycleError,
    ObjectDeleteResult, ReconcileOutcome, apply_delete_result, plan_delete, reconcile_staged,
};

/// The narrow object operation the delete role owns.
#[async_trait]
pub trait DeletableObjects: Send + Sync {
    /// Deletes exactly the object the sweep marked, conditional on its `ETag`.
    ///
    /// # Errors
    ///
    /// Only a transport failure. "The object changed" and "the object is already
    /// absent" are both outcomes, because neither is a reason to fail the worker.
    async fn delete_fenced(&self, intent: &DeleteIntent) -> Result<ObjectDeleteResult, String>;
}

/// One unit of work the delete queue carries.
///
/// The message names the exact object *and* the exact durable row, so the worker
/// re-derives nothing: a delete that had to reconstruct its own key from a digest
/// would be one key-algorithm change away from deleting the wrong body.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct DeleteMessage {
    /// The owning workspace.
    pub workspace_id: String,
    /// The organization the descriptor is re-checked against.
    pub organization_id: String,
    /// The body being reclaimed.
    pub digest: String,
    /// The exact unversioned object key.
    pub object_key: String,
    /// The `ETag` the sweep recorded, which fences the delete.
    pub object_etag: String,
    /// The epoch the descriptor was marked in.
    pub marked_epoch: u64,
    /// The epoch the sweeper holds.
    pub epoch: u64,
    /// When the body was staged, in epoch milliseconds.
    pub staged_at_ms: i64,
    /// The claim fence.
    pub fence: u64,
}

/// Which workspaces one scheduled reconcile or mark-sweep invocation covers.
///
/// The garbage-collection index is partitioned by `(workspace, bucket)` and this
/// process has no workspace directory, so the workspace set is an **explicit
/// input**. An invocation that names none refuses loudly rather than quietly
/// sweeping nothing, which is the failure mode that let a broken GC look healthy.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct SweepRequest {
    /// The workspaces to walk.
    pub workspace_ids: Vec<String>,
    /// The garbage-collection buckets to walk in each workspace.
    pub buckets: Vec<u16>,
}

/// What one scheduled reconcile pass settled.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReconcileReport {
    /// Rows examined.
    pub examined: u64,
    /// Rows still inside their grace window.
    pub kept_staged: u64,
    /// Rows whose reachability reappeared and were restored.
    pub restored_live: u64,
    /// Rows confirmed orphaned and handed to the sweep.
    pub confirmed_orphan: u64,
}

/// What one scheduled mark-sweep pass settled.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MarkSweepReport {
    /// Rows examined.
    pub examined: u64,
    /// Rows a live pin or unexpired grant kept.
    pub reachable: u64,
    /// Rows swept into the candidate set.
    pub swept: u64,
    /// Sweeps that lost their fence, which always means the body survives.
    pub lost_fence: u64,
}

/// What one delete batch settled.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeleteReport {
    /// Records whose object is gone.
    pub deleted: u64,
    /// Records whose object was already absent, which is idempotent success.
    pub already_absent: u64,
    /// Records whose object changed since it was marked, so the body survives.
    pub kept: u64,
    /// Records that must be retried, reported as batch-item failures.
    pub failed: Vec<String>,
}

/// Rechecks every staged row in the named workspaces against the exact grace
/// boundary and its live reachability.
///
/// # Errors
///
/// [`StoreError`] from the scan or the reachability read, and
/// [`StoreError::Invalid`] when the request names no workspace.
pub async fn reconcile<S: ContentMetadataStore + ?Sized>(
    store: &S,
    request: &SweepRequest,
    now: Timestamp,
    budget: PageBudget,
) -> Result<ReconcileReport, StoreError> {
    let workspaces = admit(request)?;
    let mut report = ReconcileReport::default();
    for workspace in workspaces {
        for bucket in &request.buckets {
            let page = store.scan_gc_bucket(workspace, *bucket, budget).await?;
            for entry in page.entries {
                let Some(item) = as_content_item(&entry, ContentState::Staged) else {
                    continue;
                };
                report.examined += 1;
                // A row whose digest is unreadable is corruption, not a
                // candidate. Leave it and count nothing.
                let Ok(digest) = ContentHash::parse(&entry.digest) else {
                    continue;
                };
                let reachability = store.reachability(workspace, &digest, now).await?;
                let mut item = item;
                item.grant_pins = u32::try_from(reachability.unexpired_grants).unwrap_or(u32::MAX);
                item.direct_pins = u32::try_from(reachability.pins).unwrap_or(u32::MAX);
                match reconcile_staged(&item, now.unix_millis()) {
                    ReconcileOutcome::KeepStaged => report.kept_staged += 1,
                    ReconcileOutcome::RestoreLive => report.restored_live += 1,
                    ReconcileOutcome::ConfirmOrphan => report.confirmed_orphan += 1,
                }
            }
        }
    }
    Ok(report)
}

/// Walks reachability in the named workspaces and stages what nothing points at.
///
/// # Errors
///
/// As [`reconcile`].
pub async fn mark_sweep<S: ContentMetadataStore + ?Sized>(
    store: &S,
    request: &SweepRequest,
    organization: OrganizationId,
    now: Timestamp,
    budget: PageBudget,
) -> Result<MarkSweepReport, StoreError> {
    let workspaces = admit(request)?;
    let mut report = MarkSweepReport::default();
    for workspace in workspaces {
        let epoch = store
            .load_gc_epoch(workspace)
            .await?
            .map_or(0, |epoch| epoch.epoch);
        for bucket in &request.buckets {
            let page = store.scan_gc_bucket(workspace, *bucket, budget).await?;
            for entry in page.entries {
                let Ok(digest) = ContentHash::parse(&entry.digest) else {
                    continue;
                };
                report.examined += 1;
                let reachability = store.reachability(workspace, &digest, now).await?;
                if !reachability.is_collectable() {
                    report.reachable += 1;
                    continue;
                }
                let plan = GcSweepPlan {
                    workspace,
                    organization,
                    digest,
                    epoch,
                    marked_epoch: entry.gc_epoch.unwrap_or(epoch),
                };
                match store.sweep_candidate(&plan).await {
                    Ok(()) => report.swept += 1,
                    // Losing the fence always means the body survives. That is an
                    // outcome, not a failure: another writer reached it first.
                    Err(StoreError::PreconditionFailed { .. }) => report.lost_fence += 1,
                    Err(error) => return Err(error),
                }
            }
        }
    }
    Ok(report)
}

/// Executes one queue batch of fenced object deletes.
///
/// Every record that succeeds is **acknowledged**. Only records that genuinely
/// have to be retried are reported as batch-item failures — the previous
/// implementation reported all of them unconditionally and touched S3 never, so
/// the queue drained to its dead-letter queue and nothing was ever reclaimed.
///
/// # Errors
///
/// Never: every per-record failure is carried in [`DeleteReport::failed`], which
/// is what SQS partial-batch reporting expects.
pub async fn run_delete_batch<S: ContentMetadataStore + ?Sized, O: DeletableObjects + ?Sized>(
    store: &S,
    objects: &O,
    expected_bucket_owner: &str,
    records: &[(String, String)],
    now: Timestamp,
) -> DeleteReport {
    let mut report = DeleteReport::default();
    for (message_id, body) in records {
        match delete_one(store, objects, expected_bucket_owner, body, now).await {
            Ok(ObjectDeleteResult::Deleted) => report.deleted += 1,
            Ok(ObjectDeleteResult::Missing) => report.already_absent += 1,
            Ok(ObjectDeleteResult::PreconditionFailed) => report.kept += 1,
            Ok(ObjectDeleteResult::Retryable) | Err(_) => {
                report.failed.push(message_id.clone());
            }
        }
    }
    report
}

/// Why one delete record could not be executed.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum DeleteRecordError {
    /// The body was not the declared shape.
    #[error("the delete message is not the declared shape: {0}")]
    Malformed(String),
    /// A decision refused the record.
    #[error(transparent)]
    Lifecycle(#[from] LifecycleError),
    /// The durable terminalisation failed.
    #[error("the content row could not be terminalised: {0}")]
    Store(String),
    /// The provider failed.
    #[error("the object store failed: {0}")]
    Provider(String),
}

async fn delete_one<S: ContentMetadataStore + ?Sized, O: DeletableObjects + ?Sized>(
    store: &S,
    objects: &O,
    expected_bucket_owner: &str,
    body: &str,
    now: Timestamp,
) -> Result<ObjectDeleteResult, DeleteRecordError> {
    let message: DeleteMessage = serde_json::from_str(body)
        .map_err(|error| DeleteRecordError::Malformed(error.to_string()))?;
    let workspace = WorkspaceId::parse(&message.workspace_id)
        .map_err(|error| DeleteRecordError::Malformed(error.to_string()))?;
    let organization = OrganizationId::parse(&message.organization_id)
        .map_err(|error| DeleteRecordError::Malformed(error.to_string()))?;
    let digest = ContentHash::parse(&message.digest)
        .map_err(|error| DeleteRecordError::Malformed(error.to_string()))?;

    // The reachability recheck is strongly consistent and is taken *now*, not
    // trusted from the message: a pin taken since the mark must keep the body.
    let reachability = store
        .reachability(workspace, &digest, now)
        .await
        .map_err(|error| DeleteRecordError::Store(error.to_string()))?;
    let mut item = ContentItem::staged(
        message.digest.clone(),
        message.object_key.clone(),
        message.object_etag.clone(),
        message.staged_at_ms,
        message.epoch,
    )?;
    item.state = ContentState::OrphanConfirmed;
    item.grant_pins = u32::try_from(reachability.unexpired_grants).unwrap_or(u32::MAX);
    item.direct_pins = u32::try_from(reachability.pins).unwrap_or(u32::MAX);

    let planned = plan_delete(
        &item,
        message.fence,
        DeletionDenial::PurgedClosure,
        now.unix_millis(),
        expected_bucket_owner,
    )?;
    let intent = match planned {
        // Both mean the body survives and the record is done with.
        DeleteOutcome::RestoreLive | DeleteOutcome::Deferred => {
            return Ok(ObjectDeleteResult::PreconditionFailed);
        }
        DeleteOutcome::Deleted => return Ok(ObjectDeleteResult::Missing),
        DeleteOutcome::Delete(intent) => intent,
    };

    let result = objects
        .delete_fenced(&intent)
        .await
        .map_err(DeleteRecordError::Provider)?;

    // The object effect is irreversible and goes first; the durable write that
    // records it follows and is conditional.
    item.state = ContentState::Deleting {
        fence: message.fence,
    };
    if apply_delete_result(&mut item, message.fence, result)? == DeleteOutcome::Deleted {
        let plan = GcSweepPlan {
            workspace,
            organization,
            digest,
            epoch: message.epoch,
            marked_epoch: message.marked_epoch,
        };
        store
            .sweep_candidate(&plan)
            .await
            .map_err(|error| DeleteRecordError::Store(error.to_string()))?;
    }
    Ok(result)
}

fn admit(request: &SweepRequest) -> Result<Vec<WorkspaceId>, StoreError> {
    if request.workspace_ids.is_empty() || request.buckets.is_empty() {
        return Err(StoreError::Invalid {
            detail: "a sweep invocation must name at least one workspace and one bucket; \
                     sweeping nothing silently is how a broken collector looks healthy"
                .to_owned(),
        });
    }
    request
        .workspace_ids
        .iter()
        .map(|id| {
            WorkspaceId::parse(id).map_err(|error| StoreError::Invalid {
                detail: error.to_string(),
            })
        })
        .collect()
}

/// Projects one scan row onto the lifecycle authority shape.
///
/// A row with no object key or no `ETag` has nothing a fenced delete could
/// condition on, so it is never a candidate.
fn as_content_item(entry: &GcScanEntry, state: ContentState) -> Option<ContentItem> {
    let key = entry.object_key.clone()?;
    let etag = entry.object_etag.clone()?;
    let mut item = ContentItem::staged(
        entry.digest.clone(),
        key,
        etag,
        entry.not_before.map_or(0, Timestamp::unix_millis),
        entry.gc_epoch.unwrap_or_default(),
    )
    .ok()?;
    item.state = state;
    Some(item)
}

#[cfg(test)]
mod tests {
    use super::{DeleteMessage, SweepRequest, admit};

    #[test]
    fn a_delete_message_names_the_exact_object_and_never_derives_one() {
        let body = serde_json::json!({
            "workspaceId": "wks_01J0000000000000000000000",
            "organizationId": "org_01J0000000000000000000000",
            "digest": "sha256:0000000000000000000000000000000000000000000000000000000000000000",
            "objectKey": "wks/00/00/0000",
            "objectEtag": "\"marked\"",
            "markedEpoch": 3,
            "epoch": 4,
            "stagedAtMs": 0,
            "fence": 7
        });
        let message: DeleteMessage = serde_json::from_value(body).expect("the declared shape");
        assert_eq!(message.object_key, "wks/00/00/0000");
        assert_eq!(message.object_etag, "\"marked\"");
    }

    #[test]
    fn an_unknown_member_is_refused_rather_than_ignored() {
        let body = serde_json::json!({
            "workspaceId": "wks_01J0000000000000000000000",
            "organizationId": "org_01J0000000000000000000000",
            "digest": "sha256:0000000000000000000000000000000000000000000000000000000000000000",
            "objectKey": "wks/00/00/0000",
            "objectEtag": "\"marked\"",
            "markedEpoch": 3,
            "epoch": 4,
            "stagedAtMs": 0,
            "fence": 7,
            "alsoDeleteEverythingElse": true
        });
        assert!(serde_json::from_value::<DeleteMessage>(body).is_err());
    }

    #[test]
    fn a_sweep_that_names_nothing_refuses_instead_of_sweeping_nothing() {
        assert!(
            admit(&SweepRequest {
                workspace_ids: Vec::new(),
                buckets: vec![0],
            })
            .is_err()
        );
        assert!(
            admit(&SweepRequest {
                workspace_ids: vec!["wks_01J0000000000000000000000".to_owned()],
                buckets: Vec::new(),
            })
            .is_err()
        );
    }
}
