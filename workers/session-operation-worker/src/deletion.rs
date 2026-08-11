//! Bounded eight-owner session deletion continuation.
//!
//! Every invocation performs at most one durable progress transition or one
//! guarded page delete before yielding. Provider effects remain in
//! `runtime-control-worker`; observation/export object deletion remains in the
//! observation reconciler. This coordinator records only content-free digests
//! after those authorities prove their work complete.

use std::collections::HashMap;

use aex_observation_store_dynamodb::deletion::{
    SessionObservationDeletion, SessionObservationDeletionError, SessionObservationDeletionRequest,
    SessionObservationDeletionStatus, SessionObservationDeletionStore,
};
use aex_runtime_control::store::{ReadConsistency, RuntimeActivityStore as _};
use aex_session_dynamodb::attr::{ITEM_TYPE, Item, PK, SK};
use aex_session_dynamodb::deletion::{
    DeletionOwner, DeletionTarget, DeletionTombstoneCompiler, EvidenceDigest,
    SessionDeletionEvidence, SessionDeletionProgress, SessionDeletionSnapshot,
    SessionDeletionStore, append_completion_cleanup, compile_delete_receipt,
    compile_guarded_deletes, compile_initialize_progress, compile_record_evidence,
};
use aex_session_dynamodb::error::{Idempotence, Resolution, StoreError, classify};
use aex_session_dynamodb::plan::{RegionalTables, TransactionPlan};
use aex_wire::ids::{AgentId, OperationId, SessionId, WorkspaceId};
use aex_wire::types::Timestamp;
use aws_sdk_dynamodb::types::AttributeValue;

use crate::{LifecycleReadiness, LifecycleStep};

const DELETE_PAGE: i32 = 25;
const VERIFY_PAGE: i32 = 100;

/// Production irreversible-deletion authority.
#[derive(Clone, Debug)]
pub(crate) struct DynamoDeletionCoordinator {
    dynamodb: aws_sdk_dynamodb::Client,
    sqs: aws_sdk_sqs::Client,
    runtime_queue_url: String,
    tables: RegionalTables,
    sessions: SessionDeletionStore,
    operations: aex_session_dynamodb::store::OperationStore,
    runtime: aex_runtime_activity_dynamodb::RuntimeActivityDynamoStore,
    observations: SessionObservationDeletionStore,
}

impl DynamoDeletionCoordinator {
    pub(crate) fn new(
        dynamodb: aws_sdk_dynamodb::Client,
        sqs: aws_sdk_sqs::Client,
        tables: RegionalTables,
        runtime_queue_url: impl Into<String>,
        observation_table: impl Into<String>,
        observation_duty_shards: u8,
    ) -> Result<Self, StoreError> {
        let observations = SessionObservationDeletionStore::new(
            dynamodb.clone(),
            observation_table,
            observation_duty_shards,
        )
        .map_err(observation_error)?;
        Ok(Self {
            sessions: SessionDeletionStore::new(dynamodb.clone(), tables.session_authority.clone()),
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
            observations,
        })
    }

    pub(crate) async fn prepare(
        &self,
        step: &LifecycleStep,
        now: Timestamp,
    ) -> Result<LifecycleReadiness, StoreError> {
        let head = self
            .sessions
            .load_head(step.workspace, step.session)
            .await?
            .ok_or_else(|| StoreError::Invalid {
                detail: "session deletion work has no scrubbed coordination head".to_owned(),
            })?;
        if head.operation != step.operation {
            return Err(StoreError::Invalid {
                detail: "session deletion work disagrees with the scrubbed head operation"
                    .to_owned(),
            });
        }
        let expected = SessionDeletionProgress {
            workspace: head.workspace,
            session: head.session,
            operation: head.operation,
            epoch: head.epoch,
            started_at: head.started_at,
        };
        let Some(snapshot) = self
            .sessions
            .load_snapshot(step.workspace, step.session)
            .await?
        else {
            self.commit(&compile_initialize_progress(
                &self.tables.session_authority,
                expected,
            )?)
            .await?;
            return Ok(LifecycleReadiness::Deferred);
        };
        if snapshot.progress != expected {
            return Err(StoreError::Invalid {
                detail: "session deletion progress disagrees with its scrubbed head".to_owned(),
            });
        }

        if !snapshot
            .evidence
            .contains_key(&DeletionOwner::BillingAggregate)
            || !snapshot
                .evidence
                .contains_key(&DeletionOwner::GenerationTerminated)
        {
            return self.prepare_runtime(&head, &snapshot, now).await;
        }
        if !snapshot.evidence.contains_key(&DeletionOwner::Observations)
            || !snapshot
                .evidence
                .contains_key(&DeletionOwner::ExportObjects)
        {
            return self.prepare_observations(&snapshot, now).await;
        }
        if !snapshot.evidence.contains_key(&DeletionOwner::Messages) {
            return self.prepare_messages(&snapshot, now).await;
        }
        if !snapshot
            .evidence
            .contains_key(&DeletionOwner::BrainUserContent)
        {
            return self.prepare_brain(&snapshot, now).await;
        }
        if !snapshot.evidence.contains_key(&DeletionOwner::AuditFact) {
            return self.prepare_audit(&snapshot, now).await;
        }
        if !snapshot
            .evidence
            .contains_key(&DeletionOwner::SessionContent)
        {
            return self.prepare_session_content(&snapshot, now).await;
        }
        if snapshot.is_complete() {
            let receipts = self
                .sessions
                .list_receipts(step.workspace, step.session, None, 1)
                .await?;
            if !receipts.entries.is_empty() || receipts.next.is_some() {
                return Err(StoreError::Invalid {
                    detail: "complete deletion evidence still has an idempotency locator"
                        .to_owned(),
                });
            }
            return Ok(LifecycleReadiness::Ready);
        }
        Err(StoreError::Invalid {
            detail: "session deletion snapshot is incomplete without a known owner".to_owned(),
        })
    }

    pub(crate) async fn settle(
        &self,
        step: &LifecycleStep,
        hold: &aex_work_dynamodb::WorkClaim,
        now: Timestamp,
    ) -> Result<(), StoreError> {
        let head = self
            .sessions
            .load_head(step.workspace, step.session)
            .await?
            .ok_or_else(|| StoreError::Invalid {
                detail: "session deletion settlement has no scrubbed head".to_owned(),
            })?;
        let snapshot = self
            .sessions
            .load_snapshot(step.workspace, step.session)
            .await?
            .ok_or_else(|| StoreError::Invalid {
                detail: "session deletion settlement has no atomic owner snapshot".to_owned(),
            })?;
        if !snapshot.is_complete()
            || snapshot.progress.operation != step.operation
            || head.operation != step.operation
            || head.epoch != snapshot.progress.epoch
        {
            return Err(StoreError::Contended);
        }
        let receipts = self
            .sessions
            .list_receipts(step.workspace, step.session, None, 1)
            .await?;
        if !receipts.entries.is_empty() || receipts.next.is_some() {
            return Err(StoreError::Contended);
        }
        let stored = aex_session_dynamodb::store::OperationAuthority::load(
            &self.operations,
            step.workspace,
            step.operation,
        )
        .await?
        .ok_or_else(|| StoreError::Invalid {
            detail: "session deletion operation disappeared before its final barrier".to_owned(),
        })?;
        if stored.record.kind != aex_operation_domain::OperationKind::SessionDelete
            || stored.record.session != Some(step.session)
            || stored.version < step.admitted_version
        {
            return Err(StoreError::Invalid {
                detail: "session deletion operation changed binding before its final barrier"
                    .to_owned(),
            });
        }
        let claim = aex_session_app::LifecycleWorkClaim {
            work_id: hold.work_id.clone(),
            fence: hold.fence,
            owner: hold.owner.clone(),
        };
        let planned = aex_session_app::settle_session_delete(
            &head,
            &stored.record,
            aex_operation_domain::OperationVersion(stored.version),
            &claim,
            &snapshot.domain_evidence(),
            now,
        )
        .map_err(|error| StoreError::Invalid {
            detail: format!("the final deletion barrier was rejected: {error}"),
        })?;
        let tombstone = DeletionTombstoneCompiler::new(snapshot.progress, head.revision.0);
        let operations = aex_session_dynamodb::app_authority::SessionAuthorityExternal::new(
            aex_session_dynamodb::application_plan::SessionBinding {
                workspace: head.workspace,
                organization: head.organization,
                session: head.session,
            },
        );
        let work = aex_work_dynamodb::application_plan::WorkApplicationCompiler;
        let families = aex_session_dynamodb::application_plan::FamilyCompilers::new()
            .with(
                aex_session_app::plan::TableFamily::SessionAuthority,
                &tombstone,
            )
            .with(
                aex_session_app::plan::TableFamily::OperationAuthority,
                &operations,
            )
            .with(aex_session_app::plan::TableFamily::WorkAuthority, &work);
        let mut compiled = aex_session_dynamodb::application_plan::compile_application_transaction(
            &self.tables,
            &planned.plan,
            aex_session_dynamodb::application_plan::SessionBinding {
                workspace: head.workspace,
                organization: head.organization,
                session: head.session,
            },
            &families,
        )?;
        append_completion_cleanup(
            &self.tables.session_authority,
            &snapshot,
            &mut compiled.transaction,
        )?;
        self.commit(&compiled.transaction).await
    }

    async fn prepare_runtime(
        &self,
        head: &aex_session_domain::SessionDeletionHead,
        snapshot: &SessionDeletionSnapshot,
        now: Timestamp,
    ) -> Result<LifecycleReadiness, StoreError> {
        let view = self
            .runtime
            .load_generation_view(head.generation, ReadConsistency::Strong)
            .await
            .map_err(runtime_error)?;
        if let Some(view) = view.as_ref() {
            if view.session != head.session || view.head.generation != head.generation {
                return Err(StoreError::Invalid {
                    detail: "runtime deletion observed a cross-session generation".to_owned(),
                });
            }
            if !view.head.state.is_terminal() {
                self.dispatch_terminate(head.session, head.generation)
                    .await?;
                return Ok(LifecycleReadiness::Deferred);
            }
        }
        let usage = self
            .runtime
            .load_usage_outbox(head.generation)
            .await
            .map_err(runtime_error)?;
        if !usage.is_empty() {
            return Ok(LifecycleReadiness::Deferred);
        }
        if !snapshot
            .evidence
            .contains_key(&DeletionOwner::BillingAggregate)
        {
            return self
                .record(snapshot, DeletionOwner::BillingAggregate, now)
                .await;
        }

        let partition =
            aex_runtime_activity_dynamodb::keys::generation_partition_for_id(head.generation);
        let rows = self
            .query(&self.tables.runtime_activity, &partition, None, DELETE_PAGE)
            .await?;
        if !rows.is_empty() {
            let targets = rows
                .iter()
                .map(|item| {
                    self.validated_target(
                        &self.tables.runtime_activity,
                        item,
                        &partition,
                        head.workspace,
                        head.session,
                    )
                })
                .collect::<Result<Vec<_>, _>>()?;
            if targets.iter().any(|target| target.sk.starts_with("USAGE#")) {
                return Err(StoreError::Invalid {
                    detail: "runtime deletion attempted to remove an unhanded usage outbox"
                        .to_owned(),
                });
            }
            self.commit(&compile_guarded_deletes(
                &self.tables.session_authority,
                snapshot.progress,
                &targets,
            )?)
            .await?;
            return Ok(LifecycleReadiness::Deferred);
        }
        if let Some(current) = self.runtime.load_current(head.session).await? {
            if current.generation != head.generation {
                return Err(StoreError::Invalid {
                    detail: "runtime current pointer changed generation behind deletion".to_owned(),
                });
            }
            let key = aex_runtime_activity_dynamodb::keys::current(head.session);
            let target = DeletionTarget {
                table: self.tables.runtime_activity.clone(),
                pk: key.pk,
                sk: key.sk,
                item_type: aex_runtime_activity_dynamodb::codec::CURRENT_GENERATION.to_owned(),
            };
            self.commit(&compile_guarded_deletes(
                &self.tables.session_authority,
                snapshot.progress,
                &[target],
            )?)
            .await?;
            return Ok(LifecycleReadiness::Deferred);
        }
        self.record(snapshot, DeletionOwner::GenerationTerminated, now)
            .await
    }

    async fn prepare_observations(
        &self,
        snapshot: &SessionDeletionSnapshot,
        now: Timestamp,
    ) -> Result<LifecycleReadiness, StoreError> {
        self.observations
            .request(SessionObservationDeletionRequest {
                workspace: snapshot.progress.workspace,
                session: snapshot.progress.session,
                operation: snapshot.progress.operation,
                now,
            })
            .await
            .map_err(observation_error)?;
        if self
            .observations
            .status(
                snapshot.progress.workspace,
                snapshot.progress.session,
                snapshot.progress.operation,
            )
            .await
            .map_err(observation_error)?
            != SessionObservationDeletionStatus::Complete
        {
            return Ok(LifecycleReadiness::Deferred);
        }
        if !snapshot.evidence.contains_key(&DeletionOwner::Observations) {
            return self
                .record(snapshot, DeletionOwner::Observations, now)
                .await;
        }
        self.record(snapshot, DeletionOwner::ExportObjects, now)
            .await
    }

    async fn prepare_messages(
        &self,
        snapshot: &SessionDeletionSnapshot,
        now: Timestamp,
    ) -> Result<LifecycleReadiness, StoreError> {
        let receipts = self
            .sessions
            .list_receipts(
                snapshot.progress.workspace,
                snapshot.progress.session,
                None,
                1,
            )
            .await?;
        if let Some(receipt) = receipts.entries.first() {
            self.commit(&compile_delete_receipt(
                &self.tables.session_authority,
                snapshot.progress,
                receipt,
            )?)
            .await?;
            return Ok(LifecycleReadiness::Deferred);
        }
        for prefix in ["MSG#", "SEALEDMSG#", "RUN#", "EVT#", "OUTBOX#", "APPROVAL#"] {
            if self
                .delete_prefix(
                    &self.tables.session_authority,
                    &aex_session_dynamodb::keys::session_partition(snapshot.progress.session),
                    prefix,
                    snapshot.progress,
                )
                .await?
            {
                return Ok(LifecycleReadiness::Deferred);
            }
        }
        self.record(snapshot, DeletionOwner::Messages, now).await
    }

    async fn prepare_brain(
        &self,
        snapshot: &SessionDeletionSnapshot,
        now: Timestamp,
    ) -> Result<LifecycleReadiness, StoreError> {
        let session_partition =
            aex_session_dynamodb::keys::session_partition(snapshot.progress.session);
        if self
            .delete_prefix(
                &self.tables.session_authority,
                &session_partition,
                aex_session_dynamodb::keys::BRAIN_PREFIX,
                snapshot.progress,
            )
            .await?
        {
            return Ok(LifecycleReadiness::Deferred);
        }
        let indexes = self
            .query(
                &self.tables.session_authority,
                &session_partition,
                Some("AGENT#"),
                1,
            )
            .await?;
        if let Some(index) = indexes.first() {
            let agent =
                string(index, "agentId")?
                    .parse::<AgentId>()
                    .map_err(|_| StoreError::Invalid {
                        detail: "a deletion agent index carries a malformed agentId".to_owned(),
                    })?;
            for partition in [
                aex_session_dynamodb::keys::agent_partition(snapshot.progress.session, agent),
                format!(
                    "{}{}#{}",
                    aex_session_dynamodb::keys::BRAIN_AGENT_PARTITION_PREFIX,
                    snapshot.progress.session,
                    agent
                ),
            ] {
                let rows = self
                    .query(
                        &self.tables.session_authority,
                        &partition,
                        None,
                        DELETE_PAGE,
                    )
                    .await?;
                if !rows.is_empty() {
                    let targets = rows
                        .iter()
                        .map(|item| {
                            self.validated_target(
                                &self.tables.session_authority,
                                item,
                                &partition,
                                snapshot.progress.workspace,
                                snapshot.progress.session,
                            )
                        })
                        .collect::<Result<Vec<_>, _>>()?;
                    self.commit(&compile_guarded_deletes(
                        &self.tables.session_authority,
                        snapshot.progress,
                        &targets,
                    )?)
                    .await?;
                    return Ok(LifecycleReadiness::Deferred);
                }
            }
            let target = self.validated_target(
                &self.tables.session_authority,
                index,
                &session_partition,
                snapshot.progress.workspace,
                snapshot.progress.session,
            )?;
            self.commit(&compile_guarded_deletes(
                &self.tables.session_authority,
                snapshot.progress,
                &[target],
            )?)
            .await?;
            return Ok(LifecycleReadiness::Deferred);
        }
        self.record(snapshot, DeletionOwner::BrainUserContent, now)
            .await
    }

    async fn prepare_session_content(
        &self,
        snapshot: &SessionDeletionSnapshot,
        now: Timestamp,
    ) -> Result<LifecycleReadiness, StoreError> {
        let session_partition =
            aex_session_dynamodb::keys::session_partition(snapshot.progress.session);
        let preparation_edges = self
            .query(
                &self.tables.session_authority,
                &session_partition,
                Some(aex_session_dynamodb::keys::CREATE_PREPARATION_EDGE_SK),
                1,
            )
            .await?;
        if let Some(edge) = preparation_edges.first() {
            let edge_target = self.validated_target(
                &self.tables.session_authority,
                edge,
                &session_partition,
                snapshot.progress.workspace,
                snapshot.progress.session,
            )?;
            if edge_target.item_type != "session_create_preparation_edge"
                || edge_target.sk != aex_session_dynamodb::keys::CREATE_PREPARATION_EDGE_SK
            {
                return Err(StoreError::Invalid {
                    detail: "a create-preparation locator has the wrong authority shape".to_owned(),
                });
            }
            let preparation_partition = string(edge, "preparationPk")?;
            validate_create_preparation_partition(
                preparation_partition,
                snapshot.progress.workspace,
            )?;
            let rows = self
                .query(
                    &self.tables.session_authority,
                    preparation_partition,
                    None,
                    DELETE_PAGE,
                )
                .await?;
            if !rows.is_empty() {
                let targets = rows
                    .iter()
                    .map(|item| {
                        self.validated_target(
                            &self.tables.session_authority,
                            item,
                            preparation_partition,
                            snapshot.progress.workspace,
                            snapshot.progress.session,
                        )
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                if targets.iter().any(|target| {
                    !matches!(
                        target.item_type.as_str(),
                        "session_create_preparation" | "session_create_prepared_file"
                    )
                }) {
                    return Err(StoreError::Invalid {
                        detail: "a create-preparation partition contains an unowned row".to_owned(),
                    });
                }
                self.commit(&compile_guarded_deletes(
                    &self.tables.session_authority,
                    snapshot.progress,
                    &targets,
                )?)
                .await?;
                return Ok(LifecycleReadiness::Deferred);
            }
            self.commit(&compile_guarded_deletes(
                &self.tables.session_authority,
                snapshot.progress,
                &[edge_target],
            )?)
            .await?;
            return Ok(LifecycleReadiness::Deferred);
        }
        let edges = self
            .query(
                &self.tables.session_authority,
                &session_partition,
                Some("OP#"),
                1,
            )
            .await?;
        if let Some(edge) = edges.first() {
            let edge_target = self.validated_target(
                &self.tables.session_authority,
                edge,
                &session_partition,
                snapshot.progress.workspace,
                snapshot.progress.session,
            )?;
            if edge_target.item_type != aex_session_dynamodb::codec::SESSION_OPERATION_EDGE {
                return Err(StoreError::Invalid {
                    detail: "a session operation locator has the wrong authority shape".to_owned(),
                });
            }
            let operation = string(edge, "operationId")?
                .parse::<OperationId>()
                .map_err(|_| StoreError::Invalid {
                    detail: "a deletion operation edge carries a malformed operationId".to_owned(),
                })?;
            let mut targets = Vec::with_capacity(2);
            if operation != snapshot.progress.operation {
                let key = aex_session_dynamodb::keys::operation(operation);
                if let Some(item) = self
                    .get(&self.tables.session_authority, &key.pk, &key.sk)
                    .await?
                {
                    targets.push(self.validated_target(
                        &self.tables.session_authority,
                        &item,
                        &key.pk,
                        snapshot.progress.workspace,
                        snapshot.progress.session,
                    )?);
                }
            }
            targets.push(edge_target);
            self.commit(&compile_guarded_deletes(
                &self.tables.session_authority,
                snapshot.progress,
                &targets,
            )?)
            .await?;
            return Ok(LifecycleReadiness::Deferred);
        }

        let rows = self
            .query(
                &self.tables.session_authority,
                &session_partition,
                None,
                VERIFY_PAGE,
            )
            .await?;
        let mut targets = Vec::new();
        for item in &rows {
            let sk = string(item, SK)?;
            if sk == "HEAD"
                || sk == aex_session_dynamodb::keys::DELETION_PROGRESS_SK
                || sk.starts_with(aex_session_dynamodb::keys::DELETION_EVIDENCE_PREFIX)
            {
                continue;
            }
            if sk.starts_with(aex_session_dynamodb::keys::RECEIPT_DIRECTORY_PREFIX) {
                return Err(StoreError::Invalid {
                    detail: "session-content proof observed an undrained receipt locator"
                        .to_owned(),
                });
            }
            targets.push(self.validated_target(
                &self.tables.session_authority,
                item,
                &session_partition,
                snapshot.progress.workspace,
                snapshot.progress.session,
            )?);
            if targets.len() == aex_session_dynamodb::deletion::DELETION_PAGE_MAX {
                break;
            }
        }
        if !targets.is_empty() {
            self.commit(&compile_guarded_deletes(
                &self.tables.session_authority,
                snapshot.progress,
                &targets,
            )?)
            .await?;
            return Ok(LifecycleReadiness::Deferred);
        }
        if rows.len() == usize::try_from(VERIFY_PAGE).unwrap_or(usize::MAX) {
            return Err(StoreError::Invalid {
                detail: "session-content verification exceeded its bounded authority envelope"
                    .to_owned(),
            });
        }
        self.record(snapshot, DeletionOwner::SessionContent, now)
            .await
    }

    async fn prepare_audit(
        &self,
        snapshot: &SessionDeletionSnapshot,
        now: Timestamp,
    ) -> Result<LifecycleReadiness, StoreError> {
        let stored = aex_session_dynamodb::store::OperationAuthority::load(
            &self.operations,
            snapshot.progress.workspace,
            snapshot.progress.operation,
        )
        .await?
        .ok_or_else(|| StoreError::Invalid {
            detail: "session deletion has no retained direct-replay audit operation".to_owned(),
        })?;
        if stored.record.kind != aex_operation_domain::OperationKind::SessionDelete
            || stored.record.workspace != snapshot.progress.workspace
            || stored.record.session != Some(snapshot.progress.session)
            || stored.record.scope
                != aex_operation_domain::OperationScope::Session(snapshot.progress.session)
        {
            return Err(StoreError::Invalid {
                detail: "the retained deletion audit operation crosses its exact session binding"
                    .to_owned(),
            });
        }
        self.record(snapshot, DeletionOwner::AuditFact, now).await
    }

    async fn record(
        &self,
        snapshot: &SessionDeletionSnapshot,
        owner: DeletionOwner,
        now: Timestamp,
    ) -> Result<LifecycleReadiness, StoreError> {
        let evidence = SessionDeletionEvidence {
            workspace: snapshot.progress.workspace,
            session: snapshot.progress.session,
            operation: snapshot.progress.operation,
            epoch: snapshot.progress.epoch,
            owner,
            proof: evidence_digest(snapshot.progress, owner),
            completed_at: now,
        };
        self.commit(&compile_record_evidence(
            &self.tables.session_authority,
            snapshot.progress,
            evidence,
        )?)
        .await?;
        Ok(LifecycleReadiness::Deferred)
    }

    async fn delete_prefix(
        &self,
        table: &str,
        partition: &str,
        prefix: &str,
        progress: SessionDeletionProgress,
    ) -> Result<bool, StoreError> {
        let rows = self
            .query(table, partition, Some(prefix), DELETE_PAGE)
            .await?;
        if rows.is_empty() {
            return Ok(false);
        }
        let targets = rows
            .iter()
            .map(|item| {
                self.validated_target(table, item, partition, progress.workspace, progress.session)
            })
            .collect::<Result<Vec<_>, _>>()?;
        self.commit(&compile_guarded_deletes(
            &self.tables.session_authority,
            progress,
            &targets,
        )?)
        .await?;
        Ok(true)
    }

    fn validated_target(
        &self,
        table: &str,
        item: &Item,
        expected_partition: &str,
        workspace: WorkspaceId,
        session: SessionId,
    ) -> Result<DeletionTarget, StoreError> {
        let workspace_id = workspace.to_string();
        let session_id = session.to_string();
        let pk = string(item, PK)?;
        let item_type = string(item, ITEM_TYPE)?;
        let allowed_types = if table == self.tables.session_authority {
            aex_session_dynamodb::keys::ITEM_TYPES
        } else if table == self.tables.runtime_activity {
            aex_runtime_activity_dynamodb::keys::ITEM_TYPES
        } else {
            return Err(StoreError::Invalid {
                detail: "a deletion target names an unowned table".to_owned(),
            });
        };
        if pk != expected_partition
            || !allowed_types.contains(&item_type)
            || (table == self.tables.runtime_activity && item_type == "usage_outbox")
            || item
                .get("workspaceId")
                .and_then(|value| value.as_s().ok())
                .is_some_and(|stored| stored != &workspace_id)
            || item
                .get("sessionId")
                .and_then(|value| value.as_s().ok())
                .is_some_and(|stored| stored != &session_id)
        {
            return Err(StoreError::Invalid {
                detail: "a deletion target crosses its asserted table/partition/workspace/session"
                    .to_owned(),
            });
        }
        Ok(DeletionTarget {
            table: table.to_owned(),
            pk: pk.to_owned(),
            sk: string(item, SK)?.to_owned(),
            item_type: item_type.to_owned(),
        })
    }

    async fn query(
        &self,
        table: &str,
        partition: &str,
        prefix: Option<&str>,
        limit: i32,
    ) -> Result<Vec<Item>, StoreError> {
        let mut request = self
            .dynamodb
            .query()
            .table_name(table)
            .consistent_read(true)
            .limit(limit)
            .expression_attribute_values(":pk", AttributeValue::S(partition.to_owned()));
        request = if let Some(prefix) = prefix {
            request
                .key_condition_expression("pk = :pk AND begins_with(sk, :prefix)")
                .expression_attribute_values(":prefix", AttributeValue::S(prefix.to_owned()))
        } else {
            request.key_condition_expression("pk = :pk")
        };
        request
            .send()
            .await
            .map(|output| output.items.unwrap_or_default())
            .map_err(|error| classify(&error, Idempotence::Read))
    }

    async fn get(&self, table: &str, pk: &str, sk: &str) -> Result<Option<Item>, StoreError> {
        self.dynamodb
            .get_item()
            .table_name(table)
            .key(PK, AttributeValue::S(pk.to_owned()))
            .key(SK, AttributeValue::S(sk.to_owned()))
            .consistent_read(true)
            .send()
            .await
            .map(|output| output.item)
            .map_err(|error| classify(&error, Idempotence::Read))
    }

    async fn dispatch_terminate(
        &self,
        session: SessionId,
        generation: aex_wire::ids::GenerationId,
    ) -> Result<(), StoreError> {
        let body =
            serde_json::to_string(&aex_runtime_control_aws::RuntimeCommand::SessionTerminate {
                session,
                generation,
            })
            .map_err(|error| StoreError::Invalid {
                detail: format!("the runtime delete command is not serializable: {error}"),
            })?;
        self.sqs
            .send_message()
            .queue_url(&self.runtime_queue_url)
            .message_body(body)
            .send()
            .await
            .map_err(|error| StoreError::Unavailable {
                detail: format!("the runtime delete hint could not be sent: {error}"),
            })?;
        Ok(())
    }

    async fn commit(&self, plan: &TransactionPlan) -> Result<(), StoreError> {
        let request = plan.compile(&self.dynamodb)?;
        match request.send().await {
            Ok(_) => Ok(()),
            Err(error) => Err(match error.as_service_error() {
                Some(service) => {
                    aex_session_dynamodb::error::decode_cancellation(service, plan.participants())
                }
                None => StoreError::CommitAmbiguous {
                    resolve_by: Resolution::TargetItem,
                },
            }),
        }
    }
}

fn string<'a>(
    item: &'a HashMap<String, AttributeValue>,
    name: &str,
) -> Result<&'a str, StoreError> {
    item.get(name)
        .and_then(|value| value.as_s().ok())
        .map(String::as_str)
        .ok_or_else(|| StoreError::Invalid {
            detail: format!("a deletion target carries no string `{name}`"),
        })
}

fn validate_create_preparation_partition(
    partition: &str,
    workspace: WorkspaceId,
) -> Result<(), StoreError> {
    let prefix = format!("CREATE#{workspace}#");
    let Some(digest) = partition.strip_prefix(&prefix) else {
        return Err(StoreError::Invalid {
            detail: "a create-preparation locator crosses its workspace".to_owned(),
        });
    };
    if digest.len() != 64
        || !digest
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(StoreError::Invalid {
            detail: "a create-preparation locator carries a malformed receipt digest".to_owned(),
        });
    }
    Ok(())
}

fn evidence_digest(progress: SessionDeletionProgress, owner: DeletionOwner) -> EvidenceDigest {
    EvidenceDigest::from_proof(
        format!(
            "aex.session.delete.v1\0{}\0{}\0{}\0{}\0{}",
            progress.workspace,
            progress.session,
            progress.operation,
            progress.epoch.0,
            owner.as_str()
        )
        .as_bytes(),
    )
}

fn observation_error(error: SessionObservationDeletionError) -> StoreError {
    match error {
        SessionObservationDeletionError::Store(error) => error,
        SessionObservationDeletionError::Conflict => StoreError::PreconditionFailed {
            participant: aex_session_dynamodb::plan::Participant::new(
                "observation.session_deletion",
            ),
            observed: None,
        },
        SessionObservationDeletionError::Corrupt { detail } => StoreError::Invalid { detail },
        SessionObservationDeletionError::NotFound => StoreError::Contended,
    }
}

fn runtime_error(error: aex_runtime_control::store::RuntimeStoreError) -> StoreError {
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
        aex_runtime_control::store::RuntimeStoreError::RevisionConflict { .. }
        | aex_runtime_control::store::RuntimeStoreError::ReconcileConflict { .. }
        | aex_runtime_control::store::RuntimeStoreError::IntentOpen { .. } => StoreError::Contended,
    }
}

#[cfg(test)]
mod tests {
    use aex_operation_domain::DeletionEpoch;
    use aex_wire::ids::{OperationId, SessionId, Uuid7, WorkspaceId};

    use super::*;

    fn id<T: aex_wire::ids::PrefixedId>(tag: u8) -> T {
        T::from_uuid7(Uuid7::compose(1, [tag; 10]))
    }

    #[test]
    fn every_owner_proof_is_distinct_and_content_free() {
        let progress = SessionDeletionProgress {
            workspace: id::<WorkspaceId>(1),
            session: id::<SessionId>(2),
            operation: id::<OperationId>(3),
            epoch: DeletionEpoch(4),
            started_at: Timestamp::from_unix_millis(5).expect("time"),
        };
        let proofs = DeletionOwner::ALL
            .into_iter()
            .map(|owner| evidence_digest(progress, owner))
            .collect::<std::collections::BTreeSet<_>>();
        assert_eq!(proofs.len(), DeletionOwner::ALL.len());
    }

    #[test]
    fn runtime_terminal_states_are_the_only_states_that_may_be_purged() {
        use aex_runtime_control::generation::GenerationState;

        for state in [GenerationState::Terminated, GenerationState::Lost] {
            assert!(state.is_terminal());
        }
        for state in [GenerationState::Running, GenerationState::Suspended] {
            assert!(!state.is_terminal());
        }
    }

    #[test]
    fn create_preparation_partitions_are_exact_workspace_scoped_digests() {
        let workspace = id::<WorkspaceId>(1);
        assert!(
            validate_create_preparation_partition(
                &format!("CREATE#{workspace}#{}", "ab".repeat(32)),
                workspace,
            )
            .is_ok()
        );
        assert!(
            validate_create_preparation_partition(
                &format!("CREATE#{}#{}", id::<WorkspaceId>(2), "ab".repeat(32)),
                workspace,
            )
            .is_err()
        );
        assert!(
            validate_create_preparation_partition(
                &format!("CREATE#{workspace}#{}", "AB".repeat(32)),
                workspace,
            )
            .is_err()
        );
    }
}
