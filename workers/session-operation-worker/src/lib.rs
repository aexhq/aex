//! Fenced, bounded continuation kernel for regional session operations.

pub mod config;

use serde::{Deserialize, Serialize};
use xxhash_rust::xxh3::xxh3_64;

use aex_operation_domain::OperationStatus;
use aex_session_dynamodb::StoreError;
use aex_session_dynamodb::plan::{Participant, TransactionPlan};
use aex_session_dynamodb::transactions::{OperationStepCancel, operation_cancelled};
use aex_wire::ids::{OperationId, PrefixedId, SessionId, WorkspaceId};
use aex_wire::types::Timestamp;
use aex_work_dynamodb::WorkClaim;
use aex_work_dynamodb::codec::ReconciliationCursor;
use aex_work_dynamodb::store::{DuePage, WorkAuthority};

pub use config::Config;

/// Which of the worker's two triggers an invocation carries.
///
/// The classification is structural rather than a configured mode: an SQS batch
/// always carries `Records`, and the scheduled due scan always carries the
/// `aex.due_scan` detail type. Anything else is a failure, because a worker that
/// answers success to a payload it did not understand drains its queue without
/// doing any work.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Trigger {
    /// An SQS hint batch.
    Queue,
    /// The scheduled sharded due scan.
    DueScan,
    /// Neither.
    Unknown,
}

impl Trigger {
    /// Classifies one raw invocation payload.
    #[must_use]
    pub fn classify(payload: &serde_json::Value) -> Self {
        if payload
            .get("Records")
            .is_some_and(serde_json::Value::is_array)
        {
            return Self::Queue;
        }
        if payload
            .get("detail-type")
            .and_then(serde_json::Value::as_str)
            == Some(DUE_SCAN_DETAIL_TYPE)
        {
            return Self::DueScan;
        }
        Self::Unknown
    }
}

/// The scheduled event this worker answers a due scan for.
pub const DUE_SCAN_DETAIL_TYPE: &str = "aex.due_scan";

/// Maximum scheduled shard pipelines allowed to hold AWS requests in flight.
pub const DUE_SHARD_CONCURRENCY: usize = 16;

/// Non-authoritative routing fields projected from a committed
/// `regional-work` row onto SQS.
///
/// Every field is rechecked against a strongly consistent base-table read
/// before the hint can authorize a state change.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkHint {
    /// Durable work identity.
    pub work_id: String,
    /// Tenant used for the authority read.
    pub workspace: WorkspaceId,
}

impl WorkHint {
    /// Decodes the `EventBridge` Pipe projection carried in an SQS body.
    ///
    /// # Errors
    ///
    /// Returns [`ReconcileError::InvalidHint`] for malformed JSON or an empty
    /// work identity.
    pub fn decode(body: &str) -> Result<Self, ReconcileError> {
        let hint: Self = serde_json::from_str(body).map_err(|_| ReconcileError::InvalidHint)?;
        if hint.work_id.is_empty() {
            return Err(ReconcileError::InvalidHint);
        }
        Ok(hint)
    }
}

/// The work-table operations required by terminal reconciliation.
#[async_trait::async_trait]
pub trait WorkPort: Send + Sync + 'static {
    /// Strongly reads a work row under an asserted tenant.
    async fn load(
        &self,
        workspace: WorkspaceId,
        work_id: &str,
    ) -> Result<Option<aex_work_dynamodb::codec::WorkRecord>, StoreError>;

    /// Claims or takes over one due row.
    async fn claim(
        &self,
        work_id: &str,
        owner: &str,
        now: Timestamp,
        lease_until: Timestamp,
    ) -> Result<WorkClaim, StoreError>;

    /// Retires the exact claim.
    async fn complete(&self, hold: &WorkClaim, now: Timestamp) -> Result<(), StoreError>;

    /// Reads one bounded due-index page.
    async fn scan_due(
        &self,
        shard: u16,
        now: Timestamp,
        budget: aex_session_dynamodb::paging::PageBudget,
    ) -> Result<DuePage, StoreError>;

    /// Strongly loads one shard's durable scan position.
    async fn load_cursor(&self, shard: u16) -> Result<Option<ReconciliationCursor>, StoreError>;

    /// Reads a bounded page strictly after a durable scan position.
    async fn scan_due_after(
        &self,
        shard: u16,
        now: Timestamp,
        budget: aex_session_dynamodb::paging::PageBudget,
        after: Option<&ReconciliationCursor>,
    ) -> Result<DuePage, StoreError>;

    /// Conditionally persists one wrapping scan position.
    async fn advance_cursor(&self, cursor: &ReconciliationCursor) -> Result<(), StoreError>;
}

#[async_trait::async_trait]
impl WorkPort for aex_work_dynamodb::store::WorkStore {
    async fn load(
        &self,
        workspace: WorkspaceId,
        work_id: &str,
    ) -> Result<Option<aex_work_dynamodb::codec::WorkRecord>, StoreError> {
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

    async fn scan_due(
        &self,
        shard: u16,
        now: Timestamp,
        budget: aex_session_dynamodb::paging::PageBudget,
    ) -> Result<DuePage, StoreError> {
        WorkAuthority::scan_due(self, shard, now, budget).await
    }

    async fn load_cursor(&self, shard: u16) -> Result<Option<ReconciliationCursor>, StoreError> {
        WorkAuthority::load_cursor(self, shard).await
    }

    async fn scan_due_after(
        &self,
        shard: u16,
        now: Timestamp,
        budget: aex_session_dynamodb::paging::PageBudget,
        after: Option<&ReconciliationCursor>,
    ) -> Result<DuePage, StoreError> {
        WorkAuthority::scan_due_after(self, shard, now, budget, after).await
    }

    async fn advance_cursor(&self, cursor: &ReconciliationCursor) -> Result<(), StoreError> {
        WorkAuthority::advance_cursor(self, cursor).await
    }
}

/// Plans the next durable position for one bounded due-shard page.
///
/// A full page advances past its last key. A page that reaches the end, or an
/// empty page read after an existing position, wraps to the start by clearing
/// `last_work_id`. That makes a permanently deferred row revisit-able without
/// allowing it to pin every later row behind the first page.
///
/// # Errors
///
/// [`StoreError::Invalid`] for a cross-shard cursor, a malformed page boundary,
/// or a revision that cannot advance.
pub fn next_due_cursor(
    shard: u16,
    current: Option<&ReconciliationCursor>,
    page: &DuePage,
    now: Timestamp,
) -> Result<Option<ReconciliationCursor>, StoreError> {
    if current.is_some_and(|position| position.shard != shard) {
        return Err(StoreError::Invalid {
            detail: "a due cursor belongs to another shard".to_owned(),
        });
    }
    let last = page.items.last();
    if page.scanned_through != last.map(|item| item.effective_due_at)
        || (page.has_more && last.is_none())
    {
        return Err(StoreError::Invalid {
            detail: "a due page carries an inconsistent continuation boundary".to_owned(),
        });
    }
    let had_position = current.is_some_and(|position| position.last_work_id.is_some());
    if last.is_none() && !had_position {
        return Ok(None);
    }
    let revision = current.map_or(Ok(1), |position| position.revision.checked_add(1).ok_or(()));
    let revision = revision.map_err(|()| StoreError::Invalid {
        detail: "a due cursor revision cannot advance past u64::MAX".to_owned(),
    })?;
    let last_work_id = if page.has_more {
        Some(
            last.ok_or_else(|| StoreError::Invalid {
                detail: "a continuing due page carries no last row".to_owned(),
            })?
            .work_id
            .clone(),
        )
    } else {
        None
    };
    Ok(Some(ReconciliationCursor {
        shard,
        scanned_through_effective_due_at: page.scanned_through.unwrap_or(now),
        last_work_id,
        revision,
        updated_at: now,
    }))
}

/// Operation fields that must agree with an `operation.step` row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OperationSnapshot {
    /// Durable operation identity.
    pub id: OperationId,
    /// Owning workspace.
    pub workspace: WorkspaceId,
    /// Owning session for session-scoped continuations.
    pub session: Option<SessionId>,
    /// Current monotonic status.
    pub status: OperationStatus,
    /// Optimistic store version.
    pub version: u64,
    /// Whether the public cancel command was durably accepted.
    pub cancel_requested: bool,
    /// Destructive point-of-no-return latch, when already crossed.
    pub committed_at: Option<Timestamp>,
}

/// The operation authority required before and during a continuation step.
#[async_trait::async_trait]
pub trait OperationPort: Send + Sync + 'static {
    /// Strongly reloads the operation.
    async fn load(
        &self,
        workspace: WorkspaceId,
        operation: OperationId,
    ) -> Result<Option<OperationSnapshot>, StoreError>;

    /// Atomically terminalizes an observed cancel request and retires the
    /// exact fenced work claim.
    async fn cancel_step(
        &self,
        operation: &OperationSnapshot,
        hold: &WorkClaim,
        now: Timestamp,
    ) -> Result<(), StoreError>;
}

/// Real cross-table operation-step authority.
#[derive(Debug, Clone)]
pub struct DynamoOperationPort {
    operations: aex_session_dynamodb::store::OperationStore,
    work_table: String,
}

impl DynamoOperationPort {
    /// Binds the session and work tables used by one atomic step commit.
    #[must_use]
    pub fn new(
        client: aws_sdk_dynamodb::Client,
        session_table: impl Into<String>,
        work_table: impl Into<String>,
    ) -> Self {
        Self {
            operations: aex_session_dynamodb::store::OperationStore::new(client, session_table),
            work_table: work_table.into(),
        }
    }
}

#[async_trait::async_trait]
impl OperationPort for DynamoOperationPort {
    async fn load(
        &self,
        workspace: WorkspaceId,
        operation: OperationId,
    ) -> Result<Option<OperationSnapshot>, StoreError> {
        Ok(aex_session_dynamodb::store::OperationAuthority::load(
            &self.operations,
            workspace,
            operation,
        )
        .await?
        .map(|stored| OperationSnapshot {
            id: stored.record.id,
            workspace: stored.record.workspace,
            session: stored.record.session,
            status: stored.record.status,
            version: stored.version,
            cancel_requested: stored.record.cancel_requested,
            committed_at: stored.record.committed_at,
        }))
    }

    async fn cancel_step(
        &self,
        operation: &OperationSnapshot,
        hold: &WorkClaim,
        now: Timestamp,
    ) -> Result<(), StoreError> {
        let plan = compile_cancelled_step(
            self.operations.table(),
            &self.work_table,
            operation,
            hold,
            now,
        )?;
        self.operations.commit_cancelled_step(&plan).await
    }
}

/// Compiles the two-row transaction for `StepOutcome::Cancelled`.
///
/// The `cancel:<operationId>` token is stable for the logical transition and
/// exactly fits `DynamoDB`'s 36-byte client-token ceiling for the canonical
/// operation identity. Durable correctness still comes from both conditions,
/// not from the provider's short transport-deduplication window.
///
/// # Errors
///
/// [`StoreError::Invalid`] when the snapshot is not the running,
/// cancellation-requested, pre-commit state this transition owns, or when
/// either conditional update cannot be built.
pub fn compile_cancelled_step(
    session_table: &str,
    work_table: &str,
    operation: &OperationSnapshot,
    hold: &WorkClaim,
    now: Timestamp,
) -> Result<TransactionPlan, StoreError> {
    if operation.status != OperationStatus::Running
        || !operation.cancel_requested
        || operation.committed_at.is_some()
    {
        return Err(StoreError::Invalid {
            detail:
                "a cancelled step requires a running, cancellation-requested, uncommitted operation"
                    .to_owned(),
        });
    }
    let mut plan = TransactionPlan::new(format!("cancel:{}", operation.id));
    plan.update(
        Participant::SESSION_OPERATION,
        operation_cancelled(
            session_table,
            &OperationStepCancel {
                workspace: operation.workspace,
                operation: operation.id,
                version: operation.version,
                now,
            },
        )?,
    )?;
    plan.update(
        Participant::WORK_WAKE_DONE,
        aex_work_dynamodb::claim::complete(work_table, hold, now)?,
    )?;
    Ok(plan)
}

/// Result of reconciling one terminal-operation hint.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReconcileDisposition {
    /// This invocation retired the work row.
    Retired,
    /// A previous invocation already retired it.
    AlreadyRetired,
    /// The owning operation is still nonterminal and belongs to an effect lane.
    Deferred,
}

/// Fenced reconciliation of terminal work and accepted cancellations.
///
/// This is a complete no-effect continuation: it strongly validates the hint,
/// observes the monotonic terminal operation, takes the work fence, and retires
/// the row. An ambiguous retirement is resolved by reading the target; the
/// write is never issued blindly a second time.
#[derive(Debug, Clone)]
pub struct OperationReconciler<W, O> {
    work: W,
    operations: O,
    owner: String,
    lease_ms: i64,
}

impl<W, O> OperationReconciler<W, O>
where
    W: WorkPort,
    O: OperationPort,
{
    /// Binds the two authorities and the claim identity.
    ///
    /// # Errors
    ///
    /// Returns [`ReconcileError::InvalidSettings`] for an empty owner or a
    /// non-positive lease.
    pub fn new(
        work: W,
        operations: O,
        owner: impl Into<String>,
        lease_ms: i64,
    ) -> Result<Self, ReconcileError> {
        let owner = owner.into();
        if owner.is_empty() || lease_ms <= 0 {
            return Err(ReconcileError::InvalidSettings);
        }
        Ok(Self {
            work,
            operations,
            owner,
            lease_ms,
        })
    }

    /// The bound work port, exposed for readiness and deterministic tests.
    #[must_use]
    pub const fn work(&self) -> &W {
        &self.work
    }

    /// The bound operation port, exposed for readiness and deterministic tests.
    #[must_use]
    pub const fn operations(&self) -> &O {
        &self.operations
    }

    /// Reconciles one hint.
    ///
    /// # Errors
    ///
    /// Returns [`ReconcileError`] when either authority is unavailable or the
    /// work and operation identities do not agree exactly.
    pub async fn reconcile(
        &self,
        hint: &WorkHint,
        now: Timestamp,
    ) -> Result<ReconcileDisposition, ReconcileError> {
        let Some(work) = self.work.load(hint.workspace, &hint.work_id).await? else {
            return Err(ReconcileError::MissingWork);
        };
        match work.state.as_str() {
            "done" | "poisoned" => return Ok(ReconcileDisposition::AlreadyRetired),
            "pending" | "claimed" => {}
            _ => return Err(ReconcileError::InvalidWork),
        }
        let binding = OperationBinding::from_work(&work)?;
        let operation = self
            .operations
            .load(hint.workspace, binding.operation)
            .await?
            .ok_or(ReconcileError::MissingOperation)?;
        binding.verify(&operation)?;
        let cancelling = operation.status == OperationStatus::Running
            && operation.cancel_requested
            && operation.committed_at.is_none();
        if !operation.status.is_terminal() && !cancelling {
            return Ok(ReconcileDisposition::Deferred);
        }

        if work.state == "claimed"
            && work
                .lease_expires_at
                .is_some_and(|expires| expires.unix_millis() > now.unix_millis())
        {
            return Ok(ReconcileDisposition::Deferred);
        }
        let lease_until_ms = now
            .unix_millis()
            .checked_add(self.lease_ms)
            .ok_or(ReconcileError::InvalidSettings)?;
        let lease_until = Timestamp::from_unix_millis(lease_until_ms)
            .map_err(|_| ReconcileError::InvalidSettings)?;
        let hold = match self
            .work
            .claim(&hint.work_id, &self.owner, now, lease_until)
            .await
        {
            Ok(hold) => hold,
            Err(StoreError::PreconditionFailed { .. }) => {
                return if cancelling {
                    self.resolve_cancel_after_write(hint, binding).await
                } else {
                    self.resolve_after_write(hint).await
                };
            }
            Err(error) => return Err(error.into()),
        };

        let claimed = self
            .work
            .load(hint.workspace, &hint.work_id)
            .await?
            .ok_or(ReconcileError::MissingWork)?;
        binding.verify_claimed(&claimed, &hold)?;
        let committed = if cancelling {
            self.operations.cancel_step(&operation, &hold, now).await
        } else {
            self.work.complete(&hold, now).await
        };
        match committed {
            Ok(()) => Ok(ReconcileDisposition::Retired),
            Err(StoreError::CommitAmbiguous { .. } | StoreError::PreconditionFailed { .. }) => {
                if cancelling {
                    self.resolve_cancel_after_write(hint, binding).await
                } else {
                    self.resolve_after_write(hint).await
                }
            }
            Err(error) => Err(error.into()),
        }
    }

    async fn resolve_after_write(
        &self,
        hint: &WorkHint,
    ) -> Result<ReconcileDisposition, ReconcileError> {
        match self.work.load(hint.workspace, &hint.work_id).await? {
            None => Ok(ReconcileDisposition::AlreadyRetired),
            Some(work) if matches!(work.state.as_str(), "done" | "poisoned") => {
                Ok(ReconcileDisposition::AlreadyRetired)
            }
            Some(_) => Ok(ReconcileDisposition::Deferred),
        }
    }

    async fn resolve_cancel_after_write(
        &self,
        hint: &WorkHint,
        binding: OperationBinding,
    ) -> Result<ReconcileDisposition, ReconcileError> {
        let operation = self
            .operations
            .load(hint.workspace, binding.operation)
            .await?
            .ok_or(ReconcileError::MissingOperation)?;
        binding.verify(&operation)?;
        let work = self.work.load(hint.workspace, &hint.work_id).await?;
        match (operation.status.is_terminal(), work) {
            (true, None) => Ok(ReconcileDisposition::AlreadyRetired),
            (true, Some(work)) if matches!(work.state.as_str(), "done" | "poisoned") => {
                Ok(ReconcileDisposition::AlreadyRetired)
            }
            (false, None) => Err(ReconcileError::AuthorityMismatch),
            (false, Some(work)) if matches!(work.state.as_str(), "done" | "poisoned") => {
                Err(ReconcileError::AuthorityMismatch)
            }
            _ => Ok(ReconcileDisposition::Deferred),
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct OperationBinding {
    workspace: WorkspaceId,
    operation: OperationId,
    session: SessionId,
    version: u64,
}

impl OperationBinding {
    fn from_work(work: &aex_work_dynamodb::codec::WorkRecord) -> Result<Self, ReconcileError> {
        if work.kind != "operation.step" {
            return Err(ReconcileError::UnownedKind);
        }
        let members = work.payload.members();
        let operation = members
            .get("operationId")
            .and_then(|value| OperationId::parse(value).ok())
            .ok_or(ReconcileError::InvalidWork)?;
        let session = members
            .get("sessionId")
            .and_then(|value| SessionId::parse(value).ok())
            .ok_or(ReconcileError::InvalidWork)?;
        let version = members
            .get("version")
            .and_then(|value| value.parse::<u64>().ok())
            .ok_or(ReconcileError::InvalidWork)?;
        if work.session != Some(session) {
            return Err(ReconcileError::AuthorityMismatch);
        }
        Ok(Self {
            workspace: work.workspace,
            operation,
            session,
            version,
        })
    }

    fn verify(self, operation: &OperationSnapshot) -> Result<(), ReconcileError> {
        if operation.workspace != self.workspace
            || operation.id != self.operation
            || operation.session != Some(self.session)
            || operation.version < self.version
        {
            return Err(ReconcileError::AuthorityMismatch);
        }
        Ok(())
    }

    fn verify_claimed(
        self,
        work: &aex_work_dynamodb::codec::WorkRecord,
        hold: &WorkClaim,
    ) -> Result<(), ReconcileError> {
        let observed = Self::from_work(work)?;
        if observed.operation != self.operation
            || observed.session != self.session
            || observed.version != self.version
            || work.state != "claimed"
            || work.fence != hold.fence
            || work.claim_owner.as_deref() != Some(hold.owner.as_str())
        {
            return Err(ReconcileError::AuthorityMismatch);
        }
        Ok(())
    }
}

/// Terminal reconciliation failure.
#[derive(Debug, thiserror::Error)]
pub enum ReconcileError {
    /// The SQS projection was malformed.
    #[error("the work hint is invalid")]
    InvalidHint,
    /// Worker claim settings were unusable.
    #[error("terminal reconciler settings are invalid")]
    InvalidSettings,
    /// No base row exists under the hint's asserted tenant.
    #[error("the authoritative work row is missing")]
    MissingWork,
    /// The work row does not use the strict operation-step schema.
    #[error("the authoritative work row is invalid")]
    InvalidWork,
    /// The row belongs to another worker domain.
    #[error("the work kind is not owned by the session operation worker")]
    UnownedKind,
    /// The operation row named by the work no longer exists.
    #[error("the authoritative operation row is missing")]
    MissingOperation,
    /// Work payload, tenant, session, version, claim, or operation disagreed.
    #[error("the work and operation authorities do not agree")]
    AuthorityMismatch,
    /// A regional authority call failed.
    #[error(transparent)]
    Store(#[from] StoreError),
}

/// Every deterministic shard the due scan sweeps, in order.
///
/// A literal single `due` partition key is forbidden: it would make the whole
/// regional due index one hot partition.
///
/// # Errors
///
/// Returns [`WorkError::InvalidShardCount`] for a zero shard count.
pub fn due_shards(shards: u64) -> Result<Vec<u64>, WorkError> {
    if shards == 0 {
        return Err(WorkError::InvalidShardCount);
    }
    Ok((0..shards).collect())
}

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

impl BatchItem {
    /// An item that committed and may be acknowledged.
    #[must_use]
    pub fn succeeded(id: impl Into<String>) -> Self {
        Self::Succeeded(id.into())
    }

    /// An item that must be retried independently of the rest of its batch.
    #[must_use]
    pub fn failed(id: impl Into<String>) -> Self {
        Self::Failed(id.into())
    }
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
