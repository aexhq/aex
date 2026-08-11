//! The `session-authority` port implementations.
//!
//! Authority point reads are strongly consistent without exception: an
//! authority that answers from a replica cannot fence anything. The one sparse
//! GSI listing uses the index only as an ordered locator, then strongly hydrates
//! every selected operation before filtering or projecting it. Writes go through
//! [`crate::plan::TransactionPlan`], so no method in this module can issue an
//! unconditional authority write.

use std::future::Future;

use aex_operation_domain::operation::{CancelRejection, OperationKind, OperationStatus, cancel};
use aex_session_domain::{DeletionEpoch, Message, Run, Session, SessionStatus};
use aex_wire::idempotency::IdempotencyKey;
use aex_wire::ids::{AgentId, ApprovalId, OperationId, SessionId, WorkspaceId};
use aex_wire::types::Timestamp;
use async_trait::async_trait;
use aws_sdk_dynamodb::Client;
use futures::{StreamExt as _, stream};
use sha2::{Digest as _, Sha256};

use crate::attr::{CodecError, Item, Row};
use crate::codec;
use crate::error::{
    Idempotence, Resolution, RetryPolicy, StoreError, classify, decode_cancellation,
    decode_cancellation_with_resolution,
};
use crate::keys;
use crate::paging::{CursorError, PageBudget, PagePosition};
use crate::plan::{Participant, RegionalTables, TransactionPlan, key};
use crate::replay::{IdempotencyScope, Receipt, ReceiptStore, key_digest};
use crate::transactions::{
    Foreign, OperationCancelRequest, compile_decision, compile_fanout_page, operation_cancel_owned,
    operation_cancel_requested,
};
use crate::wire_pending::{
    AgentControl, AgentDecisionPlan, Approval, FanoutPagePlan, JournalEntry, StoredOperation,
};

const INDEX_HYDRATION_CONCURRENCY: usize = 16;

/// One page of decoded rows plus its continuation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Page<T> {
    /// The rows.
    pub items: Vec<T>,
    /// Where to resume, when the collection continues.
    pub next: Option<aex_wire::cursor::Cursor>,
}

// TODO(cross-stream): `aex-session-app` publishes no journal store port. Journal reads are
// `aex_session_app::ports::SessionReader`, and journal writes are ordinary writes inside an
// `aex_session_app::plan::SessionTransaction`.
/// The agent journal, consumed by Brain through `aex-brain-store-dynamodb`.
#[async_trait]
pub trait AgentJournalStore: Send + Sync + 'static {
    /// Commits one agent decision.
    ///
    /// # Errors
    ///
    /// [`StoreError`] for a validation, conditional-write, or transport failure.
    async fn commit_decision(
        &self,
        plan: &AgentDecisionPlan,
        next_wake: Option<Foreign>,
    ) -> Result<(), StoreError>;

    /// Commits one bounded fanout page.
    ///
    /// # Errors
    ///
    /// As above, plus [`StoreError::PreconditionFailed`] on the budget
    /// condition when the effective agent limit is exhausted.
    async fn commit_fanout_page(
        &self,
        plan: &FanoutPagePlan,
        wakes: Vec<Foreign>,
    ) -> Result<(), StoreError>;

    /// Reads one agent's control item.
    ///
    /// # Errors
    ///
    /// [`StoreError`] for a transport or decode failure.
    async fn load_control(
        &self,
        workspace: WorkspaceId,
        session: SessionId,
        agent: AgentId,
    ) -> Result<Option<AgentControl>, StoreError>;

    /// Reads a bounded page of journal entries from `from_seq`.
    ///
    /// # Errors
    ///
    /// [`StoreError`] for a transport or decode failure.
    async fn read_journal(
        &self,
        session: SessionId,
        agent: AgentId,
        from_seq: u64,
        budget: PageBudget,
    ) -> Result<Vec<JournalEntry>, StoreError>;
}

/// One bounded page of decoded rows plus the position a continuation resumes
/// from.
///
/// `next` is `Some` exactly when the authority reported more rows behind this
/// page. The position is returned rather than a token because the regional edge
/// owns the one signed cursor codec: a second minting path here would be a
/// second, subtly different continuation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PositionPage<T> {
    /// The rows, in key order.
    pub items: Vec<T>,
    /// Where the next page starts, when there is one.
    pub next: Option<PagePosition>,
    /// Locators the sparse index served whose authority row was already gone —
    /// an eventually consistent index racing a purge. The row is skipped
    /// rather than answered as corruption, and the skip is counted here so the
    /// serving edge can log it; a bare skip would hide an index-integrity
    /// signal.
    pub isolated: u32,
}

/// The complete public session-status filters understood by the collection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionListStatus {
    /// No run is in flight.
    Idle,
    /// A run is executing.
    Running,
    /// Compute is stopping but this generation remains resumable.
    Suspending,
    /// Compute is stopped and this generation remains resumable.
    Suspended,
    /// The retained generation is starting again.
    Resuming,
    /// Compute and live files are being destroyed.
    Terminating,
    /// Compute and live files are permanently gone.
    Terminated,
    /// Irreversible metadata deletion is in progress.
    Deleting,
}

impl SessionListStatus {
    const fn matches(self, status: SessionStatus) -> bool {
        match self {
            Self::Idle => matches!(status, SessionStatus::Idle),
            Self::Running => matches!(status, SessionStatus::Running),
            Self::Suspending => matches!(status, SessionStatus::Suspending),
            Self::Suspended => matches!(status, SessionStatus::Suspended),
            Self::Resuming => matches!(status, SessionStatus::Resuming),
            Self::Terminating => matches!(status, SessionStatus::Terminating),
            Self::Terminated => matches!(status, SessionStatus::Terminated),
            Self::Deleting => matches!(status, SessionStatus::Deleting),
        }
    }
}

/// Exact filters on the customer-visible workspace session collection.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SessionListFilter {
    /// Restrict to one public session status.
    pub status: Option<SessionListStatus>,
}

impl SessionListFilter {
    fn matches(self, session: &Session) -> bool {
        self.status
            .is_none_or(|status| status.matches(session.status))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct SessionLocator {
    session: SessionId,
    created_at: Timestamp,
}

fn session_locator(
    pk: &str,
    sk: &str,
    index_partition: &str,
    index_sort: &str,
    workspace: WorkspaceId,
    snapshot: Timestamp,
) -> Result<SessionLocator, &'static str> {
    let session = pk
        .strip_prefix("SESSION#")
        .and_then(|value| value.parse::<SessionId>().ok())
        .ok_or("the base partition key carries no valid session identity")?;
    if pk != keys::head(session).pk || sk != keys::head(session).sk {
        return Err("the base key is not the located session head");
    }
    if index_partition != keys::workspace_index::session_partition(workspace) {
        return Err("the index partition does not match the asserted workspace");
    }
    let (created_at, indexed_session) = index_sort
        .rsplit_once('#')
        .ok_or("the index sort key is not a creation instant and session identity")?;
    let created_at = Timestamp::parse(created_at)
        .map_err(|_| "the index sort key has an invalid creation instant")?;
    if indexed_session != session.to_string()
        || index_sort != keys::workspace_index::session_sort(created_at, session)
    {
        return Err("the index sort key disagrees with the located session");
    }
    if index_sort > keys::workspace_index::session_snapshot_sort(snapshot).as_str() {
        return Err("the continuation is beyond the pinned session snapshot");
    }
    Ok(SessionLocator {
        session,
        created_at,
    })
}

fn malformed_session(reason: &str) -> StoreError {
    StoreError::Corrupt(CodecError::Malformed {
        item_type: codec::SESSION_HEAD,
        attribute: keys::workspace_index::SK,
        reason: reason.to_owned(),
    })
}

fn projected_session_locator(
    item: &Item,
    workspace: WorkspaceId,
    snapshot: Timestamp,
) -> Result<SessionLocator, StoreError> {
    let row = Row::bind_projected(item, codec::SESSION_HEAD);
    session_locator(
        row.string(crate::attr::PK)?,
        row.string(crate::attr::SK)?,
        row.string(keys::workspace_index::PK)?,
        row.string(keys::workspace_index::SK)?,
        workspace,
        snapshot,
    )
    .map_err(malformed_session)
}

fn validate_session_list_position(
    position: &PagePosition,
    workspace: WorkspaceId,
    snapshot: Timestamp,
) -> Result<(), StoreError> {
    let Some(index_partition) = position.index_pk.as_deref() else {
        return Err(StoreError::Invalid {
            detail: "a session continuation carries no workspace-index partition".to_owned(),
        });
    };
    let Some(index_sort) = position.index_sk.as_deref() else {
        return Err(StoreError::Invalid {
            detail: "a session continuation carries no workspace-index sort key".to_owned(),
        });
    };
    session_locator(
        &position.pk,
        &position.sk,
        index_partition,
        index_sort,
        workspace,
        snapshot,
    )
    .map(|_| ())
    .map_err(|reason| StoreError::Invalid {
        detail: format!("the session continuation is invalid: {reason}"),
    })
}

/// One session collection page and the deletion generation under which it was read.
///
/// The edge binds this epoch into every continuation. Without it, a cursor
/// minted before trash and restore could resume into a logically new live
/// generation of the same session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionPage<T> {
    /// The rows, in key order.
    pub items: Vec<T>,
    /// Where the next page starts, when there is one.
    pub next: Option<PagePosition>,
    /// The stable parent deletion epoch observed before and after the query.
    pub deletion_epoch: DeletionEpoch,
}

/// A read whose child resource is meaningful only while its parent session is live.
///
/// `Missing` deliberately covers both an absent session and a session owned by
/// another workspace. Giving those cases different shapes would turn a point
/// read into a tenant-existence oracle.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionScoped<T> {
    /// The session is absent from the asserted workspace.
    Missing,
    /// The session crossed its deletion fence.
    Deleted,
    /// The session is live and the requested value was read under that fence.
    Active(T),
}

/// The narrow durable-operation authority used by continuation workers and
/// operation point reads.
///
/// A worker that only needs to reload an operation before a fenced step does
/// not acquire any session mutation capability as a side effect.
#[async_trait]
pub trait OperationAuthority: Send + Sync + 'static {
    /// Strongly reads one operation under its asserted tenant.
    ///
    /// # Errors
    ///
    /// [`StoreError`] for a transport or strict decode failure.
    async fn load(
        &self,
        workspace: WorkspaceId,
        operation: OperationId,
    ) -> Result<Option<StoredOperation>, StoreError>;
}

/// Exact filters on the customer-visible workspace operation collection.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct OperationFilter {
    /// Restrict to one owning session.
    pub session: Option<SessionId>,
    /// Restrict to one public operation kind.
    pub kind: Option<OperationKind>,
    /// Restrict to one lifecycle status.
    pub status: Option<OperationStatus>,
}

impl OperationFilter {
    fn matches(self, operation: &aex_operation_domain::Operation) -> bool {
        operation.kind.is_public()
            && self
                .session
                .is_none_or(|session| operation.session == Some(session))
            && self.kind.is_none_or(|kind| operation.kind == kind)
            && self.status.is_none_or(|status| operation.status == status)
    }
}

/// Result of the public cancellation command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OperationCancelOutcome {
    /// The request is durably represented by this operation state.
    Accepted(Box<StoredOperation>),
    /// No public operation exists under the asserted workspace and identity.
    NotFound,
    /// The operation is terminal, committed, or never accepted cancellation.
    NotCancelable,
}

/// The complete operation authority used by the finite regional API.
///
/// This remains separate from [`OperationAuthority`]. A continuation worker
/// that only reloads one operation must not acquire the workspace index or the
/// public cancellation surface as a side effect.
#[async_trait]
pub trait OperationApiStore: Send + Sync + 'static {
    /// Strongly reads one operation under its asserted tenant.
    async fn load(
        &self,
        workspace: WorkspaceId,
        operation: OperationId,
    ) -> Result<Option<StoredOperation>, StoreError>;

    /// Spends one bounded, deterministic physical-row budget over the sparse
    /// workspace index, strongly hydrates it, and applies every filter to the
    /// authority rows.
    ///
    /// The result may be short while carrying `next`: the adapter examines at
    /// most the requested number of index rows per call, continuing across
    /// provider-short slices only while that budget remains. A rare filter can
    /// therefore never turn one HTTP request into an unbounded workspace walk.
    /// Iterating the authenticated continuation remains complete and skips no
    /// examined row.
    async fn page(
        &self,
        workspace: WorkspaceId,
        filter: &OperationFilter,
        budget: PageBudget,
        after: Option<&PagePosition>,
    ) -> Result<PositionPage<StoredOperation>, StoreError>;

    /// Atomically accepts cancellation or returns the exact durable reason it
    /// cannot be accepted.
    async fn request_cancel(
        &self,
        workspace: WorkspaceId,
        operation: OperationId,
        now: Timestamp,
    ) -> Result<OperationCancelOutcome, StoreError>;
}

/// Durable-operation reader and fenced step committer.
#[derive(Debug, Clone)]
pub struct OperationStore {
    client: Client,
    table: String,
}

impl OperationStore {
    /// Binds the reader to the physical `session-authority` table.
    #[must_use]
    pub fn new(client: Client, table: impl Into<String>) -> Self {
        Self {
            client,
            table: table.into(),
        }
    }

    /// The physical authority table.
    #[must_use]
    pub fn table(&self) -> &str {
        &self.table
    }

    /// Commits exactly the operation/work pair for an observed cancellation
    /// step and preserves its named cancellation reasons.
    ///
    /// # Errors
    ///
    /// Every [`StoreError`]. A transport failure is commit-ambiguous and must
    /// be resolved by strongly reading the operation and work target rows; the
    /// caller must never issue the write blindly a second time.
    pub async fn commit_cancelled_step(&self, plan: &TransactionPlan) -> Result<(), StoreError> {
        if plan.participants()
            != [
                crate::plan::Participant::SESSION_OPERATION,
                crate::plan::Participant::WORK_WAKE_DONE,
            ]
        {
            return Err(StoreError::Invalid {
                detail: "a cancelled operation step must update operation then fenced work"
                    .to_owned(),
            });
        }
        let request = plan.compile(&self.client)?;
        match request.send().await {
            Ok(_) => Ok(()),
            Err(error) => {
                if let Some(service) = error.as_service_error() {
                    return Err(decode_cancellation_with_resolution(
                        service,
                        plan.participants(),
                        Resolution::TargetItem,
                    ));
                }
                Err(classify(&error, Idempotence::Write(Resolution::TargetItem)))
            }
        }
    }

    async fn commit_cancel_request(&self, plan: &TransactionPlan) -> Result<(), StoreError> {
        if plan.participants() != [Participant::SESSION_OPERATION] {
            return Err(StoreError::Invalid {
                detail: "an operation cancel request must update exactly the operation row"
                    .to_owned(),
            });
        }
        let request = plan.compile(&self.client)?;
        match request.send().await {
            Ok(_) => Ok(()),
            Err(error) => {
                if let Some(service) = error.as_service_error() {
                    return Err(decode_cancellation_with_resolution(
                        service,
                        plan.participants(),
                        Resolution::TargetItem,
                    ));
                }
                Err(classify(&error, Idempotence::Write(Resolution::TargetItem)))
            }
        }
    }

    /// Strongly hydrates every located operation, isolating locators whose
    /// authority row is already gone.
    ///
    /// The sparse index is eventually consistent and the base row can be
    /// purged between the index read and this hydration, so an absent row is a
    /// legitimate race rather than corruption: it is skipped and counted. A
    /// row that exists but contradicts its locator remains
    /// [`StoreError::Corrupt`] — that is a forged or mis-projected index
    /// entry, not a race.
    async fn hydrate_operations(
        &self,
        workspace: WorkspaceId,
        projected: &[Item],
    ) -> Result<HydratedOperations, StoreError> {
        let operation_ids = projected
            .iter()
            .map(|item| projected_operation_id(item, workspace))
            .collect::<Result<Vec<_>, _>>()?;
        let hydrated = settle_bounded_ordered(operation_ids, |operation| async move {
            let Some(stored) = OperationAuthority::load(self, workspace, operation).await? else {
                return Ok(None);
            };
            if stored.record.id != operation || !stored.record.kind.is_public() {
                return Err(malformed_operation(
                    "operationId",
                    "the hydrated authority row does not match its public index locator",
                ));
            }
            Ok(Some(stored))
        })
        .await?;
        let mut operations = HydratedOperations {
            items: Vec::with_capacity(hydrated.len()),
            isolated: 0,
        };
        for entry in hydrated {
            match entry {
                Some(stored) => operations.items.push(stored),
                None => operations.isolated = operations.isolated.saturating_add(1),
            }
        }
        Ok(operations)
    }
}

/// One hydration pass: the rows that exist, plus the located rows that were
/// already gone.
struct HydratedOperations {
    items: Vec<StoredOperation>,
    isolated: u32,
}

async fn settle_bounded_ordered<I, F, Fut, T, E>(items: I, hydrate: F) -> Result<Vec<T>, E>
where
    I: IntoIterator,
    F: FnMut(I::Item) -> Fut,
    Fut: Future<Output = Result<T, E>>,
{
    stream::iter(items.into_iter().map(hydrate))
        .buffered(INDEX_HYDRATION_CONCURRENCY)
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect()
}

fn malformed_operation(attribute: &'static str, reason: &str) -> StoreError {
    StoreError::Corrupt(CodecError::Malformed {
        item_type: codec::OPERATION,
        attribute,
        reason: reason.to_owned(),
    })
}

fn projected_operation_id(item: &Item, workspace: WorkspaceId) -> Result<OperationId, StoreError> {
    let row = Row::bind_projected(item, codec::OPERATION);
    let base_pk = row.string(crate::attr::PK)?;
    let operation = base_pk
        .strip_prefix("OP#")
        .and_then(|value| value.parse::<OperationId>().ok())
        .ok_or_else(|| {
            malformed_operation(
                "operationId",
                "the KEYS_ONLY projection carries no valid operation base key",
            )
        })?;
    let expected = keys::operation(operation);
    if base_pk != expected.pk || row.string(crate::attr::SK)? != expected.sk {
        return Err(malformed_operation(
            "operationId",
            "the projected base key does not match the operation identity",
        ));
    }
    if row.string(keys::workspace_index::PK)?
        != keys::workspace_index::operation_partition(workspace)
    {
        return Err(malformed_operation(
            keys::workspace_index::PK,
            "the projected index partition does not match the asserted workspace",
        ));
    }
    let index_sort = row.string(keys::workspace_index::SK)?;
    let Some((created_at, indexed_operation)) = index_sort.rsplit_once('#') else {
        return Err(malformed_operation(
            keys::workspace_index::SK,
            "the projected index sort key is not a timestamp and operation identity",
        ));
    };
    let created_at = Timestamp::parse(created_at).map_err(|_| {
        malformed_operation(
            keys::workspace_index::SK,
            "the projected index sort key has an invalid creation timestamp",
        )
    })?;
    if indexed_operation != operation.to_string()
        || index_sort != keys::workspace_index::operation_sort(created_at, operation)
    {
        return Err(malformed_operation(
            keys::workspace_index::SK,
            "the projected index key does not match the operation identity and creation time",
        ));
    }
    Ok(operation)
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum CancelObservation {
    Accepted(Box<StoredOperation>),
    Retry,
    NotFound,
    NotCancelable,
}

fn observe_cancel(stored: Option<StoredOperation>) -> Result<CancelObservation, StoreError> {
    let Some(stored) = stored else {
        return Ok(CancelObservation::NotFound);
    };
    if !stored.record.kind.is_public() {
        return Ok(CancelObservation::NotFound);
    }
    if !operation_cancel_owned(stored.record.kind) {
        return Ok(CancelObservation::NotCancelable);
    }
    if stored.record.status == OperationStatus::Cancelled
        || (stored.record.status == OperationStatus::Running && stored.record.cancel_requested)
    {
        return Ok(CancelObservation::Accepted(Box::new(stored)));
    }
    if stored.record.status == OperationStatus::Queued && stored.record.cancel_requested {
        return Err(malformed_operation(
            "cancelRequested",
            "a queued cancel request must have terminalized the operation",
        ));
    }
    if stored.record.cancelable() {
        Ok(CancelObservation::Retry)
    } else {
        Ok(CancelObservation::NotCancelable)
    }
}

fn cancel_token(workspace: WorkspaceId, operation: OperationId, version: u64) -> String {
    let mut digest = Sha256::new();
    digest.update(workspace.to_string().as_bytes());
    digest.update([0]);
    digest.update(operation.to_string().as_bytes());
    digest.update([0]);
    digest.update(version.to_be_bytes());
    format!("oc:{}", hex::encode(&digest.finalize()[..16]))
}

#[async_trait]
trait OperationCancelIo: Send + Sync {
    async fn load_cancel_target(
        &self,
        workspace: WorkspaceId,
        operation: OperationId,
    ) -> Result<Option<StoredOperation>, StoreError>;

    async fn write_cancel_request(
        &self,
        request: &OperationCancelRequest,
    ) -> Result<(), StoreError>;
}

#[async_trait]
impl OperationCancelIo for OperationStore {
    async fn load_cancel_target(
        &self,
        workspace: WorkspaceId,
        operation: OperationId,
    ) -> Result<Option<StoredOperation>, StoreError> {
        OperationAuthority::load(self, workspace, operation).await
    }

    async fn write_cancel_request(
        &self,
        request: &OperationCancelRequest,
    ) -> Result<(), StoreError> {
        let mut plan = TransactionPlan::new(cancel_token(
            request.workspace,
            request.operation,
            request.version,
        ));
        plan.update(
            Participant::SESSION_OPERATION,
            operation_cancel_requested(&self.table, request)?,
        )?;
        self.commit_cancel_request(&plan).await
    }
}

async fn request_cancel_with<I: OperationCancelIo>(
    io: &I,
    workspace: WorkspaceId,
    operation: OperationId,
    now: Timestamp,
) -> Result<OperationCancelOutcome, StoreError> {
    for _ in 0..RetryPolicy::PINNED.attempts {
        let current = io.load_cancel_target(workspace, operation).await?;
        match observe_cancel(current.clone())? {
            CancelObservation::Accepted(stored) => {
                return Ok(OperationCancelOutcome::Accepted(stored));
            }
            CancelObservation::NotFound => return Ok(OperationCancelOutcome::NotFound),
            CancelObservation::NotCancelable => {
                return Ok(OperationCancelOutcome::NotCancelable);
            }
            CancelObservation::Retry => {}
        }
        let current = current.expect("a retry observation always carries an operation");
        let commit = match cancel(&current.record, now) {
            Ok(commit) => commit,
            Err(CancelRejection::NotCancelable { .. } | CancelRejection::AlreadyTerminal(_)) => {
                return Ok(OperationCancelOutcome::NotCancelable);
            }
        };
        let next_version = current
            .version
            .checked_add(1)
            .ok_or_else(|| StoreError::Invalid {
                detail: "an operation version cannot advance past u64::MAX".to_owned(),
            })?;
        let request = OperationCancelRequest {
            workspace,
            operation,
            kind: current.record.kind,
            status: current.record.status,
            version: current.version,
            now,
        };
        match io.write_cancel_request(&request).await {
            Ok(()) => {
                return Ok(OperationCancelOutcome::Accepted(Box::new(
                    StoredOperation {
                        record: commit.operation,
                        version: next_version,
                    },
                )));
            }
            Err(
                StoreError::PreconditionFailed { .. }
                | StoreError::CommitAmbiguous {
                    resolve_by: Resolution::TargetItem,
                },
            ) => match observe_cancel(io.load_cancel_target(workspace, operation).await?)? {
                CancelObservation::Accepted(stored) => {
                    return Ok(OperationCancelOutcome::Accepted(stored));
                }
                CancelObservation::NotFound => return Ok(OperationCancelOutcome::NotFound),
                CancelObservation::NotCancelable => {
                    return Ok(OperationCancelOutcome::NotCancelable);
                }
                CancelObservation::Retry => {}
            },
            Err(error) if error.retryable() => {}
            Err(error) => return Err(error),
        }
    }
    Err(StoreError::Contended)
}

#[async_trait]
impl OperationAuthority for OperationStore {
    async fn load(
        &self,
        workspace: WorkspaceId,
        operation: OperationId,
    ) -> Result<Option<StoredOperation>, StoreError> {
        let operation_key = keys::operation(operation);
        let output = self
            .client
            .get_item()
            .table_name(&self.table)
            .set_key(Some(key(&operation_key.pk, &operation_key.sk)))
            .consistent_read(true)
            .send()
            .await
            .map_err(|error| classify(&error, Idempotence::Read))?;
        match output.item {
            None => Ok(None),
            Some(item) => Ok(Some(codec::decode_operation(&item, workspace)?)),
        }
    }
}

#[async_trait]
impl OperationApiStore for OperationStore {
    async fn load(
        &self,
        workspace: WorkspaceId,
        operation: OperationId,
    ) -> Result<Option<StoredOperation>, StoreError> {
        OperationAuthority::load(self, workspace, operation).await
    }

    async fn page(
        &self,
        workspace: WorkspaceId,
        filter: &OperationFilter,
        budget: PageBudget,
        after: Option<&PagePosition>,
    ) -> Result<PositionPage<StoredOperation>, StoreError> {
        let mut remaining = budget.items();
        let mut start = after.cloned();
        let mut items = Vec::new();
        let mut isolated = 0_u32;
        let next = loop {
            let output = self
                .client
                .query()
                .table_name(&self.table)
                .index_name(keys::workspace_index::NAME)
                .key_condition_expression("#workspace = :workspace")
                .expression_attribute_names("#workspace", keys::workspace_index::PK)
                .expression_attribute_values(
                    ":workspace",
                    crate::attr::s(keys::workspace_index::operation_partition(workspace)),
                )
                .consistent_read(false)
                .scan_index_forward(true)
                .limit(
                    i32::try_from(remaining).map_err(|error| StoreError::Invalid {
                        detail: format!("the operation page budget does not fit DynamoDB: {error}"),
                    })?,
                )
                .set_exclusive_start_key(start.as_ref().map(|position| {
                    position.to_exclusive_start(
                        Some(keys::workspace_index::PK),
                        Some(keys::workspace_index::SK),
                    )
                }))
                .send()
                .await
                .map_err(|error| classify(&error, Idempotence::Read))?;

            let projected = output.items.unwrap_or_default();
            let physical = u32::try_from(projected.len()).map_err(|error| StoreError::Invalid {
                detail: format!("the operation index page length does not fit u32: {error}"),
            })?;
            if physical > remaining {
                return Err(malformed_operation(
                    "operationId",
                    "DynamoDB returned more index rows than the explicit read budget",
                ));
            }
            let continuation = output
                .last_evaluated_key
                .as_ref()
                .map(|last| {
                    PagePosition::from_last_evaluated(
                        last,
                        Some(keys::workspace_index::PK),
                        Some(keys::workspace_index::SK),
                    )
                })
                .transpose()
                .map_err(|error| cursor_error(&error))?;

            if projected.is_empty() {
                break continuation;
            }
            remaining -= physical;
            let hydrated = self.hydrate_operations(workspace, &projected).await?;
            isolated = isolated.saturating_add(hydrated.isolated);
            items.extend(
                hydrated
                    .items
                    .into_iter()
                    .filter(|stored| filter.matches(&stored.record)),
            );
            if remaining == 0 || continuation.is_none() {
                break continuation;
            }
            start = continuation;
        };
        Ok(PositionPage {
            items,
            next,
            isolated,
        })
    }

    async fn request_cancel(
        &self,
        workspace: WorkspaceId,
        operation: OperationId,
        now: Timestamp,
    ) -> Result<OperationCancelOutcome, StoreError> {
        request_cancel_with(self, workspace, operation, now).await
    }
}

#[cfg(test)]
mod operation_store_tests {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    use aex_operation_domain::operation::{
        Operation, OperationKind, OperationScope, OperationStatus, cancel,
    };
    use aex_wire::idempotency::IntentDigest;
    use aex_wire::ids::{OperationId, Uuid7, WorkspaceId};

    use super::{
        INDEX_HYDRATION_CONCURRENCY, OperationCancelIo, OperationCancelOutcome,
        OperationCancelRequest, Resolution, StoreError, StoredOperation, Timestamp, async_trait,
        request_cancel_with, settle_bounded_ordered,
    };

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_maximum_operation_page_hydrates_in_order_with_at_most_sixteen_reads() {
        let current = Arc::new(AtomicUsize::new(0));
        let peak = Arc::new(AtomicUsize::new(0));
        let page = (0..100).collect::<Vec<_>>();
        let hydrated = settle_bounded_ordered(page.clone(), |value| {
            let current = Arc::clone(&current);
            let peak = Arc::clone(&peak);
            async move {
                let in_flight = current.fetch_add(1, Ordering::SeqCst) + 1;
                peak.fetch_max(in_flight, Ordering::SeqCst);
                tokio::time::sleep(Duration::from_millis(2)).await;
                current.fetch_sub(1, Ordering::SeqCst);
                Ok::<_, ()>(value)
            }
        })
        .await
        .expect("the full page hydrates");

        assert_eq!(hydrated, page);
        assert_eq!(peak.load(Ordering::SeqCst), INDEX_HYDRATION_CONCURRENCY);
        assert_eq!(current.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn bounded_hydration_settles_every_started_read_before_returning_an_error() {
        let completed = Arc::new(AtomicUsize::new(0));
        let result = settle_bounded_ordered(0..32, |value| {
            let completed = Arc::clone(&completed);
            async move {
                tokio::task::yield_now().await;
                completed.fetch_add(1, Ordering::SeqCst);
                if value == 3 {
                    Err("injected read failure")
                } else {
                    Ok(value)
                }
            }
        })
        .await;

        assert_eq!(result, Err("injected read failure"));
        assert_eq!(completed.load(Ordering::SeqCst), 32);
    }

    fn sample<I: aex_wire::ids::PrefixedId>(seed: u8) -> I {
        I::from_uuid7(Uuid7::compose(1_754_051_696_789, [seed; 10]))
    }

    fn now() -> Timestamp {
        Timestamp::parse("2026-08-02T12:00:00.000Z").expect("a fixture instant")
    }

    fn stored(status: OperationStatus) -> StoredOperation {
        let workspace = sample::<WorkspaceId>(1);
        StoredOperation {
            record: Operation {
                id: sample::<OperationId>(2),
                workspace,
                session: None,
                kind: OperationKind::WorkspaceDelete,
                status,
                intent: IntentDigest::from_bytes([3; 32]),
                scope: OperationScope::Workspace(workspace),
                progress: None,
                cursor: None,
                cancel_requested: false,
                result: None,
                error: None,
                created_at: now(),
                started_at: (status == OperationStatus::Running).then_some(now()),
                updated_at: now(),
                committed_at: None,
                terminal_at: None,
            },
            version: 7,
        }
    }

    struct AmbiguousLanding {
        state: Mutex<StoredOperation>,
        reads: AtomicUsize,
        writes: AtomicUsize,
    }

    impl AmbiguousLanding {
        fn new(status: OperationStatus) -> Self {
            Self {
                state: Mutex::new(stored(status)),
                reads: AtomicUsize::new(0),
                writes: AtomicUsize::new(0),
            }
        }
    }

    #[async_trait]
    impl OperationCancelIo for AmbiguousLanding {
        async fn load_cancel_target(
            &self,
            workspace: WorkspaceId,
            operation: OperationId,
        ) -> Result<Option<StoredOperation>, StoreError> {
            self.reads.fetch_add(1, Ordering::Relaxed);
            let stored = self.state.lock().expect("an uncontended fixture").clone();
            Ok(
                (stored.record.workspace == workspace && stored.record.id == operation)
                    .then_some(stored),
            )
        }

        async fn write_cancel_request(
            &self,
            request: &OperationCancelRequest,
        ) -> Result<(), StoreError> {
            self.writes.fetch_add(1, Ordering::Relaxed);
            let mut stored = self.state.lock().expect("an uncontended fixture");
            assert_eq!(stored.record.workspace, request.workspace);
            assert_eq!(stored.record.id, request.operation);
            assert_eq!(stored.record.status, request.status);
            assert_eq!(stored.version, request.version);
            stored.record = cancel(&stored.record, request.now)
                .expect("the injected write lands")
                .operation;
            stored.version = stored.version.checked_add(1).expect("a fixture version");
            Err(StoreError::CommitAmbiguous {
                resolve_by: Resolution::TargetItem,
            })
        }
    }

    async fn assert_generic_cancel_is_refused(status: OperationStatus) {
        let io = AmbiguousLanding::new(status);
        let initial = io.state.lock().expect("an uncontended fixture").clone();
        let outcome = request_cancel_with(&io, initial.record.workspace, initial.record.id, now())
            .await
            .expect("the strong read classifies the operation");
        assert!(matches!(outcome, OperationCancelOutcome::NotCancelable));
        assert_eq!(io.reads.load(Ordering::Relaxed), 1);
        assert_eq!(io.writes.load(Ordering::Relaxed), 0);

        let replay = request_cancel_with(&io, initial.record.workspace, initial.record.id, now())
            .await
            .expect("the durable state answers the same refusal");
        assert!(matches!(replay, OperationCancelOutcome::NotCancelable));
        assert_eq!(io.reads.load(Ordering::Relaxed), 2);
        assert_eq!(io.writes.load(Ordering::Relaxed), 0);
    }

    #[tokio::test]
    async fn a_queued_lifecycle_operation_refuses_generic_cancellation() {
        assert_generic_cancel_is_refused(OperationStatus::Queued).await;
    }

    #[tokio::test]
    async fn a_running_lifecycle_operation_refuses_generic_cancellation() {
        assert_generic_cancel_is_refused(OperationStatus::Running).await;
    }
}

/// The read-only `session-authority` surface the finite regional API composes.
///
/// There is no method here that commits anything, so a deployable that holds
/// only this port cannot write the session table however it is wired.
#[async_trait]
pub trait SessionQueries: Send + Sync + 'static {
    /// Lists canonical session heads in the workspace-wide creation order.
    ///
    /// `snapshot` is the first-page instant the regional edge binds into its
    /// signed cursor. The index query never crosses it. `after` is the exact
    /// four-part provider position from the prior page; this authority does
    /// not mint a second cursor vocabulary.
    async fn page_sessions(
        &self,
        workspace: WorkspaceId,
        filter: &SessionListFilter,
        snapshot: Timestamp,
        budget: PageBudget,
        after: Option<&PagePosition>,
    ) -> Result<PositionPage<Session>, StoreError>;

    /// Reads one complete canonical session authority document.
    async fn load_session(
        &self,
        workspace: WorkspaceId,
        session: SessionId,
    ) -> Result<SessionScoped<Session>, StoreError>;

    /// Lists canonical messages under one live session fence.
    ///
    /// A continuation caller passes the epoch from the strong parent read it
    /// performed before decoding the cursor. `None` makes this adapter perform
    /// that initial read itself. Either path always performs the final read.
    async fn page_messages(
        &self,
        workspace: WorkspaceId,
        session: SessionId,
        expected_deletion_epoch: Option<DeletionEpoch>,
        budget: PageBudget,
        after: Option<&PagePosition>,
    ) -> Result<SessionScoped<SessionPage<Message>>, StoreError>;

    /// Reads one canonical run under one live session fence.
    async fn load_run(
        &self,
        workspace: WorkspaceId,
        session: SessionId,
        run: aex_internal_contracts::RunId,
    ) -> Result<SessionScoped<Option<Run>>, StoreError>;

    /// Lists canonical runs under one live session fence.
    ///
    /// `expected_deletion_epoch` has the same predecoded-cursor semantics as
    /// [`SessionQueries::page_messages`].
    async fn page_runs(
        &self,
        workspace: WorkspaceId,
        session: SessionId,
        expected_deletion_epoch: Option<DeletionEpoch>,
        budget: PageBudget,
        after: Option<&PagePosition>,
    ) -> Result<SessionScoped<SessionPage<Run>>, StoreError>;

    /// Reads one approval.
    ///
    /// # Errors
    ///
    /// As for every strongly consistent authority read.
    async fn load_approval(
        &self,
        workspace: WorkspaceId,
        session: SessionId,
        approval: ApprovalId,
    ) -> Result<SessionScoped<Option<Approval>>, StoreError>;

    /// Lists one session's approvals, oldest first, from a continuation.
    ///
    /// `expected_deletion_epoch` has the same predecoded-cursor semantics as
    /// [`SessionQueries::page_messages`].
    ///
    /// # Errors
    ///
    /// As for every strongly consistent authority read.
    async fn page_approvals(
        &self,
        workspace: WorkspaceId,
        session: SessionId,
        expected_deletion_epoch: Option<DeletionEpoch>,
        budget: PageBudget,
        after: Option<&PagePosition>,
    ) -> Result<SessionScoped<SessionPage<Approval>>, StoreError>;
}

fn cursor_error(error: &CursorError) -> StoreError {
    StoreError::Invalid {
        detail: error.to_string(),
    }
}

/// The read-only adapter.
///
/// It holds a client and a table name and nothing else — in particular no
/// cursor key, because it mints no token, and no transaction compiler, because
/// it commits nothing.
#[derive(Debug, Clone)]
pub struct SessionReads {
    client: Client,
    table: String,
}

impl SessionReads {
    /// Binds a reader to a client and the physical `session-authority` name.
    #[must_use]
    pub fn new(client: Client, table: impl Into<String>) -> Self {
        Self {
            client,
            table: table.into(),
        }
    }

    /// The physical table name.
    #[must_use]
    pub fn table(&self) -> &str {
        &self.table
    }

    async fn get(&self, pk: &str, sk: &str) -> Result<Option<Item>, StoreError> {
        let output = self
            .client
            .get_item()
            .table_name(&self.table)
            .set_key(Some(key(pk, sk)))
            .consistent_read(true)
            .send()
            .await
            .map_err(|error| classify(&error, Idempotence::Read))?;
        Ok(output.item)
    }

    async fn get_scoped_item(
        &self,
        workspace: WorkspaceId,
        session: SessionId,
        child: &keys::Key,
    ) -> Result<(SessionScoped<Session>, Option<Item>), StoreError> {
        let head = keys::head(session);
        // Two plain strongly consistent point reads issued together, not a
        // `TransactGetItems`: a serializable read pair mutually cancels with
        // this session's own write transactions, so a busy session's runs and
        // approvals became unreadable exactly when they were being written.
        // Atomicity across the pair bought nothing the head decode does not
        // already enforce - a head that stopped admitting work answers
        // `Deleted`/`Missing` whatever the child read saw, so a torn pair
        // fails closed rather than publishing anything.
        let (head_item, child_item) =
            futures::try_join!(self.get(&head.pk, &head.sk), self.get(&child.pk, &child.sk),)?;
        let Some(item) = head_item else {
            return Ok((SessionScoped::Missing, child_item));
        };
        match crate::authority_codec::is_session_tombstone(&item, workspace, session) {
            Ok(true) => return Ok((SessionScoped::Deleted, child_item)),
            Ok(false) => {}
            Err(CodecError::WrongTenant { .. }) => {
                return Ok((SessionScoped::Missing, child_item));
            }
            Err(error) => return Err(StoreError::Corrupt(error)),
        }
        match crate::authority_codec::is_session_deletion_head(&item, workspace, session) {
            Ok(true) => return Ok((SessionScoped::Deleted, child_item)),
            Ok(false) => {}
            Err(CodecError::WrongTenant { .. }) => {
                return Ok((SessionScoped::Missing, child_item));
            }
            Err(error) => return Err(StoreError::Corrupt(error)),
        }
        let decoded = match crate::authority_codec::decode_session(&item, workspace) {
            Ok(decoded) => decoded,
            Err(CodecError::WrongTenant { .. }) => {
                return Ok((SessionScoped::Missing, child_item));
            }
            Err(error) => return Err(StoreError::Corrupt(error)),
        };
        if decoded.deletion.state.admits_work() {
            Ok((SessionScoped::Active(decoded), child_item))
        } else {
            Ok((SessionScoped::Deleted, child_item))
        }
    }

    async fn scoped_session(
        &self,
        workspace: WorkspaceId,
        session: SessionId,
    ) -> Result<SessionScoped<Session>, StoreError> {
        let key = keys::head(session);
        let Some(item) = self.get(&key.pk, &key.sk).await? else {
            return Ok(SessionScoped::Missing);
        };
        match crate::authority_codec::is_session_tombstone(&item, workspace, session) {
            Ok(true) => return Ok(SessionScoped::Deleted),
            Ok(false) => {}
            Err(CodecError::WrongTenant { .. }) => return Ok(SessionScoped::Missing),
            Err(error) => return Err(StoreError::Corrupt(error)),
        }
        match crate::authority_codec::is_session_deletion_head(&item, workspace, session) {
            Ok(true) => return Ok(SessionScoped::Deleted),
            Ok(false) => {}
            Err(CodecError::WrongTenant { .. }) => return Ok(SessionScoped::Missing),
            Err(error) => return Err(StoreError::Corrupt(error)),
        }
        let decoded = match crate::authority_codec::decode_session(&item, workspace) {
            Ok(decoded) => decoded,
            Err(CodecError::WrongTenant { .. }) => return Ok(SessionScoped::Missing),
            Err(error) => return Err(StoreError::Corrupt(error)),
        };
        if decoded.deletion.state.admits_work() {
            Ok(SessionScoped::Active(decoded))
        } else {
            Ok(SessionScoped::Deleted)
        }
    }

    /// Strongly hydrates every session locator in provider order.
    ///
    /// A missing row, or a retained head that has completed purge and removed
    /// its sparse index attributes, is an ordinary eventual-index race. It is
    /// skipped and counted. Every other contradiction is corruption.
    async fn hydrate_sessions(
        &self,
        workspace: WorkspaceId,
        snapshot: Timestamp,
        projected: &[Item],
    ) -> Result<HydratedSessions, StoreError> {
        let locators = projected
            .iter()
            .map(|item| projected_session_locator(item, workspace, snapshot))
            .collect::<Result<Vec<_>, _>>()?;
        let hydrated = settle_bounded_ordered(locators, |locator| async move {
            let head = keys::head(locator.session);
            let Some(item) = self.get(&head.pk, &head.sk).await? else {
                return Ok(None);
            };
            if crate::authority_codec::is_session_tombstone(&item, workspace, locator.session)? {
                return Ok(None);
            }
            if crate::authority_codec::is_session_deletion_head(&item, workspace, locator.session)?
            {
                return Ok(None);
            }
            let session = crate::authority_codec::decode_session(&item, workspace)?;
            if session.id != locator.session || session.created_at != locator.created_at {
                return Err(malformed_session(
                    "the strongly hydrated head disagrees with its index locator",
                ));
            }
            Ok(Some(session))
        })
        .await?;

        let mut sessions = HydratedSessions {
            items: Vec::with_capacity(hydrated.len()),
            isolated: 0,
        };
        for entry in hydrated {
            match entry {
                Some(session) => sessions.items.push(session),
                None => sessions.isolated = sessions.isolated.saturating_add(1),
            }
        }
        Ok(sessions)
    }

    async fn query_session_range(
        &self,
        session: SessionId,
        prefix: &'static str,
        budget: PageBudget,
        after: Option<&PagePosition>,
    ) -> Result<(Vec<Item>, Option<PagePosition>), StoreError> {
        if let Some(position) = after {
            validate_session_position(position, session, prefix)?;
        }
        let output = self
            .client
            .query()
            .table_name(&self.table)
            .key_condition_expression("#pk = :pk AND begins_with(#sk, :prefix)")
            .expression_attribute_names("#pk", crate::attr::PK)
            .expression_attribute_names("#sk", crate::attr::SK)
            .expression_attribute_values(":pk", crate::attr::s(keys::session_partition(session)))
            .expression_attribute_values(":prefix", crate::attr::s(prefix))
            .limit(budget.limit())
            .consistent_read(true)
            .scan_index_forward(true)
            .set_exclusive_start_key(after.map(|position| position.to_exclusive_start(None, None)))
            .send()
            .await
            .map_err(|error| classify(&error, Idempotence::Read))?;
        let next = output
            .last_evaluated_key
            .as_ref()
            .map(|last| PagePosition::from_last_evaluated(last, None, None))
            .transpose()
            .map_err(|error| cursor_error(&error))?;
        if let Some(position) = next.as_ref() {
            validate_session_position(position, session, prefix)?;
        }
        Ok((output.items.unwrap_or_default(), next))
    }

    async fn finish_scoped<T>(
        &self,
        workspace: WorkspaceId,
        session: SessionId,
        expected_deletion_epoch: DeletionEpoch,
        value: T,
    ) -> Result<SessionScoped<T>, StoreError> {
        match self.scoped_session(workspace, session).await? {
            SessionScoped::Missing | SessionScoped::Deleted => Ok(SessionScoped::Deleted),
            SessionScoped::Active(after) if after.deletion.epoch == expected_deletion_epoch => {
                Ok(SessionScoped::Active(value))
            }
            // A trash followed by a restore can be live again at the second
            // read. The advanced epoch proves the child read crossed the
            // deletion fence, so it must not be published. These read routes
            // do not declare a retryable snapshot-conflict response and this
            // adapter never loops internally, so fail closed as a nonretryable
            // internal refusal rather than misclassifying it as contention.
            SessionScoped::Active(_) => Err(StoreError::Invalid {
                detail: "the session deletion epoch changed across a collection read".to_owned(),
            }),
        }
    }
}

struct HydratedSessions {
    items: Vec<Session>,
    isolated: u32,
}

fn validate_session_position(
    position: &PagePosition,
    session: SessionId,
    prefix: &'static str,
) -> Result<(), StoreError> {
    if position.pk != keys::session_partition(session)
        || !position.sk.starts_with(prefix)
        || position.index_pk.is_some()
        || position.index_sk.is_some()
    {
        return Err(StoreError::Invalid {
            detail: "a continuation does not belong to this session collection".to_owned(),
        });
    }
    Ok(())
}

#[cfg(test)]
mod canonical_read_tests {
    use proptest::prelude::*;

    use super::validate_session_position;
    use crate::paging::PagePosition;

    #[test]
    fn a_session_collection_position_is_exactly_base_table_scoped() {
        let session = aex_session_domain::testing::session_fixture().id;
        let valid = PagePosition {
            pk: crate::keys::session_partition(session),
            sk: format!(
                "{}{}",
                crate::keys::run_prefix(),
                aex_session_domain::testing::run_id(9)
            ),
            index_pk: None,
            index_sk: None,
        };
        assert!(validate_session_position(&valid, session, crate::keys::run_prefix()).is_ok());

        let mut foreign = valid.clone();
        foreign.pk = crate::keys::session_partition(aex_session_domain::testing::id(99));
        assert!(validate_session_position(&foreign, session, crate::keys::run_prefix()).is_err());

        let mut wrong_range = valid.clone();
        wrong_range.sk = format!(
            "{}{}",
            crate::keys::message_prefix(),
            aex_session_domain::testing::id::<aex_wire::ids::MessageId>(8)
        );
        assert!(
            validate_session_position(&wrong_range, session, crate::keys::run_prefix()).is_err()
        );
    }

    #[test]
    fn a_delete_restore_cycle_invalidates_an_in_flight_child_read() {
        let before = aex_session_domain::testing::session_fixture();
        let mut restored = before.clone();
        restored.deletion.epoch = restored.deletion.epoch.next().next();
        assert_ne!(before.deletion.epoch, restored.deletion.epoch);
    }

    proptest! {
        #[test]
        fn an_index_position_is_never_accepted_by_a_base_collection(
            index_partition in ".{0,64}",
            index_sort in ".{0,64}",
        ) {
            let session = aex_session_domain::testing::session_fixture().id;
            let position = PagePosition {
                pk: crate::keys::session_partition(session),
                sk: format!(
                    "{}{}",
                    crate::keys::run_prefix(),
                    aex_session_domain::testing::run_id(9)
                ),
                index_pk: Some(index_partition),
                index_sk: Some(index_sort),
            };
            prop_assert!(
                validate_session_position(&position, session, crate::keys::run_prefix()).is_err()
            );
        }
    }
}

fn approval_for_session(
    item: &Item,
    workspace: WorkspaceId,
    session: SessionId,
) -> Result<Approval, StoreError> {
    let approval = codec::decode_approval(item, workspace)?;
    if approval.binding.session != session {
        return Err(StoreError::Corrupt(CodecError::Malformed {
            item_type: codec::APPROVAL,
            attribute: "sessionId",
            reason: "does not match the session partition queried".to_owned(),
        }));
    }
    Ok(approval)
}

#[async_trait]
impl SessionQueries for SessionReads {
    async fn page_sessions(
        &self,
        workspace: WorkspaceId,
        filter: &SessionListFilter,
        snapshot: Timestamp,
        budget: PageBudget,
        after: Option<&PagePosition>,
    ) -> Result<PositionPage<Session>, StoreError> {
        if let Some(position) = after {
            validate_session_list_position(position, workspace, snapshot)?;
        }
        let mut remaining = budget.items();
        let mut start = after.cloned();
        let mut items = Vec::new();
        let mut isolated = 0_u32;
        let next = loop {
            let output = self
                .client
                .query()
                .table_name(&self.table)
                .index_name(keys::workspace_index::NAME)
                .key_condition_expression("#workspace = :workspace AND #order <= :snapshot")
                .expression_attribute_names("#workspace", keys::workspace_index::PK)
                .expression_attribute_names("#order", keys::workspace_index::SK)
                .expression_attribute_values(
                    ":workspace",
                    crate::attr::s(keys::workspace_index::session_partition(workspace)),
                )
                .expression_attribute_values(
                    ":snapshot",
                    crate::attr::s(keys::workspace_index::session_snapshot_sort(snapshot)),
                )
                .consistent_read(false)
                .scan_index_forward(true)
                .limit(
                    i32::try_from(remaining).map_err(|error| StoreError::Invalid {
                        detail: format!("the session page budget does not fit DynamoDB: {error}"),
                    })?,
                )
                .set_exclusive_start_key(start.as_ref().map(|position| {
                    position.to_exclusive_start(
                        Some(keys::workspace_index::PK),
                        Some(keys::workspace_index::SK),
                    )
                }))
                .send()
                .await
                .map_err(|error| classify(&error, Idempotence::Read))?;

            let projected = output.items.unwrap_or_default();
            let physical = u32::try_from(projected.len()).map_err(|error| StoreError::Invalid {
                detail: format!("the session index page length does not fit u32: {error}"),
            })?;
            if physical > remaining {
                return Err(malformed_session(
                    "DynamoDB returned more session locators than the explicit read budget",
                ));
            }
            let continuation = output
                .last_evaluated_key
                .as_ref()
                .map(|last| {
                    PagePosition::from_last_evaluated(
                        last,
                        Some(keys::workspace_index::PK),
                        Some(keys::workspace_index::SK),
                    )
                })
                .transpose()
                .map_err(|error| cursor_error(&error))?;
            if let Some(position) = continuation.as_ref() {
                validate_session_list_position(position, workspace, snapshot)?;
            }

            if projected.is_empty() {
                break continuation;
            }
            remaining -= physical;
            let hydrated = self
                .hydrate_sessions(workspace, snapshot, &projected)
                .await?;
            isolated = isolated.saturating_add(hydrated.isolated);
            items.extend(
                hydrated
                    .items
                    .into_iter()
                    .filter(|session| filter.matches(session)),
            );
            if remaining == 0 || continuation.is_none() {
                break continuation;
            }
            start = continuation;
        };

        Ok(PositionPage {
            items,
            next,
            isolated,
        })
    }

    async fn load_session(
        &self,
        workspace: WorkspaceId,
        session: SessionId,
    ) -> Result<SessionScoped<Session>, StoreError> {
        self.scoped_session(workspace, session).await
    }

    async fn page_messages(
        &self,
        workspace: WorkspaceId,
        session: SessionId,
        expected_deletion_epoch: Option<DeletionEpoch>,
        budget: PageBudget,
        after: Option<&PagePosition>,
    ) -> Result<SessionScoped<SessionPage<Message>>, StoreError> {
        let deletion_epoch = match expected_deletion_epoch {
            Some(epoch) => epoch,
            None => match self.scoped_session(workspace, session).await? {
                SessionScoped::Active(session) => session.deletion.epoch,
                SessionScoped::Missing => return Ok(SessionScoped::Missing),
                SessionScoped::Deleted => return Ok(SessionScoped::Deleted),
            },
        };
        let (rows, next) = self
            .query_session_range(session, keys::sealed_message_prefix(), budget, after)
            .await?;
        let items = rows
            .iter()
            .map(|item| crate::authority_codec::decode_sealed_message(item, workspace))
            .collect::<Result<Vec<_>, _>>()?;
        self.finish_scoped(
            workspace,
            session,
            deletion_epoch,
            SessionPage {
                items,
                next,
                deletion_epoch,
            },
        )
        .await
    }

    async fn load_run(
        &self,
        workspace: WorkspaceId,
        session: SessionId,
        run: aex_internal_contracts::RunId,
    ) -> Result<SessionScoped<Option<Run>>, StoreError> {
        let child = keys::run(session, run);
        let (scope, item) = self.get_scoped_item(workspace, session, &child).await?;
        match scope {
            SessionScoped::Active(_) => {}
            SessionScoped::Missing => return Ok(SessionScoped::Missing),
            SessionScoped::Deleted => return Ok(SessionScoped::Deleted),
        }
        let value = item
            .as_ref()
            .map(|item| crate::authority_codec::decode_domain_run(item, workspace))
            .transpose()?;
        Ok(SessionScoped::Active(value))
    }

    async fn page_runs(
        &self,
        workspace: WorkspaceId,
        session: SessionId,
        expected_deletion_epoch: Option<DeletionEpoch>,
        budget: PageBudget,
        after: Option<&PagePosition>,
    ) -> Result<SessionScoped<SessionPage<Run>>, StoreError> {
        let deletion_epoch = match expected_deletion_epoch {
            Some(epoch) => epoch,
            None => match self.scoped_session(workspace, session).await? {
                SessionScoped::Active(session) => session.deletion.epoch,
                SessionScoped::Missing => return Ok(SessionScoped::Missing),
                SessionScoped::Deleted => return Ok(SessionScoped::Deleted),
            },
        };
        let (rows, next) = self
            .query_session_range(session, keys::run_prefix(), budget, after)
            .await?;
        let items = rows
            .iter()
            .map(|item| crate::authority_codec::decode_domain_run(item, workspace))
            .collect::<Result<Vec<_>, _>>()?;
        self.finish_scoped(
            workspace,
            session,
            deletion_epoch,
            SessionPage {
                items,
                next,
                deletion_epoch,
            },
        )
        .await
    }

    async fn load_approval(
        &self,
        workspace: WorkspaceId,
        session: SessionId,
        approval: ApprovalId,
    ) -> Result<SessionScoped<Option<Approval>>, StoreError> {
        let child = keys::approval(session, approval);
        let (scope, item) = self.get_scoped_item(workspace, session, &child).await?;
        match scope {
            SessionScoped::Active(_) => {}
            SessionScoped::Missing => return Ok(SessionScoped::Missing),
            SessionScoped::Deleted => return Ok(SessionScoped::Deleted),
        }
        let value = item
            .as_ref()
            .map(|item| approval_for_session(item, workspace, session))
            .transpose()?;
        Ok(SessionScoped::Active(value))
    }

    async fn page_approvals(
        &self,
        workspace: WorkspaceId,
        session: SessionId,
        expected_deletion_epoch: Option<DeletionEpoch>,
        budget: PageBudget,
        after: Option<&PagePosition>,
    ) -> Result<SessionScoped<SessionPage<Approval>>, StoreError> {
        let deletion_epoch = match expected_deletion_epoch {
            Some(epoch) => epoch,
            None => match self.scoped_session(workspace, session).await? {
                SessionScoped::Active(session) => session.deletion.epoch,
                SessionScoped::Missing => return Ok(SessionScoped::Missing),
                SessionScoped::Deleted => return Ok(SessionScoped::Deleted),
            },
        };
        // One `begins_with` range inside the session partition. No index and no
        // filter expression: an approval that belongs to another session is in
        // another partition, so it is unreachable rather than filtered out.
        let (rows, next) = self
            .query_session_range(session, keys::approval_prefix(), budget, after)
            .await?;

        let mut items = Vec::new();
        for item in rows {
            items.push(approval_for_session(&item, workspace, session)?);
        }
        self.finish_scoped(
            workspace,
            session,
            deletion_epoch,
            SessionPage {
                items,
                next,
                deletion_epoch,
            },
        )
        .await
    }
}

/// The adapter.
#[derive(Debug, Clone)]
pub struct SessionStore {
    client: Client,
    tables: RegionalTables,
}

impl SessionStore {
    /// Binds a store to a client and the physical regional table names.
    #[must_use]
    pub fn new(client: Client, tables: RegionalTables) -> Self {
        Self { client, tables }
    }

    /// The physical `session-authority` table name.
    #[must_use]
    pub fn table(&self) -> &str {
        &self.tables.session_authority
    }

    /// The regional table names this store compiles against.
    #[must_use]
    pub const fn tables(&self) -> &RegionalTables {
        &self.tables
    }

    async fn get(&self, pk: &str, sk: &str) -> Result<Option<Item>, StoreError> {
        let output = self
            .client
            .get_item()
            .table_name(self.table())
            .set_key(Some(key(pk, sk)))
            .consistent_read(true)
            .send()
            .await
            .map_err(|error| classify(&error, Idempotence::Read))?;
        Ok(output.item)
    }

    /// Commits a compiled plan and decodes a cancellation into a
    /// participant-named error.
    ///
    /// # Errors
    ///
    /// Every [`StoreError`]. A transport failure on a transaction is always
    /// [`StoreError::CommitAmbiguous`], never a silent retry.
    pub async fn commit(&self, plan: &TransactionPlan) -> Result<(), StoreError> {
        let request = plan.compile(&self.client)?;
        match request.send().await {
            Ok(_) => Ok(()),
            Err(error) => {
                if let Some(service) = error.as_service_error() {
                    return Err(decode_cancellation(service, plan.participants()));
                }
                Err(classify(
                    &error,
                    Idempotence::Write(Resolution::IdempotencyReceipt),
                ))
            }
        }
    }
}

#[async_trait]
impl AgentJournalStore for SessionStore {
    async fn commit_decision(
        &self,
        plan: &AgentDecisionPlan,
        next_wake: Option<Foreign>,
    ) -> Result<(), StoreError> {
        let compiled = compile_decision(&self.tables, plan, next_wake)?;
        self.commit(&compiled).await
    }

    async fn commit_fanout_page(
        &self,
        plan: &FanoutPagePlan,
        wakes: Vec<Foreign>,
    ) -> Result<(), StoreError> {
        let compiled = compile_fanout_page(&self.tables, plan, wakes)?;
        self.commit(&compiled).await
    }

    async fn load_control(
        &self,
        workspace: WorkspaceId,
        session: SessionId,
        agent: AgentId,
    ) -> Result<Option<AgentControl>, StoreError> {
        let key = keys::agent_control(session, agent);
        match self.get(&key.pk, &key.sk).await? {
            None => Ok(None),
            Some(item) => Ok(Some(codec::decode_control(&item, workspace)?)),
        }
    }

    async fn read_journal(
        &self,
        session: SessionId,
        agent: AgentId,
        from_seq: u64,
        budget: PageBudget,
    ) -> Result<Vec<JournalEntry>, StoreError> {
        let partition = keys::agent_partition(session, agent);
        let output = self
            .client
            .query()
            .table_name(self.table())
            .key_condition_expression("pk = :pk AND sk >= :from")
            .expression_attribute_values(":pk", crate::attr::s(partition))
            .expression_attribute_values(":from", crate::attr::s(keys::journal_sort_key(from_seq)))
            .limit(budget.limit())
            .consistent_read(true)
            .send()
            .await
            .map_err(|error| classify(&error, Idempotence::Read))?;
        let mut entries = Vec::new();
        for item in output.items.unwrap_or_default() {
            entries.push(codec::decode_journal(&item)?);
        }
        Ok(entries)
    }
}

#[async_trait]
impl ReceiptStore for SessionStore {
    async fn read_receipt(
        &self,
        workspace: WorkspaceId,
        scope: &IdempotencyScope<'_>,
        key: &IdempotencyKey,
        now: Timestamp,
    ) -> Result<Option<Receipt>, StoreError> {
        let rendered = scope.render();
        let receipt_key = keys::receipt(workspace, &rendered, &key_digest(key))?;
        let Some(item) = self.get(&receipt_key.pk, &receipt_key.sk).await? else {
            return Ok(None);
        };
        let receipt = codec::decode_receipt(&item)?;
        // Expiry is checked here rather than trusted to TTL: AWS reclaims a
        // TTL'd row within 48 hours, so a reader that trusted it would replay
        // an expired receipt for up to two days.
        if codec::receipt_is_live(&receipt, now) {
            Ok(Some(receipt))
        } else {
            Ok(None)
        }
    }
}
