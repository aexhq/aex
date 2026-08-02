//! Conservative content lifecycle decisions and exact fenced deletes.

pub mod config;

pub use config::{Config, Mode as DeployedMode};

/// Twenty-four-hour staged-object grace.
pub const GRACE_MILLIS: i64 = 24 * 60 * 60 * 1_000;

/// Maximum concurrent grant+pin expiry transactions in one Lambda invocation.
pub const MAX_EXPIRY_WRITES_IN_FLIGHT: usize = 16;

/// Durable lifecycle state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContentState {
    /// Reachable content.
    Live,
    /// Awaiting the orphan grace.
    Staged,
    /// Rechecks passed and delete work may be claimed.
    OrphanConfirmed,
    /// Exact object delete is in flight under this fence.
    Deleting {
        /// Claim fence.
        fence: u64,
    },
    /// Object was absent or conditionally deleted under this fence.
    Deleted {
        /// Claim fence.
        fence: u64,
    },
}

/// Strongly read content authority projection.
#[derive(Clone, PartialEq, Eq)]
pub struct ContentItem {
    /// Stable content identity.
    pub content_id: String,
    /// Exact unversioned object key. Never rendered by `Debug`.
    pub object_key: String,
    /// Stored object entity tag.
    pub etag: String,
    /// Durable lifecycle state.
    pub state: ContentState,
    /// Original staged time.
    pub staged_at_ms: i64,
    /// Monotonic GC epoch.
    pub gc_epoch: u64,
    /// Direct owner edges.
    pub owner_edges: u32,
    /// Workspace/session root pins.
    pub root_pins: u32,
    /// Unexpired grant pins.
    pub grant_pins: u32,
    /// Active operation roots.
    pub operation_roots: u32,
}

impl std::fmt::Debug for ContentItem {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ContentItem")
            .field("content_id", &self.content_id)
            .field("object_key", &"<redacted>")
            .field("etag", &self.etag)
            .field("state", &self.state)
            .field("staged_at_ms", &self.staged_at_ms)
            .field("gc_epoch", &self.gc_epoch)
            .field("owner_edges", &self.owner_edges)
            .field("root_pins", &self.root_pins)
            .field("grant_pins", &self.grant_pins)
            .field("operation_roots", &self.operation_roots)
            .finish()
    }
}

impl ContentItem {
    /// Creates a staged object authority row.
    ///
    /// # Errors
    ///
    /// Returns [`LifecycleError::InvalidItem`] for empty identities or tags.
    pub fn staged(
        content_id: impl Into<String>,
        object_key: impl Into<String>,
        etag: impl Into<String>,
        staged_at_ms: i64,
        gc_epoch: u64,
    ) -> Result<Self, LifecycleError> {
        let content_id = content_id.into();
        let object_key = object_key.into();
        let etag = etag.into();
        if content_id.is_empty() || object_key.is_empty() || etag.is_empty() {
            return Err(LifecycleError::InvalidItem);
        }
        Ok(Self {
            content_id,
            object_key,
            etag,
            state: ContentState::Staged,
            staged_at_ms,
            gc_epoch,
            owner_edges: 0,
            root_pins: 0,
            grant_pins: 0,
            operation_roots: 0,
        })
    }

    const fn has_reachability(&self) -> bool {
        self.owner_edges > 0
            || self.root_pins > 0
            || self.grant_pins > 0
            || self.operation_roots > 0
    }
}

/// Conservative staged-object reconciliation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReconcileOutcome {
    /// Grace has not elapsed.
    KeepStaged,
    /// A reachability edge reappeared.
    RestoreLive,
    /// Grace elapsed and every pin is absent.
    ConfirmOrphan,
}

/// Rechecks all reachability and the exact 24-hour boundary.
#[must_use]
pub const fn reconcile_staged(item: &ContentItem, now_ms: i64) -> ReconcileOutcome {
    if item.has_reachability() {
        return ReconcileOutcome::RestoreLive;
    }
    if now_ms.saturating_sub(item.staged_at_ms) < GRACE_MILLIS {
        return ReconcileOutcome::KeepStaged;
    }
    ReconcileOutcome::ConfirmOrphan
}

/// Strong deletion-denial projection result.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeletionDenial {
    /// Purge may exist centrally but is not projected yet.
    PendingProjection,
    /// Owning closure remains live.
    LiveClosure,
    /// Content-free denial is durable and projected.
    PurgedClosure,
}

/// Exact conditional request passed to the S3 adapter.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeleteIntent {
    /// Exact unversioned key.
    pub key: String,
    /// `x-amz-if-match` value.
    pub if_match: String,
    /// Explicit plane account id.
    pub expected_bucket_owner: String,
    /// Claim fence.
    pub fence: u64,
}

/// Result of a lifecycle decision or delete effect.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeleteOutcome {
    /// Re-arm the due index; no object call.
    Deferred,
    /// Restore the content row to live; no object call.
    RestoreLive,
    /// Execute exactly one conditional object delete.
    Delete(DeleteIntent),
    /// Content row may terminalize as deleted.
    Deleted,
}

/// Rechecks state, pins, grace and denial before constructing the exact request.
///
/// # Errors
///
/// Returns [`LifecycleError`] for a stale state or missing bucket owner.
pub fn plan_delete(
    item: &ContentItem,
    fence: u64,
    denial: DeletionDenial,
    now_ms: i64,
    expected_bucket_owner: &str,
) -> Result<DeleteOutcome, LifecycleError> {
    if item.state != ContentState::OrphanConfirmed {
        return Err(LifecycleError::WrongState);
    }
    if item.has_reachability() {
        return Ok(DeleteOutcome::RestoreLive);
    }
    if now_ms.saturating_sub(item.staged_at_ms) < GRACE_MILLIS {
        return Ok(DeleteOutcome::Deferred);
    }
    match denial {
        DeletionDenial::PendingProjection => Ok(DeleteOutcome::Deferred),
        DeletionDenial::LiveClosure => Ok(DeleteOutcome::RestoreLive),
        DeletionDenial::PurgedClosure => {
            if expected_bucket_owner.is_empty()
                || !expected_bucket_owner
                    .bytes()
                    .all(|byte| byte.is_ascii_digit())
            {
                return Err(LifecycleError::InvalidBucketOwner);
            }
            Ok(DeleteOutcome::Delete(DeleteIntent {
                key: item.object_key.clone(),
                if_match: item.etag.clone(),
                expected_bucket_owner: expected_bucket_owner.to_owned(),
                fence,
            }))
        }
    }
}

/// Result from the conditional object adapter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ObjectDeleteResult {
    /// Exact `ETag` matched and object was deleted.
    Deleted,
    /// Object was already absent; this is idempotent success.
    Missing,
    /// Key now names different bytes and must not be deleted.
    PreconditionFailed,
    /// Provider failure may be retried.
    Retryable,
}

/// Applies an object result only under the deleting fence.
///
/// # Errors
///
/// Returns [`LifecycleError::StaleFence`] for any other fence or state.
pub fn apply_delete_result(
    item: &mut ContentItem,
    fence: u64,
    result: ObjectDeleteResult,
) -> Result<DeleteOutcome, LifecycleError> {
    if item.state != (ContentState::Deleting { fence }) {
        return Err(LifecycleError::StaleFence);
    }
    match result {
        ObjectDeleteResult::Deleted | ObjectDeleteResult::Missing => {
            item.state = ContentState::Deleted { fence };
            Ok(DeleteOutcome::Deleted)
        }
        ObjectDeleteResult::PreconditionFailed | ObjectDeleteResult::Retryable => {
            Ok(DeleteOutcome::Deferred)
        }
    }
}

/// One deployed binary role.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// Upload/grant expiry.
    Expiry,
    /// Staged-orphan and inventory reconciliation.
    Reconcile,
    /// Reachability mark/sweep.
    MarkSweep,
    /// Fenced exact-key deletion.
    Delete,
}

/// Capability admitted for one role-specific deployment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ModeAdmission {
    /// Whether this role may link object deletion.
    pub may_delete_objects: bool,
}

/// Rejects both missing and unnecessarily broad delete capability.
///
/// # Errors
///
/// Returns [`ModeError`] when capability and mode disagree.
pub const fn admit_mode(
    mode: Mode,
    declares_content_delete: bool,
) -> Result<ModeAdmission, ModeError> {
    match (mode, declares_content_delete) {
        (Mode::Delete, true) => Ok(ModeAdmission {
            may_delete_objects: true,
        }),
        (Mode::Delete, false) => Err(ModeError::DeleteCapabilityMissing),
        (Mode::Expiry | Mode::Reconcile | Mode::MarkSweep, true) => {
            Err(ModeError::UnexpectedDeleteCapability)
        }
        (Mode::Expiry | Mode::Reconcile | Mode::MarkSweep, false) => Ok(ModeAdmission {
            may_delete_objects: false,
        }),
    }
}

/// Invalid mode/capability pairing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum ModeError {
    /// Delete role omitted the compile/runtime capability.
    #[error("delete mode requires ContentObjectDelete")]
    DeleteCapabilityMissing,
    /// A non-delete role was over-privileged.
    #[error("this mode must not declare ContentObjectDelete")]
    UnexpectedDeleteCapability,
}

/// Invalid content state or configuration.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum LifecycleError {
    /// Required authority fields were empty.
    #[error("content item is invalid")]
    InvalidItem,
    /// Item was not orphan-confirmed.
    #[error("content item is not ready for deletion")]
    WrongState,
    /// Expected bucket owner was absent or malformed.
    #[error("expected bucket owner is invalid")]
    InvalidBucketOwner,
    /// Result did not match the deleting fence.
    #[error("content deletion fence is stale")]
    StaleFence,
}
