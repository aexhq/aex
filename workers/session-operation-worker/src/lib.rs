//! Fenced, bounded continuation kernel for regional session operations.

pub mod config;

use serde::{Deserialize, Serialize};
use xxhash_rust::xxh3::xxh3_64;

use std::sync::Arc;

use aex_observation_store_dynamodb::{
    SessionObservationDeletion, SessionObservationDeletionError, SessionObservationDeletionRequest,
    SessionObservationDeletionStatus,
};
use aex_operation_domain::{OperationKind, OperationStatus};
use aex_runtime_control::generation::GenerationState;
use aex_session_dynamodb::StoreError;
use aex_session_dynamodb::plan::{Participant, TransactionPlan};
use aex_session_dynamodb::transactions::{OperationStepCancel, operation_cancelled};
use aex_wire::ids::{AgentId, OperationId, PrefixedId, SessionId, WorkspaceId};
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
    /// Immutable operation kind.
    pub kind: OperationKind,
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
            kind: stored.record.kind,
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

/// Immutable authority binding for one session lifecycle step.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LifecycleStep {
    /// Owning workspace.
    pub workspace: WorkspaceId,
    /// Owning session.
    pub session: SessionId,
    /// Public operation identity.
    pub operation: OperationId,
    /// Exact lifecycle kind.
    pub kind: OperationKind,
    /// Operation version the original work item names.
    pub admitted_version: u64,
}

/// Whether the provider/runtime authority is ready for session publication.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LifecycleReadiness {
    /// The exact provider state has committed and the session barrier may run.
    Ready,
    /// An idempotent runtime command was dispatched; redelivery observes it.
    Deferred,
    /// This port does not own the operation kind.
    Unowned,
}

/// Provider/runtime bridge and atomic session-facing settlement.
#[async_trait::async_trait]
pub trait LifecyclePort: Send + Sync + 'static {
    /// Observes or dispatches the exact-generation effect without taking the
    /// regional-work claim.
    async fn prepare(
        &self,
        step: &LifecycleStep,
        now: Timestamp,
    ) -> Result<LifecycleReadiness, StoreError>;

    /// Publishes the already-observed effect and retires the exact claim.
    async fn settle(
        &self,
        step: &LifecycleStep,
        hold: &WorkClaim,
        now: Timestamp,
    ) -> Result<(), StoreError>;
}

#[derive(Debug, Default)]
struct NoLifecycle;

#[async_trait::async_trait]
impl LifecyclePort for NoLifecycle {
    async fn prepare(
        &self,
        _step: &LifecycleStep,
        _now: Timestamp,
    ) -> Result<LifecycleReadiness, StoreError> {
        Ok(LifecycleReadiness::Unowned)
    }

    async fn settle(
        &self,
        _step: &LifecycleStep,
        _hold: &WorkClaim,
        _now: Timestamp,
    ) -> Result<(), StoreError> {
        Err(StoreError::Invalid {
            detail: "no lifecycle settlement authority was composed".to_owned(),
        })
    }
}

/// Production exact-generation lifecycle bridge.
///
/// Provider effects remain owned by `runtime-control-worker`: this adapter
/// sends an idempotent command to its existing queue, then a later redelivery
/// strongly observes `runtime-activity` before committing the session-facing
/// barrier. It never calls the provider control plane itself.
#[derive(Debug, Clone)]
pub struct DynamoLifecyclePort {
    dynamodb: aws_sdk_dynamodb::Client,
    sessions: aex_session_dynamodb::store::SessionReads,
    operations: aex_session_dynamodb::store::OperationStore,
    runtime: aex_runtime_activity_dynamodb::RuntimeActivityDynamoStore,
    sqs: aws_sdk_sqs::Client,
    runtime_queue_url: String,
    tables: aex_session_dynamodb::plan::RegionalTables,
}

impl DynamoLifecyclePort {
    /// Binds the three durable authorities and the runtime-control hint queue.
    #[must_use]
    pub fn new(
        dynamodb: aws_sdk_dynamodb::Client,
        sqs: aws_sdk_sqs::Client,
        tables: aex_session_dynamodb::plan::RegionalTables,
        runtime_queue_url: impl Into<String>,
    ) -> Self {
        Self {
            sessions: aex_session_dynamodb::store::SessionReads::new(
                dynamodb.clone(),
                tables.session_authority.clone(),
            ),
            operations: aex_session_dynamodb::store::OperationStore::new(
                dynamodb.clone(),
                tables.session_authority.clone(),
            ),
            runtime: aex_runtime_activity_dynamodb::RuntimeActivityDynamoStore::new(
                dynamodb.clone(),
                tables.runtime_activity.clone(),
            ),
            dynamodb,
            sqs,
            runtime_queue_url: runtime_queue_url.into(),
            tables,
        }
    }

    async fn session(
        &self,
        step: &LifecycleStep,
    ) -> Result<aex_session_domain::Session, StoreError> {
        match aex_session_dynamodb::store::SessionQueries::load_session(
            &self.sessions,
            step.workspace,
            step.session,
        )
        .await?
        {
            aex_session_dynamodb::store::SessionScoped::Active(session) => Ok(session),
            aex_session_dynamodb::store::SessionScoped::Missing => Err(StoreError::Invalid {
                detail: "the lifecycle work names a missing session".to_owned(),
            }),
            aex_session_dynamodb::store::SessionScoped::Deleted => Err(StoreError::Invalid {
                detail: "the lifecycle work names a session past its deletion fence".to_owned(),
            }),
        }
    }

    async fn runtime_view(
        &self,
        session: &aex_session_domain::Session,
    ) -> Result<aex_runtime_control::store::GenerationView, StoreError> {
        use aex_runtime_control::store::{ReadConsistency, RuntimeActivityStore as _};

        let generation = session.lifecycle.generation;
        let view = self
            .runtime
            .load_generation_view(generation, ReadConsistency::Strong)
            .await
            .map_err(runtime_store_error)?
            .ok_or_else(|| StoreError::Invalid {
                detail: "the session generation has no runtime-activity authority".to_owned(),
            })?;
        if view.session != session.id || view.head.generation != generation {
            return Err(StoreError::Invalid {
                detail: "runtime-activity disagrees with the exact session generation".to_owned(),
            });
        }
        Ok(view)
    }

    async fn dispatch(
        &self,
        command: aex_runtime_control_aws::RuntimeCommand,
    ) -> Result<(), StoreError> {
        let body = serde_json::to_string(&command).map_err(|error| StoreError::Invalid {
            detail: format!("the runtime lifecycle command is not serializable: {error}"),
        })?;
        self.sqs
            .send_message()
            .queue_url(&self.runtime_queue_url)
            .message_body(body)
            .send()
            .await
            .map_err(|error| StoreError::Unavailable {
                detail: format!("the runtime lifecycle hint could not be sent: {error}"),
            })?;
        Ok(())
    }

    async fn partition_page(
        &self,
        table: &str,
        partition: &str,
        prefix: Option<&str>,
        limit: i32,
    ) -> Result<Vec<(String, String)>, StoreError> {
        use aws_sdk_dynamodb::types::AttributeValue;

        let mut query = self
            .dynamodb
            .query()
            .table_name(table)
            .consistent_read(true)
            .key_condition_expression(if prefix.is_some() {
                "pk = :pk AND begins_with(sk, :prefix)"
            } else {
                "pk = :pk"
            })
            .expression_attribute_values(":pk", AttributeValue::S(partition.to_owned()))
            .limit(limit);
        if let Some(prefix) = prefix {
            query =
                query.expression_attribute_values(":prefix", AttributeValue::S(prefix.to_owned()));
        }
        let output = query
            .send()
            .await
            .map_err(|error| StoreError::Unavailable {
                detail: format!("the deletion participant could not query `{table}`: {error}"),
            })?;
        output
            .items
            .unwrap_or_default()
            .into_iter()
            .map(|item| {
                let pk = item.get("pk").and_then(|value| value.as_s().ok()).cloned();
                let sk = item.get("sk").and_then(|value| value.as_s().ok()).cloned();
                pk.zip(sk).ok_or_else(|| StoreError::Invalid {
                    detail: "a deletion query returned a row without string pk/sk".to_owned(),
                })
            })
            .collect()
    }

    async fn delete_rows(
        &self,
        table: &str,
        rows: Vec<(String, String)>,
    ) -> Result<bool, StoreError> {
        use aws_sdk_dynamodb::types::AttributeValue;

        let changed = !rows.is_empty();
        for (pk, sk) in rows {
            self.dynamodb
                .delete_item()
                .table_name(table)
                .key("pk", AttributeValue::S(pk))
                .key("sk", AttributeValue::S(sk))
                .send()
                .await
                .map_err(|error| StoreError::Unavailable {
                    detail: format!(
                        "the deletion participant could not delete from `{table}`: {error}"
                    ),
                })?;
        }
        Ok(changed)
    }

    async fn delete_prefix(
        &self,
        table: &str,
        partition: &str,
        prefix: Option<&str>,
    ) -> Result<bool, StoreError> {
        let rows = self.partition_page(table, partition, prefix, 25).await?;
        self.delete_rows(table, rows).await
    }

    async fn prepare_runtime_delete(
        &self,
        session: &aex_session_domain::Session,
    ) -> Result<LifecycleReadiness, StoreError> {
        let generation_partition = aex_runtime_activity_dynamodb::keys::generation_partition_for_id(
            session.lifecycle.generation,
        );
        let generation_head = self
            .partition_page(
                &self.tables.runtime_activity,
                &generation_partition,
                Some("HEAD"),
                1,
            )
            .await?
            .into_iter()
            .next();
        if let Some(generation_head) = generation_head {
            let view = self.runtime_view(session).await?;
            if !view.head.state.is_terminal() {
                self.dispatch(aex_runtime_control_aws::RuntimeCommand::SessionTerminate {
                    session: session.id,
                    generation: session.lifecycle.generation,
                })
                .await?;
                return Ok(LifecycleReadiness::Deferred);
            }
            if !self
                .partition_page(
                    &self.tables.runtime_activity,
                    &generation_partition,
                    Some("USAGE#"),
                    1,
                )
                .await?
                .is_empty()
            {
                // Usage/accounting authority must consume the outbox before
                // its operational source row can disappear.
                return Ok(LifecycleReadiness::Deferred);
            }
            for prefix in ["INTENT#", "OPERATION#", "PROBE#", "RECEIPT#"] {
                if self
                    .delete_prefix(
                        &self.tables.runtime_activity,
                        &generation_partition,
                        Some(prefix),
                    )
                    .await?
                {
                    return Ok(LifecycleReadiness::Deferred);
                }
            }
            let remaining = self
                .partition_page(
                    &self.tables.runtime_activity,
                    &generation_partition,
                    None,
                    2,
                )
                .await?;
            if remaining.as_slice() != [generation_head.clone()] {
                return Err(StoreError::Invalid {
                    detail: "session deletion found an unowned retained runtime row".to_owned(),
                });
            }
            self.delete_rows(&self.tables.runtime_activity, vec![generation_head])
                .await?;
            return Ok(LifecycleReadiness::Deferred);
        }
        if let Some(current) = self.runtime.load_current(session.id).await? {
            if current.generation != session.lifecycle.generation {
                return Err(StoreError::Invalid {
                    detail: "the deletion fence observed a different current generation".to_owned(),
                });
            }
            let key = aex_runtime_activity_dynamodb::keys::current(session.id);
            self.delete_rows(&self.tables.runtime_activity, vec![(key.pk, key.sk)])
                .await?;
            return Ok(LifecycleReadiness::Deferred);
        }

        Ok(LifecycleReadiness::Ready)
    }

    async fn prepare_session_content_delete(
        &self,
        session: SessionId,
    ) -> Result<LifecycleReadiness, StoreError> {
        let session_partition = aex_session_dynamodb::keys::session_partition(session);
        for prefix in [
            "APPROVAL#",
            "CLONE#",
            "EVT#",
            "MSG#",
            "OP#",
            "OUTBOX#",
            "RUN#",
            "SEALEDMSG#",
        ] {
            if self
                .delete_prefix(
                    &self.tables.session_authority,
                    &session_partition,
                    Some(prefix),
                )
                .await?
            {
                return Ok(LifecycleReadiness::Deferred);
            }
        }

        Ok(LifecycleReadiness::Ready)
    }

    async fn prepare_brain_content_delete(
        &self,
        session: SessionId,
    ) -> Result<LifecycleReadiness, StoreError> {
        let session_partition = aex_session_dynamodb::keys::session_partition(session);
        let agent_indexes = self
            .partition_page(
                &self.tables.session_authority,
                &session_partition,
                Some("AGENT#"),
                1,
            )
            .await?;
        if let Some((_, index_sk)) = agent_indexes.first() {
            let agent = index_sk
                .strip_prefix("AGENT#")
                .and_then(|value| value.parse::<AgentId>().ok())
                .ok_or_else(|| StoreError::Invalid {
                    detail: "a session agent index carries no valid agent identity".to_owned(),
                })?;
            for partition in [
                aex_session_dynamodb::keys::agent_partition(session, agent),
                format!("BRAINAGENT#{session}#{agent}"),
            ] {
                if self
                    .delete_prefix(&self.tables.session_authority, &partition, None)
                    .await?
                {
                    return Ok(LifecycleReadiness::Deferred);
                }
            }
            self.delete_rows(
                &self.tables.session_authority,
                vec![(session_partition.clone(), index_sk.clone())],
            )
            .await?;
            return Ok(LifecycleReadiness::Deferred);
        }
        if self
            .delete_prefix(
                &self.tables.session_authority,
                &session_partition,
                Some(aex_session_dynamodb::keys::BRAIN_PREFIX),
            )
            .await?
        {
            return Ok(LifecycleReadiness::Deferred);
        }
        let remaining = self
            .partition_page(&self.tables.session_authority, &session_partition, None, 2)
            .await?;
        if remaining.as_slice() != [(session_partition, "HEAD".to_owned())] {
            return Err(StoreError::Invalid {
                detail: "session deletion found an unowned retained payload row".to_owned(),
            });
        }
        Ok(LifecycleReadiness::Ready)
    }

    async fn prepare_session_delete(
        &self,
        session: &aex_session_domain::Session,
    ) -> Result<LifecycleReadiness, StoreError> {
        let readiness = self.prepare_runtime_delete(session).await?;
        if readiness != LifecycleReadiness::Ready {
            return Ok(readiness);
        }
        let readiness = self.prepare_session_content_delete(session.id).await?;
        if readiness != LifecycleReadiness::Ready {
            return Ok(readiness);
        }
        let readiness = self.prepare_brain_content_delete(session.id).await?;
        if readiness != LifecycleReadiness::Ready {
            return Ok(readiness);
        }
        Ok(LifecycleReadiness::Ready)
    }
}

#[async_trait::async_trait]
impl LifecyclePort for DynamoLifecyclePort {
    async fn prepare(
        &self,
        step: &LifecycleStep,
        _now: Timestamp,
    ) -> Result<LifecycleReadiness, StoreError> {
        if step.kind == OperationKind::SessionCancel {
            return Ok(LifecycleReadiness::Ready);
        }
        let session = self.session(step).await?;
        if step.kind == OperationKind::SessionDelete {
            return self.prepare_session_delete(&session).await;
        }
        let view = self.runtime_view(&session).await?;
        let ready = match step.kind {
            OperationKind::SessionSuspend => view.head.state == GenerationState::Suspended,
            OperationKind::SessionResume => view.head.state == GenerationState::Running,
            OperationKind::SessionTerminate => view.head.state.is_terminal(),
            OperationKind::SessionCancel | OperationKind::SessionDelete => unreachable!(),
            OperationKind::WorkspaceDelete
            | OperationKind::TelemetryExport
            | OperationKind::ContentGc => return Ok(LifecycleReadiness::Unowned),
        };
        if ready {
            return Ok(LifecycleReadiness::Ready);
        }
        if view.head.state.is_terminal() {
            // Terminate has reached its requested provider effect. Suspend and
            // resume instead need the loss barrier, which fails the operation
            // while terminalizing the public session under the same work fence.
            return Ok(LifecycleReadiness::Ready);
        }
        let command = match step.kind {
            OperationKind::SessionSuspend => {
                aex_runtime_control_aws::RuntimeCommand::SessionSuspend {
                    session: step.session,
                    generation: session.lifecycle.generation,
                }
            }
            OperationKind::SessionResume => {
                aex_runtime_control_aws::RuntimeCommand::SessionResume {
                    session: step.session,
                    generation: session.lifecycle.generation,
                }
            }
            OperationKind::SessionTerminate => {
                aex_runtime_control_aws::RuntimeCommand::SessionTerminate {
                    session: step.session,
                    generation: session.lifecycle.generation,
                }
            }
            _ => unreachable!(),
        };
        self.dispatch(command).await?;
        Ok(LifecycleReadiness::Deferred)
    }

    #[expect(
        clippy::too_many_lines,
        reason = "the exact runtime, deletion evidence, operation and work fences settle in one atomic authority"
    )]
    async fn settle(
        &self,
        step: &LifecycleStep,
        hold: &WorkClaim,
        now: Timestamp,
    ) -> Result<(), StoreError> {
        let session = self.session(step).await?;
        let stored = aex_session_dynamodb::store::OperationAuthority::load(
            &self.operations,
            step.workspace,
            step.operation,
        )
        .await?
        .ok_or_else(|| StoreError::Invalid {
            detail: "the lifecycle operation disappeared before settlement".to_owned(),
        })?;
        if stored.record.kind != step.kind
            || stored.record.session != Some(step.session)
            || stored.version < step.admitted_version
        {
            return Err(StoreError::Invalid {
                detail: "the lifecycle operation changed binding before settlement".to_owned(),
            });
        }
        let runtime = if matches!(
            step.kind,
            OperationKind::SessionCancel | OperationKind::SessionDelete
        ) {
            None
        } else {
            Some(self.runtime_view(&session).await?)
        };
        if let Some(view) = runtime.as_ref() {
            let ready = match step.kind {
                OperationKind::SessionSuspend => {
                    view.head.state == aex_runtime_control::generation::GenerationState::Suspended
                        || view.head.state.is_terminal()
                }
                OperationKind::SessionResume => {
                    view.head.state == aex_runtime_control::generation::GenerationState::Running
                        || view.head.state.is_terminal()
                }
                OperationKind::SessionTerminate => view.head.state.is_terminal(),
                _ => false,
            };
            if !ready {
                return Err(StoreError::Contended);
            }
        }
        let claim = aex_session_app::LifecycleWorkClaim {
            work_id: hold.work_id.clone(),
            fence: hold.fence,
            owner: hold.owner.clone(),
        };
        let planned = if step.kind == OperationKind::SessionDelete {
            aex_session_app::settle_session_delete(
                &session,
                &stored.record,
                aex_operation_domain::operation::OperationVersion(stored.version),
                &claim,
                &aex_session_domain::DeleteEvidence {
                    generation_terminated: true,
                    session_content_removed: true,
                    messages_removed: true,
                    brain_user_content_removed: true,
                    observations_removed: true,
                    export_objects_removed: true,
                    billing_aggregate_retained: true,
                    audit_fact_retained: true,
                },
                now,
            )
        } else if matches!(
            step.kind,
            OperationKind::SessionSuspend | OperationKind::SessionResume
        ) && runtime
            .as_ref()
            .is_some_and(|view| view.head.state.is_terminal())
        {
            let (reason, at) = observed_termination(
                runtime
                    .as_ref()
                    .expect("terminal runtime was checked")
                    .head
                    .state,
                session.lifecycle.expires_at,
                now,
            );
            aex_session_app::settle_lifecycle_loss(
                &session,
                &stored.record,
                aex_operation_domain::operation::OperationVersion(stored.version),
                &claim,
                reason,
                at,
            )
        } else {
            aex_session_app::settle_lifecycle_operation(
                &session,
                &stored.record,
                aex_operation_domain::operation::OperationVersion(stored.version),
                &claim,
                now,
            )
        }
        .map_err(|error| StoreError::Invalid {
            detail: format!("the lifecycle settlement plan was rejected: {error}"),
        })?;
        let binding = aex_session_dynamodb::application_plan::SessionBinding {
            workspace: step.workspace,
            organization: session.organization,
            session: step.session,
        };
        let operations =
            aex_session_dynamodb::app_authority::SessionAuthorityExternal::new(binding);
        let work = aex_work_dynamodb::WorkApplicationCompiler;
        let compilers = aex_session_dynamodb::application_plan::FamilyCompilers::new()
            .with(
                aex_session_app::TableFamily::OperationAuthority,
                &operations,
            )
            .with(aex_session_app::TableFamily::WorkAuthority, &work);
        let committer = aex_session_dynamodb::app_authority::DynamoAuthorityCommitter::new(
            self.dynamodb.clone(),
            self.tables.clone(),
            binding,
            now,
            aex_session_dynamodb::app_authority::ApiHintSink,
        );
        committer
            .commit_replayable_resolving(
                &planned.plan,
                &compilers,
                aex_session_dynamodb::error::Resolution::TargetItem,
            )
            .await
    }
}

fn observed_termination(
    state: aex_runtime_control::generation::GenerationState,
    expires_at: Timestamp,
    observed_at: Timestamp,
) -> (aex_session_domain::TerminationReason, Timestamp) {
    if state == aex_runtime_control::generation::GenerationState::Terminated
        && observed_at >= expires_at
    {
        return (
            aex_session_domain::TerminationReason::LifetimeExpired,
            expires_at,
        );
    }
    (
        aex_session_domain::TerminationReason::RuntimeLost,
        if observed_at > expires_at {
            expires_at
        } else {
            observed_at
        },
    )
}

fn runtime_store_error(error: aex_runtime_control::store::RuntimeStoreError) -> StoreError {
    match error {
        aex_runtime_control::store::RuntimeStoreError::Unavailable { reason } => {
            StoreError::Unavailable { detail: reason }
        }
        aex_runtime_control::store::RuntimeStoreError::Malformed { reason } => {
            StoreError::Invalid { detail: reason }
        }
        aex_runtime_control::store::RuntimeStoreError::NoSuchGeneration { generation } => {
            StoreError::Invalid {
                detail: format!("runtime-activity has no generation {generation}"),
            }
        }
        _ => StoreError::Contended,
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

/// Result of reconciling one authoritative operation-step hint.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReconcileDisposition {
    /// This invocation retired the work row.
    Retired,
    /// A previous invocation already retired it.
    AlreadyRetired,
    /// An idempotent nonterminal participant was durably scheduled or replayed.
    EffectScheduled,
    /// The owning operation is still nonterminal and belongs to an effect lane.
    Deferred,
}

/// Reconciliation of terminal work, accepted cancellations and idempotent
/// participant admission.
///
/// Every path strongly validates the hint against the work and operation
/// authorities first. Lifecycle effects are prepared before the work fence and
/// settled behind it; session observation deletion uses its own operation-bound
/// conditional row, so duplicate invocations replay that participant.
#[derive(Clone)]
pub struct OperationReconciler<W, O, D> {
    work: W,
    operations: O,
    observation_deletions: D,
    lifecycle: Arc<dyn LifecyclePort>,
    owner: String,
    lease_ms: i64,
}

impl<W, O, D> OperationReconciler<W, O, D>
where
    W: WorkPort,
    O: OperationPort,
    D: SessionObservationDeletion,
{
    /// Binds the durable authorities and the claim identity.
    ///
    /// # Errors
    ///
    /// Returns [`ReconcileError::InvalidSettings`] for an empty owner or a
    /// non-positive lease.
    pub fn new(
        work: W,
        operations: O,
        observation_deletions: D,
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
            lifecycle: Arc::new(NoLifecycle),
            observation_deletions,
            owner,
            lease_ms,
        })
    }

    /// Adds the production lifecycle effect and settlement authority.
    #[must_use]
    pub fn with_lifecycle(mut self, lifecycle: impl LifecyclePort) -> Self {
        self.lifecycle = Arc::new(lifecycle);
        self
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

    /// The bound observation deletion participant.
    #[must_use]
    pub const fn observation_deletions(&self) -> &D {
        &self.observation_deletions
    }

    /// Reconciles one hint.
    ///
    /// # Errors
    ///
    /// Returns [`ReconcileError`] when either authority is unavailable or the
    /// work and operation identities do not agree exactly.
    #[expect(
        clippy::too_many_lines,
        reason = "one reconciliation pass binds work, operation, lifecycle effect and retirement without a split"
    )]
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
        let lifecycle_step =
            (!operation.status.is_terminal() && !cancelling).then_some(LifecycleStep {
                workspace: binding.workspace,
                session: binding.session,
                operation: binding.operation,
                kind: operation.kind,
                admitted_version: binding.version,
            });
        if let Some(step) = lifecycle_step.as_ref() {
            match self.lifecycle.prepare(step, now).await? {
                LifecycleReadiness::Ready => {}
                LifecycleReadiness::Deferred | LifecycleReadiness::Unowned => {
                    return Ok(ReconcileDisposition::Deferred);
                }
            }
            if step.kind == OperationKind::SessionDelete {
                self.observation_deletions
                    .request(SessionObservationDeletionRequest {
                        workspace: binding.workspace,
                        session: binding.session,
                        operation: binding.operation,
                        now,
                    })
                    .await?;
                if self
                    .observation_deletions
                    .status(binding.workspace, binding.session, binding.operation)
                    .await?
                    != SessionObservationDeletionStatus::Complete
                {
                    return Ok(ReconcileDisposition::EffectScheduled);
                }
            }
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
        let committed = if let Some(step) = lifecycle_step.as_ref() {
            self.lifecycle.settle(step, &hold, now).await
        } else if cancelling {
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
    #[error("operation reconciler settings are invalid")]
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
    /// Observation deletion admission or replay failed.
    #[error(transparent)]
    ObservationDeletion(#[from] SessionObservationDeletionError),
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

/// The vocabulary this worker's continuations are built on.
///
/// Deliberately a pointer rather than a definition. Everything a fenced step
/// needs — `WorkItem`, `Lease`, `Fence`, `WorkState`, `claim`, `renew`,
/// `StepOutcome`, `complete` — lives in `aex_operation_domain::lease`, and the
/// operation vocabulary it advances lives in `aex_operation_domain::operation`.
///
/// This module used to carry a second, entirely in-memory copy of that model:
/// its own `WorkKind` beside `OperationKind`, its own `WorkRecord` beside
/// `WorkItem`, its own `claim`, `plan_step`, `commit_step` and `record_failure`.
/// It had **no production call site** — `main.rs` never constructed a
/// `WorkRecord` — and its vocabulary predated R-DELETE, naming `SessionDelete`
/// and `SessionForkPage` for what are now trash, purge and clone. Two models of
/// "may this step commit" is one more than a fenced authority can survive, so it
/// is deleted rather than ported forward (D-16).
pub use aex_operation_domain::lease::{
    Fence, Lease, StepOutcome, WorkItem, WorkState, claim, complete, renew,
};

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

/// Invalid work input.
///
/// One arm, because one thing here can be invalid: the due-scan shard count.
/// The four other arms this enum carried belonged to the deleted in-memory
/// kernel, and `ReconcileError` already owns every failure the live
/// reconciliation path can produce.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum WorkError {
    /// Shard count was zero.
    #[error("due shard count must be positive")]
    InvalidShardCount,
}

#[cfg(test)]
mod lifecycle_observation_tests {
    use aex_runtime_control::generation::GenerationState;
    use aex_session_domain::TerminationReason;

    use super::*;

    fn at(millis: i64) -> Timestamp {
        Timestamp::from_unix_millis(millis).expect("fixture instant")
    }

    #[test]
    fn provider_expiry_uses_the_immutable_fence_even_when_observed_late() {
        assert_eq!(
            observed_termination(GenerationState::Terminated, at(28_800), at(40_000)),
            (TerminationReason::LifetimeExpired, at(28_800))
        );
    }

    #[test]
    fn provider_loss_never_publishes_or_bills_past_the_immutable_fence() {
        assert_eq!(
            observed_termination(GenerationState::Lost, at(28_800), at(40_000)),
            (TerminationReason::RuntimeLost, at(28_800))
        );
        assert_eq!(
            observed_termination(GenerationState::Lost, at(28_800), at(20_000)),
            (TerminationReason::RuntimeLost, at(20_000))
        );
    }
}
