//! Fenced, bounded continuation kernel for regional session operations.

use serde::{Deserialize, Serialize};
use xxhash_rust::xxh3::xxh3_64;

/// Poison-work ceiling.
pub const MAX_ATTEMPTS: u32 = 8;

/// The only cross-invocation kinds this worker owns.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkKind {
    /// Session purge leg after trash.
    SessionDelete,
    /// Regional leg of workspace purge.
    WorkspaceDelete,
    /// One oversized persist staging page.
    SessionPersistPage,
    /// One oversized fork staging page.
    SessionForkPage,
}

impl WorkKind {
    /// Parses the closed ownership vocabulary.
    ///
    /// # Errors
    ///
    /// Returns [`WorkError::UnownedKind`] for inline or peer-owned work.
    pub fn parse(value: &str) -> Result<Self, WorkError> {
        match value {
            "session_delete" => Ok(Self::SessionDelete),
            "workspace_delete" => Ok(Self::WorkspaceDelete),
            "session_persist_page" => Ok(Self::SessionPersistPage),
            "session_fork_page" => Ok(Self::SessionForkPage),
            _ => Err(WorkError::UnownedKind),
        }
    }

    const fn domain(self) -> &'static str {
        match self {
            Self::SessionDelete | Self::WorkspaceDelete => "session_operation.purge",
            Self::SessionPersistPage => "session_operation.persist_page",
            Self::SessionForkPage => "session_operation.fork_page",
        }
    }
}

/// Durable state of one continuation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkStatus {
    /// Available in the due index.
    Due,
    /// Held under a live claim.
    Claimed,
    /// All pages committed.
    Completed,
    /// Poison work removed from the due index.
    ManualReview,
}

/// Durable work authority projection used by the kernel.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkRecord {
    /// Stable work key.
    pub work_id: String,
    /// Durable operation identity.
    pub operation_id: String,
    /// Closed owner domain.
    pub kind: WorkKind,
    /// Next page cursor.
    pub cursor: u64,
    /// Total bounded pages.
    pub total: u64,
    /// Current monotonic fence.
    pub fence: u64,
    /// Current lease owner.
    pub owner: Option<String>,
    /// Exclusive lease expiry.
    pub lease_expires_at_ms: i64,
    /// Poison attempt count.
    pub attempts: u32,
    /// Error kind only, never provider text.
    pub error_kind: Option<String>,
    /// Durable status.
    pub status: WorkStatus,
}

impl WorkRecord {
    /// Creates due work at cursor zero.
    ///
    /// # Errors
    ///
    /// Returns [`WorkError::InvalidRecord`] for empty ids or zero pages.
    pub fn new(
        work_id: impl Into<String>,
        operation_id: impl Into<String>,
        kind: WorkKind,
        total: u64,
    ) -> Result<Self, WorkError> {
        let work_id = work_id.into();
        let operation_id = operation_id.into();
        if work_id.is_empty() || operation_id.is_empty() || total == 0 {
            return Err(WorkError::InvalidRecord);
        }
        Ok(Self {
            work_id,
            operation_id,
            kind,
            cursor: 0,
            total,
            fence: 0,
            owner: None,
            lease_expires_at_ms: 0,
            attempts: 0,
            error_kind: None,
            status: WorkStatus::Due,
        })
    }
}

/// Monotonic ownership proof for one cursor position.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Claim {
    /// Work key.
    pub work_id: String,
    /// Owner id.
    pub owner: String,
    /// Monotonic fence.
    pub fence: u64,
    /// Cursor observed at claim.
    pub cursor: u64,
    /// Exclusive lease expiry.
    pub lease_expires_at_ms: i64,
}

/// Claims or takes over expired work and increments the fence.
///
/// # Errors
///
/// Returns [`ClaimError`] for a live lease, terminal state or invalid duration.
pub fn claim(
    work: &mut WorkRecord,
    owner: impl Into<String>,
    now_ms: i64,
    lease_ms: i64,
) -> Result<Claim, ClaimError> {
    if lease_ms <= 0 {
        return Err(ClaimError::InvalidLease);
    }
    if matches!(
        work.status,
        WorkStatus::Completed | WorkStatus::ManualReview
    ) {
        return Err(ClaimError::Terminal);
    }
    if work.status == WorkStatus::Claimed && now_ms < work.lease_expires_at_ms {
        return Err(ClaimError::LeaseHeld);
    }
    let owner = owner.into();
    if owner.is_empty() {
        return Err(ClaimError::InvalidOwner);
    }
    work.fence = work
        .fence
        .checked_add(1)
        .ok_or(ClaimError::FenceExhausted)?;
    work.owner = Some(owner.clone());
    work.lease_expires_at_ms = now_ms.saturating_add(lease_ms);
    work.status = WorkStatus::Claimed;
    Ok(Claim {
        work_id: work.work_id.clone(),
        owner,
        fence: work.fence,
        cursor: work.cursor,
        lease_expires_at_ms: work.lease_expires_at_ms,
    })
}

/// One bounded effect planned outside the authority transaction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StepPlan {
    /// Stable BLAKE3 effect identity.
    pub effect_id: String,
    /// Cursor this effect consumes.
    pub cursor: u64,
}

/// Derives the one effect allowed for a claimed page.
///
/// # Errors
///
/// Returns [`CommitError`] when the claim is stale or work is not claimed.
pub fn plan_step(work: &WorkRecord, claim: &Claim) -> Result<StepPlan, CommitError> {
    validate_claim(work, claim)?;
    let mut digest = blake3::Hasher::new();
    digest.update(work.kind.domain().as_bytes());
    digest.update(&[0x1f]);
    digest.update(work.operation_id.as_bytes());
    digest.update(&[0x1f]);
    digest.update(&work.cursor.to_be_bytes());
    digest.update(&claim.fence.to_be_bytes());
    Ok(StepPlan {
        effect_id: digest.finalize().to_hex().to_string(),
        cursor: work.cursor,
    })
}

/// Commits a receipt and advances one page under the claim fence.
///
/// # Errors
///
/// Returns [`CommitError`] for stale, duplicate or mismatched commits.
pub fn commit_step(
    work: &mut WorkRecord,
    claim: &Claim,
    effect_id: &str,
) -> Result<(), CommitError> {
    if work.cursor != claim.cursor {
        return Err(CommitError::AlreadyAdvanced);
    }
    validate_claim(work, claim)?;
    if plan_step(work, claim)?.effect_id != effect_id {
        return Err(CommitError::EffectMismatch);
    }
    work.cursor = work
        .cursor
        .checked_add(1)
        .ok_or(CommitError::CursorExhausted)?;
    work.owner = None;
    work.lease_expires_at_ms = 0;
    work.status = if work.cursor == work.total {
        WorkStatus::Completed
    } else {
        WorkStatus::Due
    };
    Ok(())
}

fn validate_claim(work: &WorkRecord, claim: &Claim) -> Result<(), CommitError> {
    if work.fence != claim.fence || work.owner.as_deref() != Some(claim.owner.as_str()) {
        return Err(CommitError::StaleFence);
    }
    if work.status != WorkStatus::Claimed {
        return Err(CommitError::NotClaimed);
    }
    Ok(())
}

/// Records a typed failure and terminalizes poison work at exactly eight attempts.
///
/// # Errors
///
/// Returns [`WorkError::AttemptExhausted`] if called after manual review.
pub fn record_failure(work: &mut WorkRecord, kind: &str) -> Result<(), WorkError> {
    if work.status == WorkStatus::ManualReview {
        return Err(WorkError::AttemptExhausted);
    }
    work.attempts = work
        .attempts
        .checked_add(1)
        .ok_or(WorkError::AttemptExhausted)?;
    work.owner = None;
    work.lease_expires_at_ms = 0;
    if work.attempts >= MAX_ATTEMPTS {
        work.status = WorkStatus::ManualReview;
        work.error_kind = Some("operation_manual_review".to_owned());
    } else {
        work.status = WorkStatus::Due;
        work.error_kind = Some(kind.to_owned());
    }
    Ok(())
}

/// Deterministic domain-local due shard.
///
/// # Errors
///
/// Returns [`WorkError::InvalidShardCount`] for zero shards.
pub fn due_shard(work_id: &str, shards: u64) -> Result<u64, WorkError> {
    if shards == 0 {
        return Err(WorkError::InvalidShardCount);
    }
    Ok(xxh3_64(work_id.as_bytes()) % shards)
}

/// One SQS item outcome.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BatchItem {
    /// Committed and safe to acknowledge.
    Succeeded(String),
    /// Must be retried independently.
    Failed(String),
}

/// Lambda SQS partial-batch response model.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct BatchResponse {
    /// Failed message ids only.
    pub batch_item_failures: Vec<String>,
}

/// Builds a partial-batch response without throwing successful items away.
#[must_use]
pub fn batch_response(items: &[BatchItem]) -> BatchResponse {
    BatchResponse {
        batch_item_failures: items
            .iter()
            .filter_map(|item| match item {
                BatchItem::Succeeded(_) => None,
                BatchItem::Failed(id) => Some(id.clone()),
            })
            .collect(),
    }
}

/// State of the content-free deletion-denial fact.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeletionDenial {
    /// Fact is absent.
    Missing,
    /// Central fact exists but regional projection has not caught up.
    DurableNotProjected,
    /// Fact is durable and consistently visible regionally.
    DurableAndProjected,
}

/// A tombstone is legal only after the denial is both durable and projected.
#[must_use]
pub const fn may_commit_tombstone(denial: DeletionDenial) -> bool {
    matches!(denial, DeletionDenial::DurableAndProjected)
}

/// Invalid work input or terminal poison state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum WorkError {
    /// Work kind belongs inline or to another deployable.
    #[error("work kind is not owned by session-operation-worker")]
    UnownedKind,
    /// Id or total-page count was invalid.
    #[error("work record is invalid")]
    InvalidRecord,
    /// Shard count was zero.
    #[error("due shard count must be positive")]
    InvalidShardCount,
    /// Failure was recorded after terminal manual review.
    #[error("work already exhausted its attempts")]
    AttemptExhausted,
}

/// Claim refusal.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum ClaimError {
    /// Another owner has a live lease.
    #[error("work lease is held")]
    LeaseHeld,
    /// Work is already terminal.
    #[error("work is terminal")]
    Terminal,
    /// Lease duration was non-positive.
    #[error("lease duration is invalid")]
    InvalidLease,
    /// Owner was empty.
    #[error("worker owner is invalid")]
    InvalidOwner,
    /// Fence counter overflowed.
    #[error("work fence is exhausted")]
    FenceExhausted,
}

/// Fenced commit refusal.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum CommitError {
    /// Fence or owner no longer matches.
    #[error("claim fence is stale")]
    StaleFence,
    /// Work is not currently claimed.
    #[error("work is not claimed")]
    NotClaimed,
    /// Cursor already advanced after this hint was emitted.
    #[error("work cursor already advanced")]
    AlreadyAdvanced,
    /// Receipt names another effect.
    #[error("effect identity does not match")]
    EffectMismatch,
    /// Cursor counter overflowed.
    #[error("work cursor is exhausted")]
    CursorExhausted,
}
