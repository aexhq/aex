//! The `runtime-activity` port implementation.
//!
//! The reaper's scan reads the slim due-index projection, so a sweep for
//! generations that need evaluating never pulls a whole head back — and the
//! projection has no attribute that could carry customer content in the first
//! place, which is what makes this table's `NEW_IMAGE` stream safe.

use aex_hands_protocol::rpc::Fence;
use aex_runtime_control::generation::{GenerationHead, GenerationState, Revision, TransportMode};
use aex_runtime_control::lifecycle::{IntentRecord, IntentState, MicrovmId, RECONCILE_ATTEMPTS};
use aex_runtime_control::store::{
    GenerationCommit, GenerationPlan, GenerationPointer, GenerationView,
    IdleProbe as CanonicalIdleProbe, LifecycleIntentCommit, LifecycleIntentPlan,
    LifecycleReceipt as CanonicalReceipt, LifecycleReceiptPlan, LifecycleReconcilePlan,
    LifecycleRequestPlan, OpenCountRepairPlan, OperationAdmissionPlan, OperationSettlementPlan,
    PageBudget as CanonicalPageBudget, RuntimeActivityStore, RuntimeDuePage, RuntimeShard,
    RuntimeStoreError, StoreFuture, UsageOutboxEntry,
};
use aex_session_dynamodb::attr::{Item, ItemBuilder, PK, SK, n, s, stamp};
use aex_session_dynamodb::error::{Idempotence, Resolution, StoreError, classify};
use aex_session_dynamodb::paging::PageBudget;
use aex_session_dynamodb::plan::{Participant, key};
use aex_usage_domain::fact::FactDraft;
use aex_usage_domain::meter::Category;
use aex_wire::ids::{GenerationId, PrefixedId, SessionId, WorkspaceId};
use aex_wire::types::Timestamp;
use aws_sdk_dynamodb::Client;
use aws_sdk_dynamodb::types::builders::{PutBuilder, UpdateBuilder};
use aws_sdk_dynamodb::types::{
    Delete, Put, ReturnValuesOnConditionCheckFailure, TransactWriteItem, Update,
};
use std::str::FromStr as _;

use crate::codec::{
    self, CurrentGeneration, GenerationRow, IdleProbe, LifecycleIntent, LifecycleReceipt,
};
use crate::{expressions, keys};

const HANDS_OPERATION_ADMISSION: &str = "hands_operation_admission";

/// One row of the slim evaluation projection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DueGeneration {
    /// Which session.
    pub session: SessionId,
    /// Which generation.
    pub generation: GenerationId,
    /// Its state.
    pub state: GenerationState,
    /// Its fence.
    pub fence: Fence,
    /// When quiescence started.
    pub idle_since: Option<Timestamp>,
    /// When the provider's lifetime ends.
    pub provider_lifetime_expires_at: Option<Timestamp>,
    /// When a paid keepalive lease lapses.
    pub keepalive_lease_until: Option<Timestamp>,
}

/// The adapter.
#[derive(Debug, Clone)]
pub struct RuntimeActivityDynamoStore {
    client: Client,
    table: String,
}

impl RuntimeActivityDynamoStore {
    /// Binds a store to a client and a physical table name.
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

    async fn conditional_put(
        &self,
        builder: PutBuilder,
        participant: Participant,
    ) -> Result<(), StoreError> {
        let built = builder.build().map_err(|error| StoreError::Invalid {
            detail: error.to_string(),
        })?;
        let outcome = self
            .client
            .put_item()
            .table_name(&self.table)
            .set_item(Some(built.item().clone()))
            .set_condition_expression(built.condition_expression().map(str::to_owned))
            .return_values_on_condition_check_failure(ReturnValuesOnConditionCheckFailure::AllOld)
            .send()
            .await;
        match outcome {
            Ok(_) => Ok(()),
            Err(error) => {
                if let Some(
                    aws_sdk_dynamodb::operation::put_item::PutItemError::ConditionalCheckFailedException(failed),
                ) = error.as_service_error()
                {
                    return Err(StoreError::PreconditionFailed {
                        participant,
                        observed: failed.item.clone().map(Box::new),
                    });
                }
                Err(classify(&error, Idempotence::Write(Resolution::TargetItem)))
            }
        }
    }

    async fn conditional_update(
        &self,
        builder: UpdateBuilder,
        participant: Participant,
    ) -> Result<(), StoreError> {
        let built = builder.build().map_err(|error| StoreError::Invalid {
            detail: error.to_string(),
        })?;
        let outcome = self
            .client
            .update_item()
            .table_name(&self.table)
            .set_key(Some(built.key().clone()))
            .set_condition_expression(built.condition_expression().map(str::to_owned))
            .update_expression(built.update_expression())
            .set_expression_attribute_names(built.expression_attribute_names().cloned())
            .set_expression_attribute_values(built.expression_attribute_values().cloned())
            .return_values_on_condition_check_failure(ReturnValuesOnConditionCheckFailure::AllOld)
            .send()
            .await;
        match outcome {
            Ok(_) => Ok(()),
            Err(error) => {
                if let Some(
                    aws_sdk_dynamodb::operation::update_item::UpdateItemError::ConditionalCheckFailedException(failed),
                ) = error.as_service_error()
                {
                    return Err(StoreError::PreconditionFailed {
                        participant,
                        observed: failed.item.clone().map(Box::new),
                    });
                }
                Err(classify(&error, Idempotence::Write(Resolution::TargetItem)))
            }
        }
    }
}

impl RuntimeActivityDynamoStore {
    async fn load_canonical_row(
        &self,
        generation: GenerationId,
    ) -> Result<Option<GenerationRow>, RuntimeStoreError> {
        let target = keys::head_for_generation(generation);
        self.get(&target.pk, &target.sk)
            .await
            .map_err(|error| runtime_error(error, None))?
            .map(|item| {
                codec::decode_generation_view(&item).map_err(|error| RuntimeStoreError::Malformed {
                    reason: error.to_string(),
                })
            })
            .transpose()
    }

    async fn operation_is_admitted(
        &self,
        generation: GenerationId,
        operation: aex_hands_protocol::rpc::HandsOperationId,
    ) -> Result<bool, RuntimeStoreError> {
        let target = keys::operation_admission(generation, operation);
        let Some(item) = self
            .get(&target.pk, &target.sk)
            .await
            .map_err(|error| runtime_error(error, None))?
        else {
            return Ok(false);
        };
        let row = aex_session_dynamodb::attr::Row::bind(&item, HANDS_OPERATION_ADMISSION)
            .map_err(|error| malformed(&error.to_string()))?;
        if row
            .string("generationId")
            .map_err(|error| malformed(&error.to_string()))?
            != generation.to_string()
            || row
                .string("operationId")
                .map_err(|error| malformed(&error.to_string()))?
                != operation.0.to_string()
        {
            return Err(malformed(
                "an operation admission marker disagrees with its key",
            ));
        }
        Ok(true)
    }

    async fn transact(
        &self,
        items: Vec<TransactWriteItem>,
        token: String,
    ) -> Result<(), RuntimeStoreError> {
        self.client
            .transact_write_items()
            .set_transact_items(Some(items))
            .client_request_token(token)
            .send()
            .await
            .map_err(|error| {
                runtime_error(
                    classify(&error, Idempotence::Write(Resolution::TargetItem)),
                    None,
                )
            })?;
        Ok(())
    }
}

impl RuntimeActivityStore for RuntimeActivityDynamoStore {
    fn load_current_generation(
        &self,
        session: SessionId,
    ) -> StoreFuture<'_, Option<GenerationPointer>> {
        Box::pin(async move {
            self.load_current(session)
                .await
                .map(|pointer| {
                    pointer.map(|pointer| GenerationPointer {
                        session: pointer.session,
                        generation: pointer.generation,
                        fence: pointer.fence,
                        revision: pointer.revision,
                    })
                })
                .map_err(|error| runtime_error(error, None))
        })
    }

    fn load_generation_view(
        &self,
        generation: GenerationId,
    ) -> StoreFuture<'_, Option<GenerationView>> {
        Box::pin(async move {
            self.load_canonical_row(generation)
                .await
                .map(|row| row.map(generation_view))
        })
    }

    #[allow(
        clippy::too_many_lines,
        reason = "the head and current-pointer mutations are one atomic fence transition; keeping the expressions together makes their equality auditable"
    )]
    fn commit_generation<'a>(
        &'a self,
        plan: &'a GenerationPlan,
    ) -> StoreFuture<'a, GenerationCommit> {
        Box::pin(async move {
            let before = self.load_canonical_row(plan.generation).await?.ok_or(
                RuntimeStoreError::NoSuchGeneration {
                    generation: plan.generation,
                },
            )?;
            let target = keys::head_for_generation(plan.generation);
            let next_revision = plan.expected_revision.next();
            let evaluable = keys::is_evaluable(plan.next_state);
            let mut update = String::from(
                "SET #state = :nextState, fence = :nextFence, revision = :nextRevision, updatedAt = :at",
            );
            let mut remove = Vec::new();
            let mut builder = Update::builder()
                .table_name(&self.table)
                .set_key(Some(key(&target.pk, &target.sk)))
                .condition_expression(
                    "generationId = :generation AND #state = :expectedState AND fence = :expectedFence AND revision = :expectedRevision",
                )
                .expression_attribute_names("#state", "state")
                .expression_attribute_values(":generation", s(plan.generation.to_string()))
                .expression_attribute_values(":expectedState", s(keys::state_str(plan.expected_state)))
                .expression_attribute_values(":expectedFence", n(plan.expected_fence.0))
                .expression_attribute_values(":expectedRevision", n(plan.expected_revision.value()))
                .expression_attribute_values(":nextState", s(keys::state_str(plan.next_state)))
                .expression_attribute_values(":nextFence", n(plan.next_fence.0))
                .expression_attribute_values(":nextRevision", n(next_revision.value()))
                .expression_attribute_values(":at", stamp(plan.at));
            if let Some(microvm) = &plan.microvm {
                update.push_str(", providerVmId = :microvm");
                builder = builder.expression_attribute_values(":microvm", s(microvm.0.clone()));
            } else {
                remove.push("providerVmId");
            }
            if let Some(mode) = plan.transport_mode {
                update.push_str(", transportMode = :transportMode");
                builder = builder
                    .expression_attribute_values(":transportMode", s(transport_mode_str(mode)));
            } else {
                remove.push("transportMode");
            }
            if let Some(accounting) = plan.accounting {
                update.push_str(
                    ", accountedFrom = :accountedFrom, snapshotOrdinal = :snapshotOrdinal",
                );
                builder = builder
                    .expression_attribute_values(":accountedFrom", stamp(accounting.accounted_from))
                    .expression_attribute_values(
                        ":snapshotOrdinal",
                        n(u64::from(accounting.snapshot_ordinal)),
                    );
                if let Some(suspended_at) = accounting.suspended_at {
                    update.push_str(", suspendedAt = :suspendedAt");
                    builder =
                        builder.expression_attribute_values(":suspendedAt", stamp(suspended_at));
                } else {
                    remove.push("suspendedAt");
                }
                if let Some(launched_at) = accounting.lifetime_started_at {
                    update.push_str(
                        ", providerLaunchedAt = :providerLaunchedAt, providerLifetimeExpiresAt = :providerLifetimeExpiresAt",
                    );
                    builder = builder
                        .expression_attribute_values(":providerLaunchedAt", stamp(launched_at))
                        .expression_attribute_values(
                            ":providerLifetimeExpiresAt",
                            stamp(aex_runtime_control::clock::plus_millis(
                                launched_at,
                                aex_runtime_control::lifecycle::PROVIDER_LIFETIME_MS,
                            )),
                        );
                }
            }
            if !evaluable {
                remove.extend([keys::DUE_PK, keys::DUE_SK]);
            }
            if !remove.is_empty() {
                update.push_str(" REMOVE ");
                update.push_str(&remove.join(", "));
            }
            let head_update = builder
                .update_expression(update)
                .return_values_on_condition_check_failure(
                    ReturnValuesOnConditionCheckFailure::AllOld,
                )
                .build()
                .map_err(|error| malformed(&error.to_string()))?;
            let current = keys::current(before.session);
            let current_update = Update::builder()
                .table_name(&self.table)
                .set_key(Some(key(&current.pk, &current.sk)))
                .condition_expression("generationId = :generation AND fence = :expectedFence")
                .update_expression(
                    "SET fence = :nextFence, revision = :nextRevision, updatedAt = :at",
                )
                .expression_attribute_values(":generation", s(plan.generation.to_string()))
                .expression_attribute_values(":expectedFence", n(plan.expected_fence.0))
                .expression_attribute_values(":nextFence", n(plan.next_fence.0))
                .expression_attribute_values(":nextRevision", n(next_revision.value()))
                .expression_attribute_values(":at", stamp(plan.at))
                .return_values_on_condition_check_failure(
                    ReturnValuesOnConditionCheckFailure::AllOld,
                )
                .build()
                .map_err(|error| malformed(&error.to_string()))?;
            self.transact(
                vec![
                    TransactWriteItem::builder().update(head_update).build(),
                    TransactWriteItem::builder().update(current_update).build(),
                ],
                transaction_token(
                    "generation",
                    plan.generation,
                    &format!(
                        "{}:{}:{}",
                        plan.expected_revision.value(),
                        next_revision.value(),
                        plan.next_fence.0
                    ),
                ),
            )
            .await
            .map_err(|error| match error {
                RuntimeStoreError::RevisionConflict { found, .. } => {
                    RuntimeStoreError::RevisionConflict {
                        expected: plan.expected_revision,
                        found,
                    }
                }
                other => other,
            })?;
            let persisted = self.load_canonical_row(plan.generation).await?.ok_or(
                RuntimeStoreError::NoSuchGeneration {
                    generation: plan.generation,
                },
            )?;
            Ok(GenerationCommit {
                head: generation_view(persisted).head,
                revision: next_revision,
            })
        })
    }

    fn admit_operation<'a>(&'a self, plan: &'a OperationAdmissionPlan) -> StoreFuture<'a, ()> {
        Box::pin(async move {
            if plan.open_operations == 0 || plan.next_revision != plan.expected_revision.next() {
                return Err(malformed(
                    "an operation admission plan is internally inconsistent",
                ));
            }
            let row = self.load_canonical_row(plan.generation).await?.ok_or(
                RuntimeStoreError::NoSuchGeneration {
                    generation: plan.generation,
                },
            )?;
            if self
                .operation_is_admitted(plan.generation, plan.operation)
                .await?
            {
                return Ok(());
            }
            let expected_open = plan.open_operations - 1;
            let target = keys::head_for_generation(plan.generation);
            let head = Update::builder()
                .table_name(&self.table)
                .set_key(Some(key(&target.pk, &target.sk)))
                .condition_expression(
                    "generationId = :generation AND #state = :running AND fence = :fence AND revision = :expectedRevision AND openOperations = :expectedOpen",
                )
                .update_expression(
                    "SET openOperations = :open, revision = :nextRevision, lastBusyAt = :at, updatedAt = :at REMOVE idleSince",
                )
                .expression_attribute_names("#state", "state")
                .expression_attribute_values(":generation", s(plan.generation.to_string()))
                .expression_attribute_values(":running", s(keys::state_str(GenerationState::Running)))
                .expression_attribute_values(":fence", n(plan.fence.0))
                .expression_attribute_values(":expectedRevision", n(plan.expected_revision.value()))
                .expression_attribute_values(":expectedOpen", n(u64::from(expected_open)))
                .expression_attribute_values(":open", n(u64::from(plan.open_operations)))
                .expression_attribute_values(":nextRevision", n(plan.next_revision.value()))
                .expression_attribute_values(":at", stamp(plan.last_busy_at))
                .build()
                .map_err(|error| malformed(&error.to_string()))?;
            let current = keys::current(row.session);
            let pointer = Update::builder()
                .table_name(&self.table)
                .set_key(Some(key(&current.pk, &current.sk)))
                .condition_expression(
                    "generationId = :generation AND fence = :fence AND revision = :expectedRevision",
                )
                .update_expression("SET revision = :nextRevision, updatedAt = :at")
                .expression_attribute_values(":generation", s(plan.generation.to_string()))
                .expression_attribute_values(":fence", n(plan.fence.0))
                .expression_attribute_values(":expectedRevision", n(plan.expected_revision.value()))
                .expression_attribute_values(":nextRevision", n(plan.next_revision.value()))
                .expression_attribute_values(":at", stamp(plan.last_busy_at))
                .build()
                .map_err(|error| malformed(&error.to_string()))?;
            let marker = keys::operation_admission(plan.generation, plan.operation);
            let marker = ItemBuilder::new(HANDS_OPERATION_ADMISSION)
                .set(PK, s(marker.pk))
                .set(SK, s(marker.sk))
                .set("generationId", s(plan.generation.to_string()))
                .set("operationId", s(plan.operation.0.to_string()))
                .set("admittedAt", stamp(plan.last_busy_at))
                .build();
            let marker = Put::builder()
                .table_name(&self.table)
                .set_item(Some(marker))
                .condition_expression("attribute_not_exists(pk)")
                .build()
                .map_err(|error| malformed(&error.to_string()))?;
            let outcome = self
                .transact(
                    vec![
                        TransactWriteItem::builder().put(marker).build(),
                        TransactWriteItem::builder().update(head).build(),
                        TransactWriteItem::builder().update(pointer).build(),
                    ],
                    transaction_token(
                        "operation-admit",
                        plan.generation,
                        &format!("{}:{}", plan.operation.0, plan.expected_revision.value()),
                    ),
                )
                .await;
            if outcome.is_err()
                && self
                    .operation_is_admitted(plan.generation, plan.operation)
                    .await?
            {
                return Ok(());
            }
            outcome
        })
    }

    fn settle_operation<'a>(&'a self, plan: &'a OperationSettlementPlan) -> StoreFuture<'a, ()> {
        Box::pin(async move {
            if plan.next_revision != plan.expected_revision.next() {
                return Err(malformed(
                    "an operation settlement plan is internally inconsistent",
                ));
            }
            let row = self.load_canonical_row(plan.generation).await?.ok_or(
                RuntimeStoreError::NoSuchGeneration {
                    generation: plan.generation,
                },
            )?;
            if !self
                .operation_is_admitted(plan.generation, plan.operation)
                .await?
            {
                return Ok(());
            }
            let expected_open = plan
                .open_operations
                .checked_add(1)
                .ok_or_else(|| malformed("an operation settlement count cannot exceed u32"))?;
            let target = keys::head_for_generation(plan.generation);
            let update = if plan.open_operations == 0 {
                "SET openOperations = :open, revision = :nextRevision, lastBusyAt = :at, idleSince = :at, updatedAt = :at"
            } else {
                "SET openOperations = :open, revision = :nextRevision, lastBusyAt = :at, updatedAt = :at REMOVE idleSince"
            };
            let head = Update::builder()
                .table_name(&self.table)
                .set_key(Some(key(&target.pk, &target.sk)))
                .condition_expression(
                    "generationId = :generation AND revision = :expectedRevision AND openOperations = :expectedOpen",
                )
                .update_expression(update)
                .expression_attribute_values(":generation", s(plan.generation.to_string()))
                .expression_attribute_values(":expectedRevision", n(plan.expected_revision.value()))
                .expression_attribute_values(":expectedOpen", n(u64::from(expected_open)))
                .expression_attribute_values(":open", n(u64::from(plan.open_operations)))
                .expression_attribute_values(":nextRevision", n(plan.next_revision.value()))
                .expression_attribute_values(":at", stamp(plan.last_busy_at))
                .build()
                .map_err(|error| malformed(&error.to_string()))?;
            let current = keys::current(row.session);
            let pointer = Update::builder()
                .table_name(&self.table)
                .set_key(Some(key(&current.pk, &current.sk)))
                .condition_expression("generationId = :generation AND revision = :expectedRevision")
                .update_expression("SET revision = :nextRevision, updatedAt = :at")
                .expression_attribute_values(":generation", s(plan.generation.to_string()))
                .expression_attribute_values(":expectedRevision", n(plan.expected_revision.value()))
                .expression_attribute_values(":nextRevision", n(plan.next_revision.value()))
                .expression_attribute_values(":at", stamp(plan.last_busy_at))
                .build()
                .map_err(|error| malformed(&error.to_string()))?;
            let marker = keys::operation_admission(plan.generation, plan.operation);
            let marker = Delete::builder()
                .table_name(&self.table)
                .set_key(Some(key(&marker.pk, &marker.sk)))
                .condition_expression("attribute_exists(pk)")
                .build()
                .map_err(|error| malformed(&error.to_string()))?;
            let outcome = self
                .transact(
                    vec![
                        TransactWriteItem::builder().delete(marker).build(),
                        TransactWriteItem::builder().update(head).build(),
                        TransactWriteItem::builder().update(pointer).build(),
                    ],
                    transaction_token(
                        "operation-settle",
                        plan.generation,
                        &format!("{}:{}", plan.operation.0, plan.expected_revision.value()),
                    ),
                )
                .await;
            if outcome.is_err()
                && !self
                    .operation_is_admitted(plan.generation, plan.operation)
                    .await?
            {
                return Ok(());
            }
            outcome
        })
    }

    #[allow(
        clippy::too_many_lines,
        reason = "one contiguous builder keeps the fenced HEAD, CURRENT pointer, and immutable intent transaction auditable as one atomic unit"
    )]
    fn record_intent<'a>(
        &'a self,
        plan: &'a LifecycleIntentPlan,
    ) -> StoreFuture<'a, LifecycleIntentCommit> {
        Box::pin(async move {
            let row = self.load_canonical_row(plan.generation).await?.ok_or(
                RuntimeStoreError::NoSuchGeneration {
                    generation: plan.generation,
                },
            )?;
            if let Some(intent) = &row.open_intent
                && !intent.permits_new_effect()
            {
                return Err(RuntimeStoreError::IntentOpen {
                    intent_id: intent.intent_id.clone(),
                    state: intent.state,
                });
            }
            if row.state != plan.expected_state
                || row.fence != plan.expected_fence
                || row.revision != plan.expected_revision
            {
                return Err(RuntimeStoreError::RevisionConflict {
                    expected: plan.expected_revision,
                    found: Some(row.revision),
                });
            }
            if !plan.expected_state.permits(plan.next_state)
                || plan.fence.0 <= plan.expected_fence.0
            {
                return Err(RuntimeStoreError::Malformed {
                    reason: format!(
                        "lifecycle intent cannot move {:?}/{} to {:?}/{}",
                        plan.expected_state, plan.expected_fence.0, plan.next_state, plan.fence.0
                    ),
                });
            }
            let record = IntentRecord {
                intent_id: plan.intent_id.clone(),
                generation: plan.generation,
                microvm: plan.microvm.clone(),
                action: plan.action,
                fence: plan.fence,
                state: IntentState::Dispatched,
                provider_request_id: None,
                attempts: 0,
                dispatched_at: plan.dispatched_at,
            };
            let encoded =
                serde_json::to_string(&record).map_err(|error| RuntimeStoreError::Malformed {
                    reason: error.to_string(),
                })?;
            let target = keys::head_for_generation(plan.generation);
            let next_revision = plan.expected_revision.next();
            let mut head_expression = String::from(
                "SET #state = :nextState, fence = :nextFence, revision = :nextRevision, updatedAt = :at, openIntent = :intent",
            );
            let mut head_builder = Update::builder()
                .table_name(&self.table)
                .set_key(Some(key(&target.pk, &target.sk)))
                .condition_expression(
                    "generationId = :generation AND #state = :expectedState AND fence = :expectedFence AND revision = :expectedRevision AND attribute_not_exists(openIntent)",
                )
                .expression_attribute_names("#state", "state")
                .expression_attribute_values(":generation", s(plan.generation.to_string()))
                .expression_attribute_values(
                    ":expectedState",
                    s(keys::state_str(plan.expected_state)),
                )
                .expression_attribute_values(":expectedFence", n(plan.expected_fence.0))
                .expression_attribute_values(
                    ":expectedRevision",
                    n(plan.expected_revision.value()),
                )
                .expression_attribute_values(":nextState", s(keys::state_str(plan.next_state)))
                .expression_attribute_values(":nextFence", n(plan.fence.0))
                .expression_attribute_values(":nextRevision", n(next_revision.value()))
                .expression_attribute_values(":at", stamp(plan.dispatched_at))
                .expression_attribute_values(":intent", s(encoded));
            if let Some(microvm) = &plan.microvm {
                head_expression.push_str(", providerVmId = :microvm");
                head_builder =
                    head_builder.expression_attribute_values(":microvm", s(microvm.0.clone()));
            }
            let update = head_builder
                .update_expression(head_expression)
                .build()
                .map_err(|error| RuntimeStoreError::Malformed {
                    reason: error.to_string(),
                })?;
            let current = keys::current(row.session);
            let current_update = Update::builder()
                .table_name(&self.table)
                .set_key(Some(key(&current.pk, &current.sk)))
                .condition_expression(
                    "generationId = :generation AND fence = :expectedFence AND revision = :expectedRevision",
                )
                .update_expression(
                    "SET fence = :nextFence, revision = :nextRevision, updatedAt = :at",
                )
                .expression_attribute_values(":generation", s(plan.generation.to_string()))
                .expression_attribute_values(":expectedFence", n(plan.expected_fence.0))
                .expression_attribute_values(
                    ":expectedRevision",
                    n(plan.expected_revision.value()),
                )
                .expression_attribute_values(":nextFence", n(plan.fence.0))
                .expression_attribute_values(":nextRevision", n(next_revision.value()))
                .expression_attribute_values(":at", stamp(plan.dispatched_at))
                .build()
                .map_err(|error| RuntimeStoreError::Malformed {
                    reason: error.to_string(),
                })?;
            let durable = LifecycleIntent {
                session: row.session,
                workspace: row.workspace,
                generation: plan.generation,
                intent_id: plan.intent_id.0.clone(),
                action: action_str(plan.action).to_owned(),
                requested_fence: plan.fence,
                state: "dispatched".to_owned(),
                provider_request_id: None,
                requested_at: plan.dispatched_at,
                dispatched_at: Some(plan.dispatched_at),
                reconcile_attempts: 0,
                last_reconciled_at: None,
            };
            let put = expressions::record_intent(&self.table, &durable)
                .map_err(|error| runtime_error(error, None))?
                .build()
                .map_err(|error| RuntimeStoreError::Malformed {
                    reason: error.to_string(),
                })?;
            self.transact(
                vec![
                    TransactWriteItem::builder().update(update).build(),
                    TransactWriteItem::builder().update(current_update).build(),
                    TransactWriteItem::builder().put(put).build(),
                ],
                transaction_token("intent", plan.generation, &plan.intent_id.0),
            )
            .await?;
            let persisted = self.load_canonical_row(plan.generation).await?.ok_or(
                RuntimeStoreError::NoSuchGeneration {
                    generation: plan.generation,
                },
            )?;
            Ok(LifecycleIntentCommit {
                generation: GenerationCommit {
                    head: generation_view(persisted).head,
                    revision: next_revision,
                },
                intent: record,
            })
        })
    }

    fn record_provider_request<'a>(
        &'a self,
        plan: &'a LifecycleRequestPlan,
    ) -> StoreFuture<'a, IntentRecord> {
        Box::pin(async move {
            let row = self.load_canonical_row(plan.generation).await?.ok_or(
                RuntimeStoreError::NoSuchGeneration {
                    generation: plan.generation,
                },
            )?;
            let Some(mut intent) = row.open_intent else {
                return Err(malformed("provider request has no open lifecycle intent"));
            };
            if intent.intent_id != plan.intent_id {
                return Err(RuntimeStoreError::IntentOpen {
                    intent_id: intent.intent_id,
                    state: intent.state,
                });
            }
            if let Some(existing) = &intent.provider_request_id {
                if existing == &plan.provider_request_id
                    && intent.microvm.as_ref() == Some(&plan.microvm)
                {
                    return Ok(intent);
                }
                return Err(malformed(
                    "one lifecycle intent has conflicting provider evidence",
                ));
            }
            if intent
                .microvm
                .as_ref()
                .is_some_and(|existing| existing != &plan.microvm)
            {
                return Err(malformed(
                    "one lifecycle intent has two provider MicroVM identities",
                ));
            }
            let expected =
                serde_json::to_string(&intent).map_err(|error| malformed(&error.to_string()))?;
            intent.microvm = Some(plan.microvm.clone());
            intent.provider_request_id = Some(plan.provider_request_id.clone());
            let next =
                serde_json::to_string(&intent).map_err(|error| malformed(&error.to_string()))?;
            let target = keys::head_for_generation(plan.generation);
            let head = Update::builder()
                .table_name(&self.table)
                .set_key(Some(key(&target.pk, &target.sk)))
                .condition_expression("openIntent = :expected")
                .update_expression("SET openIntent = :next, providerVmId = :microvm")
                .expression_attribute_values(":expected", s(expected))
                .expression_attribute_values(":next", s(next))
                .expression_attribute_values(":microvm", s(plan.microvm.0.clone()))
                .build()
                .map_err(|error| malformed(&error.to_string()))?;
            let durable_target = keys::intent(row.session, plan.generation, &plan.intent_id.0)
                .map_err(|error| malformed(&error.to_string()))?;
            let durable = Update::builder()
                .table_name(&self.table)
                .set_key(Some(key(&durable_target.pk, &durable_target.sk)))
                .condition_expression(
                    "#state IN (:dispatched, :unknown) AND attribute_not_exists(providerRequestId)",
                )
                .update_expression("SET providerRequestId = :request")
                .expression_attribute_names("#state", "state")
                .expression_attribute_values(":dispatched", s("dispatched"))
                .expression_attribute_values(":unknown", s("unknown"))
                .expression_attribute_values(":request", s(plan.provider_request_id.0.clone()))
                .build()
                .map_err(|error| malformed(&error.to_string()))?;
            self.transact(
                vec![
                    TransactWriteItem::builder().update(head).build(),
                    TransactWriteItem::builder().update(durable).build(),
                ],
                transaction_token("request", plan.generation, &plan.intent_id.0),
            )
            .await?;
            Ok(intent)
        })
    }

    fn record_reconcile_attempt<'a>(
        &'a self,
        plan: &'a LifecycleReconcilePlan,
    ) -> StoreFuture<'a, IntentRecord> {
        Box::pin(async move {
            let row = self.load_canonical_row(plan.generation).await?.ok_or(
                RuntimeStoreError::NoSuchGeneration {
                    generation: plan.generation,
                },
            )?;
            let Some(mut intent) = row.open_intent else {
                return Err(malformed("reconciliation has no open lifecycle intent"));
            };
            if intent.intent_id != plan.intent_id {
                return Err(RuntimeStoreError::IntentOpen {
                    intent_id: intent.intent_id,
                    state: intent.state,
                });
            }
            if intent.attempts != plan.expected_attempts {
                return Err(RuntimeStoreError::ReconcileConflict {
                    expected: plan.expected_attempts,
                    found: intent.attempts,
                });
            }
            if !matches!(intent.state, IntentState::Dispatched | IntentState::Unknown) {
                return Err(RuntimeStoreError::IntentOpen {
                    intent_id: intent.intent_id,
                    state: intent.state,
                });
            }
            let expected =
                serde_json::to_string(&intent).map_err(|error| malformed(&error.to_string()))?;
            intent.attempts = intent.attempts.saturating_add(1).min(RECONCILE_ATTEMPTS);
            if intent.attempts >= RECONCILE_ATTEMPTS {
                intent.state = IntentState::Quarantined;
            }
            let next =
                serde_json::to_string(&intent).map_err(|error| malformed(&error.to_string()))?;
            let target = keys::head_for_generation(plan.generation);
            let (head_expression, mut head_builder) = if intent.state == IntentState::Quarantined {
                (
                    "SET openIntent = :next, updatedAt = :at REMOVE nextEvaluateAt, rtDuePk, rtDueSk",
                    Update::builder(),
                )
            } else {
                (
                    "SET openIntent = :next, nextEvaluateAt = :due, rtDueSk = :dueSk, updatedAt = :at",
                    Update::builder()
                        .expression_attribute_values(":due", stamp(plan.next_evaluate_at))
                        .expression_attribute_values(
                            ":dueSk",
                            s(keys::due_sort(plan.next_evaluate_at, plan.generation)),
                        ),
                )
            };
            head_builder = head_builder
                .table_name(&self.table)
                .set_key(Some(key(&target.pk, &target.sk)))
                .condition_expression("openIntent = :expected")
                .update_expression(head_expression)
                .expression_attribute_values(":expected", s(expected))
                .expression_attribute_values(":next", s(next))
                .expression_attribute_values(":at", stamp(plan.reconciled_at));
            let head = head_builder
                .build()
                .map_err(|error| malformed(&error.to_string()))?;
            let durable_target = keys::intent(row.session, plan.generation, &plan.intent_id.0)
                .map_err(|error| malformed(&error.to_string()))?;
            let durable = Update::builder()
                .table_name(&self.table)
                .set_key(Some(key(&durable_target.pk, &durable_target.sk)))
                .condition_expression("reconcileAttempts = :expectedAttempts")
                .update_expression(
                    "SET #state = :state, reconcileAttempts = :attempts, lastReconciledAt = :at",
                )
                .expression_attribute_names("#state", "state")
                .expression_attribute_values(
                    ":expectedAttempts",
                    n(u64::from(plan.expected_attempts)),
                )
                .expression_attribute_values(":state", s(intent_state_str(intent.state)))
                .expression_attribute_values(":attempts", n(u64::from(intent.attempts)))
                .expression_attribute_values(":at", stamp(plan.reconciled_at))
                .build()
                .map_err(|error| malformed(&error.to_string()))?;
            self.transact(
                vec![
                    TransactWriteItem::builder().update(head).build(),
                    TransactWriteItem::builder().update(durable).build(),
                ],
                transaction_token(
                    "reconcile",
                    plan.generation,
                    &format!("{}:{}", plan.intent_id.0, intent.attempts),
                ),
            )
            .await?;
            Ok(intent)
        })
    }

    #[allow(
        clippy::too_many_lines,
        reason = "one contiguous builder keeps the lifecycle receipt, intent, head, and bounded usage outbox transaction auditable as one atomic unit"
    )]
    fn settle_intent<'a>(
        &'a self,
        plan: &'a LifecycleReceiptPlan,
    ) -> StoreFuture<'a, CanonicalReceipt> {
        Box::pin(async move {
            let row = self.load_canonical_row(plan.generation).await?.ok_or(
                RuntimeStoreError::NoSuchGeneration {
                    generation: plan.generation,
                },
            )?;
            let Some(mut intent) = row.open_intent else {
                let receipt_key = keys::receipt(row.session, plan.generation, &plan.intent_id.0)
                    .map_err(|error| RuntimeStoreError::Malformed {
                        reason: error.to_string(),
                    })?;
                let stored = self
                    .get(&receipt_key.pk, &receipt_key.sk)
                    .await
                    .map_err(|error| runtime_error(error, None))?
                    .ok_or_else(|| RuntimeStoreError::Malformed {
                        reason: format!(
                            "generation {} has neither open intent nor receipt {}",
                            plan.generation, plan.intent_id
                        ),
                    })?;
                let text = |name: &str| stored.get(name).and_then(|value| value.as_s().ok());
                let stored_intent: IntentRecord = serde_json::from_str(
                    text("intent")
                        .ok_or_else(|| malformed("lifecycle receipt has no stored intent"))?,
                )
                .map_err(|error| malformed(&error.to_string()))?;
                if stored_intent.intent_id != plan.intent_id {
                    return Err(malformed("lifecycle receipt intent identity disagrees"));
                }
                let receipt_id = text("receiptId")
                    .ok_or_else(|| malformed("lifecycle receipt has no receiptId"))?
                    .to_owned();
                let snapshot = text("snapshot")
                    .map(|value| serde_json::from_str(value))
                    .transpose()
                    .map_err(|error| malformed(&error.to_string()))?;
                return Ok(CanonicalReceipt {
                    intent: stored_intent,
                    receipt_id,
                    snapshot,
                });
            };
            if intent.intent_id != plan.intent_id {
                return Err(RuntimeStoreError::IntentOpen {
                    intent_id: intent.intent_id,
                    state: intent.state,
                });
            }
            let prior_state = intent.state;
            let expected =
                serde_json::to_string(&intent).map_err(|error| RuntimeStoreError::Malformed {
                    reason: error.to_string(),
                })?;
            intent.state = plan.next_intent_state;
            intent.provider_request_id = plan.provider_request_id.clone();
            let target = keys::head_for_generation(plan.generation);
            let durable_target = keys::intent(row.session, plan.generation, &plan.intent_id.0)
                .map_err(|error| RuntimeStoreError::Malformed {
                    reason: error.to_string(),
                })?;
            let durable_intent_update = || {
                let mut update = String::from("SET #state = :nextState");
                let mut builder = Update::builder()
                    .table_name(&self.table)
                    .set_key(Some(key(&durable_target.pk, &durable_target.sk)))
                    .condition_expression("#state = :priorState")
                    .expression_attribute_names("#state", "state")
                    .expression_attribute_values(":priorState", s(intent_state_str(prior_state)))
                    .expression_attribute_values(
                        ":nextState",
                        s(intent_state_str(plan.next_intent_state)),
                    );
                if let Some(request) = &plan.provider_request_id {
                    update.push_str(", providerRequestId = :providerRequestId");
                    builder = builder
                        .expression_attribute_values(":providerRequestId", s(request.0.clone()));
                }
                builder.update_expression(update).build().map_err(|error| {
                    RuntimeStoreError::Malformed {
                        reason: error.to_string(),
                    }
                })
            };
            if plan.next_intent_state == IntentState::Unknown {
                if !plan.usage.is_empty() {
                    return Err(malformed("an unknown provider outcome cannot emit usage"));
                }
                let next = serde_json::to_string(&intent).map_err(|error| {
                    RuntimeStoreError::Malformed {
                        reason: error.to_string(),
                    }
                })?;
                let mut transaction = Vec::with_capacity(3);
                if let Some(generation) = &plan.generation_commit {
                    if generation.generation != plan.generation
                        || generation.next_state != GenerationState::Unknown
                    {
                        return Err(malformed(
                            "an unknown outcome may only move its own generation to unknown",
                        ));
                    }
                    let next_revision = generation.expected_revision.next();
                    let head_update = Update::builder()
                        .table_name(&self.table)
                        .set_key(Some(key(&target.pk, &target.sk)))
                        .condition_expression(
                            "generationId = :generation AND #state = :expectedState AND fence = :expectedFence AND revision = :expectedRevision AND openIntent = :expected",
                        )
                        .update_expression(
                            "SET #state = :nextState, fence = :nextFence, revision = :nextRevision, updatedAt = :at, openIntent = :next",
                        )
                        .expression_attribute_names("#state", "state")
                        .expression_attribute_values(
                            ":generation",
                            s(generation.generation.to_string()),
                        )
                        .expression_attribute_values(
                            ":expectedState",
                            s(keys::state_str(generation.expected_state)),
                        )
                        .expression_attribute_values(
                            ":expectedFence",
                            n(generation.expected_fence.0),
                        )
                        .expression_attribute_values(
                            ":expectedRevision",
                            n(generation.expected_revision.value()),
                        )
                        .expression_attribute_values(
                            ":nextState",
                            s(keys::state_str(generation.next_state)),
                        )
                        .expression_attribute_values(":nextFence", n(generation.next_fence.0))
                        .expression_attribute_values(":nextRevision", n(next_revision.value()))
                        .expression_attribute_values(":at", stamp(generation.at))
                        .expression_attribute_values(":expected", s(expected))
                        .expression_attribute_values(":next", s(next))
                        .build()
                        .map_err(|error| malformed(&error.to_string()))?;
                    transaction.push(TransactWriteItem::builder().update(head_update).build());
                    let current = keys::current(row.session);
                    let current_update = Update::builder()
                        .table_name(&self.table)
                        .set_key(Some(key(&current.pk, &current.sk)))
                        .condition_expression(
                            "generationId = :generation AND fence = :expectedFence AND revision = :expectedRevision",
                        )
                        .update_expression(
                            "SET fence = :nextFence, revision = :nextRevision, updatedAt = :at",
                        )
                        .expression_attribute_values(
                            ":generation",
                            s(generation.generation.to_string()),
                        )
                        .expression_attribute_values(
                            ":expectedFence",
                            n(generation.expected_fence.0),
                        )
                        .expression_attribute_values(
                            ":expectedRevision",
                            n(generation.expected_revision.value()),
                        )
                        .expression_attribute_values(":nextFence", n(generation.next_fence.0))
                        .expression_attribute_values(":nextRevision", n(next_revision.value()))
                        .expression_attribute_values(":at", stamp(generation.at))
                        .build()
                        .map_err(|error| malformed(&error.to_string()))?;
                    transaction.push(TransactWriteItem::builder().update(current_update).build());
                } else {
                    let head_update = Update::builder()
                        .table_name(&self.table)
                        .set_key(Some(key(&target.pk, &target.sk)))
                        .condition_expression("openIntent = :expected")
                        .update_expression("SET openIntent = :next")
                        .expression_attribute_values(":expected", s(expected))
                        .expression_attribute_values(":next", s(next))
                        .build()
                        .map_err(|error| RuntimeStoreError::Malformed {
                            reason: error.to_string(),
                        })?;
                    transaction.push(TransactWriteItem::builder().update(head_update).build());
                }
                transaction.push(
                    TransactWriteItem::builder()
                        .update(durable_intent_update()?)
                        .build(),
                );
                self.transact(
                    transaction,
                    transaction_token("unknown", plan.generation, &plan.intent_id.0),
                )
                .await?;
                return Ok(CanonicalReceipt {
                    intent,
                    receipt_id: format!("unknown:{}:{}", plan.generation, plan.intent_id.0),
                    snapshot: plan.snapshot.clone(),
                });
            }
            let receipt_id = intent.receipt_id().unwrap_or_else(|_| {
                format!("aex-runtime:{}:{}", plan.generation, plan.intent_id.0)
            });
            let receipt_key = keys::receipt(row.session, plan.generation, &plan.intent_id.0)
                .map_err(|error| RuntimeStoreError::Malformed {
                    reason: error.to_string(),
                })?;
            let receipt_item = ItemBuilder::new(codec::LIFECYCLE_RECEIPT)
                .set(PK, s(receipt_key.pk))
                .set(SK, s(receipt_key.sk))
                .set("generationId", s(plan.generation.to_string()))
                .set("intentId", s(plan.intent_id.0.clone()))
                .set("receiptId", s(receipt_id.clone()))
                .set(
                    "intent",
                    s(serde_json::to_string(&intent).map_err(|error| {
                        RuntimeStoreError::Malformed {
                            reason: error.to_string(),
                        }
                    })?),
                )
                .set_opt(
                    "snapshot",
                    plan.snapshot.as_ref().map(|snapshot| {
                        s(serde_json::to_string(snapshot).expect("SnapshotResidence serializes"))
                    }),
                )
                .set("settledAt", stamp(plan.settled_at))
                .build();
            let mut head_update = String::new();
            let mut remove = vec!["openIntent"];
            let mut head_builder = Update::builder()
                .table_name(&self.table)
                .set_key(Some(key(&target.pk, &target.sk)))
                .expression_attribute_values(":expected", s(expected));
            if let Some(generation) = &plan.generation_commit {
                if generation.generation != plan.generation {
                    return Err(malformed(
                        "the receipt and final generation transition name different generations",
                    ));
                }
                let next_revision = generation.expected_revision.next();
                head_update.push_str(
                    "SET #state = :nextState, fence = :nextFence, revision = :nextRevision, updatedAt = :at",
                );
                head_builder = head_builder
                    .condition_expression(
                        "generationId = :generation AND #state = :expectedState AND fence = :expectedFence AND revision = :expectedRevision AND openIntent = :expected",
                    )
                    .expression_attribute_names("#state", "state")
                    .expression_attribute_values(
                        ":generation",
                        s(generation.generation.to_string()),
                    )
                    .expression_attribute_values(
                        ":expectedState",
                        s(keys::state_str(generation.expected_state)),
                    )
                    .expression_attribute_values(
                        ":expectedFence",
                        n(generation.expected_fence.0),
                    )
                    .expression_attribute_values(
                        ":expectedRevision",
                        n(generation.expected_revision.value()),
                    )
                    .expression_attribute_values(
                        ":nextState",
                        s(keys::state_str(generation.next_state)),
                    )
                    .expression_attribute_values(":nextFence", n(generation.next_fence.0))
                    .expression_attribute_values(":nextRevision", n(next_revision.value()))
                    .expression_attribute_values(":at", stamp(generation.at));
                if let Some(accounting) = generation.accounting {
                    head_update.push_str(
                        ", accountedFrom = :accountedFrom, snapshotOrdinal = :snapshotOrdinal",
                    );
                    head_builder = head_builder
                        .expression_attribute_values(
                            ":accountedFrom",
                            stamp(accounting.accounted_from),
                        )
                        .expression_attribute_values(
                            ":snapshotOrdinal",
                            n(u64::from(accounting.snapshot_ordinal)),
                        );
                    if let Some(suspended_at) = accounting.suspended_at {
                        head_update.push_str(", suspendedAt = :suspendedAt");
                        head_builder = head_builder
                            .expression_attribute_values(":suspendedAt", stamp(suspended_at));
                    } else {
                        remove.push("suspendedAt");
                    }
                    if let Some(launched_at) = accounting.lifetime_started_at {
                        head_update.push_str(
                            ", providerLaunchedAt = :providerLaunchedAt, providerLifetimeExpiresAt = :providerLifetimeExpiresAt",
                        );
                        head_builder = head_builder
                            .expression_attribute_values(":providerLaunchedAt", stamp(launched_at))
                            .expression_attribute_values(
                                ":providerLifetimeExpiresAt",
                                stamp(aex_runtime_control::clock::plus_millis(
                                    launched_at,
                                    aex_runtime_control::lifecycle::PROVIDER_LIFETIME_MS,
                                )),
                            );
                    }
                }
                if !keys::is_evaluable(generation.next_state) {
                    remove.extend([keys::DUE_PK, keys::DUE_SK]);
                }
            } else {
                head_builder = head_builder.condition_expression("openIntent = :expected");
            }
            if head_update.is_empty() {
                head_update.push_str("REMOVE ");
            } else {
                head_update.push_str(" REMOVE ");
            }
            head_update.push_str(&remove.join(", "));
            let update = head_builder
                .update_expression(head_update)
                .return_values_on_condition_check_failure(
                    ReturnValuesOnConditionCheckFailure::AllOld,
                )
                .build()
                .map_err(|error| RuntimeStoreError::Malformed {
                    reason: error.to_string(),
                })?;
            let put = Put::builder()
                .table_name(&self.table)
                .set_item(Some(receipt_item))
                .condition_expression("attribute_not_exists(pk)")
                .build()
                .map_err(|error| RuntimeStoreError::Malformed {
                    reason: error.to_string(),
                })?;
            if plan.usage.len() > 3 {
                return Err(RuntimeStoreError::Malformed {
                    reason: format!(
                        "one lifecycle interval produced {} usage drafts, over the three-meter ceiling",
                        plan.usage.len()
                    ),
                });
            }
            let mut transaction = vec![
                TransactWriteItem::builder().update(update).build(),
                TransactWriteItem::builder()
                    .update(durable_intent_update()?)
                    .build(),
                TransactWriteItem::builder().put(put).build(),
            ];
            if let Some(generation) = &plan.generation_commit {
                let next_revision = generation.expected_revision.next();
                let current = keys::current(row.session);
                let current_update = Update::builder()
                    .table_name(&self.table)
                    .set_key(Some(key(&current.pk, &current.sk)))
                    .condition_expression(
                        "generationId = :generation AND fence = :expectedFence AND revision = :expectedRevision",
                    )
                    .update_expression(
                        "SET fence = :nextFence, revision = :nextRevision, updatedAt = :at",
                    )
                    .expression_attribute_values(
                        ":generation",
                        s(generation.generation.to_string()),
                    )
                    .expression_attribute_values(
                        ":expectedFence",
                        n(generation.expected_fence.0),
                    )
                    .expression_attribute_values(
                        ":expectedRevision",
                        n(generation.expected_revision.value()),
                    )
                    .expression_attribute_values(":nextFence", n(generation.next_fence.0))
                    .expression_attribute_values(":nextRevision", n(next_revision.value()))
                    .expression_attribute_values(":at", stamp(generation.at))
                    .build()
                    .map_err(|error| malformed(&error.to_string()))?;
                transaction.push(TransactWriteItem::builder().update(current_update).build());
            }
            let (outbox_state, outbox_fence, outbox_revision) = plan
                .generation_commit
                .as_ref()
                .map_or((row.state, row.fence, row.revision), |generation| {
                    (
                        generation.next_state,
                        generation.next_fence,
                        generation.expected_revision.next(),
                    )
                });
            for pending in &plan.usage {
                if pending.category != pending.draft.authority.category {
                    return Err(RuntimeStoreError::Malformed {
                        reason: format!(
                            "usage outbox category {} disagrees with draft authority {}",
                            pending.category, pending.draft.authority.category
                        ),
                    });
                }
                let fact_id = pending.draft.fact_id().to_string();
                let target = keys::usage_outbox(plan.generation, &fact_id);
                let body = serde_json::to_string(&pending.draft).map_err(|error| {
                    RuntimeStoreError::Malformed {
                        reason: format!("usage draft {fact_id} does not serialize: {error}"),
                    }
                })?;
                let item = ItemBuilder::new("usage_outbox")
                    .set(PK, s(target.pk))
                    .set(SK, s(target.sk))
                    .set("sessionId", s(row.session.to_string()))
                    .set("workspaceId", s(row.workspace.to_string()))
                    .set("generationId", s(plan.generation.to_string()))
                    .set("factId", s(fact_id.clone()))
                    .set("category", s(pending.category.id()))
                    .set("draft", s(body))
                    .set("state", s(keys::state_str(outbox_state)))
                    .set("fence", n(outbox_fence.0))
                    .set("revision", n(outbox_revision.value()))
                    .set("enqueuedAt", stamp(plan.settled_at))
                    .set(keys::DUE_PK, s(keys::due_partition(plan.generation)))
                    .set(
                        keys::DUE_SK,
                        s(format!(
                            "{}#{}#{fact_id}",
                            plan.settled_at.to_wire(),
                            plan.generation
                        )),
                    )
                    .build();
                let put = Put::builder()
                    .table_name(&self.table)
                    .set_item(Some(item))
                    .condition_expression("attribute_not_exists(pk)")
                    .build()
                    .map_err(|error| RuntimeStoreError::Malformed {
                        reason: error.to_string(),
                    })?;
                transaction.push(TransactWriteItem::builder().put(put).build());
            }
            self.transact(
                transaction,
                transaction_token("settle", plan.generation, &plan.intent_id.0),
            )
            .await?;
            Ok(CanonicalReceipt {
                intent,
                receipt_id,
                snapshot: plan.snapshot.clone(),
            })
        })
    }

    fn load_usage_outbox(
        &self,
        generation: GenerationId,
    ) -> StoreFuture<'_, Vec<UsageOutboxEntry>> {
        Box::pin(async move {
            let mut exclusive_start_key = None;
            let mut pending = Vec::new();
            loop {
                let output = self
                    .client
                    .query()
                    .table_name(&self.table)
                    .key_condition_expression("pk = :pk AND begins_with(sk, :prefix)")
                    .expression_attribute_values(
                        ":pk",
                        s(keys::generation_partition_for_id(generation)),
                    )
                    .expression_attribute_values(":prefix", s("USAGE#"))
                    .consistent_read(true)
                    .set_exclusive_start_key(exclusive_start_key)
                    .send()
                    .await
                    .map_err(|error| runtime_error(classify(&error, Idempotence::Read), None))?;
                for item in output.items() {
                    let text = |name: &str| item.get(name).and_then(|value| value.as_s().ok());
                    if text("itemType").map(String::as_str) != Some("usage_outbox") {
                        return Err(malformed("USAGE# row is not a usage_outbox item"));
                    }
                    let category = text("category")
                        .ok_or_else(|| malformed("usage outbox row has no category"))
                        .and_then(|value| {
                            Category::from_str(value).map_err(|error| malformed(&error.to_string()))
                        })?;
                    let draft: FactDraft = serde_json::from_str(
                        text("draft").ok_or_else(|| malformed("usage outbox row has no draft"))?,
                    )
                    .map_err(|error| malformed(&error.to_string()))?;
                    if draft.authority.category != category {
                        return Err(malformed(
                            "usage outbox category disagrees with its draft authority",
                        ));
                    }
                    pending.push(UsageOutboxEntry {
                        generation,
                        category,
                        draft,
                    });
                }
                exclusive_start_key = output.last_evaluated_key().cloned();
                if exclusive_start_key.is_none() {
                    break;
                }
            }
            pending.sort_by_key(|entry| entry.draft.fact_id().to_string());
            Ok(pending)
        })
    }

    fn mark_usage_emitted<'a>(
        &'a self,
        generation: GenerationId,
        draft: &'a FactDraft,
    ) -> StoreFuture<'a, ()> {
        Box::pin(async move {
            let fact_id = draft.fact_id().to_string();
            let target = keys::usage_outbox(generation, &fact_id);
            self.client
                .delete_item()
                .table_name(&self.table)
                .set_key(Some(key(&target.pk, &target.sk)))
                .condition_expression("attribute_not_exists(pk) OR factId = :factId")
                .expression_attribute_values(":factId", s(fact_id))
                .return_values_on_condition_check_failure(
                    ReturnValuesOnConditionCheckFailure::AllOld,
                )
                .send()
                .await
                .map_err(|error| {
                    runtime_error(
                        classify(&error, Idempotence::Write(Resolution::TargetItem)),
                        None,
                    )
                })?;
            Ok(())
        })
    }

    fn record_probe<'a>(&'a self, probe: &'a CanonicalIdleProbe) -> StoreFuture<'a, ()> {
        Box::pin(async move {
            let row = self.load_canonical_row(probe.generation).await?.ok_or(
                RuntimeStoreError::NoSuchGeneration {
                    generation: probe.generation,
                },
            )?;
            let legacy = IdleProbe {
                session: row.session,
                workspace: row.workspace,
                generation: probe.generation,
                observed_at: probe.assessment.evidence.observed_at,
                open_operations: probe.assessment.evidence.open,
                queued_operations: probe.assessment.evidence.queued,
                admitted_operations: probe.assessment.evidence.admitted,
                keepalive_lease_until: probe
                    .assessment
                    .evidence
                    .keepalive_lease
                    .as_ref()
                    .map(|lease| lease.expires_at),
            };
            let put = expressions::record_probe(&self.table, &legacy)
                .map_err(|error| runtime_error(error, None))?
                .build()
                .map_err(|error| RuntimeStoreError::Malformed {
                    reason: error.to_string(),
                })?;
            let target = keys::head_for_generation(probe.generation);
            // The probe always rearms the due index: a generation that holds
            // provider compute never leaves it, because the index is also what
            // enforces the lifetime and re-examines a stale busy count.
            let update = "SET idleSince = :idleSince, nextEvaluateAt = :next, rtDueSk = :dueSk, revision = :nextRevision";
            let mut builder = Update::builder()
                .expression_attribute_values(":next", stamp(probe.next_evaluate_at))
                .expression_attribute_values(
                    ":dueSk",
                    s(keys::due_sort(probe.next_evaluate_at, probe.generation)),
                );
            let idle_since = probe
                .assessment
                .idle_since()
                .unwrap_or(probe.assessment.last_busy_at);
            builder = builder
                .table_name(&self.table)
                .set_key(Some(key(&target.pk, &target.sk)))
                .condition_expression("revision = :revision")
                .update_expression(update)
                .expression_attribute_values(":revision", n(probe.expected_revision.value()))
                .expression_attribute_values(
                    ":nextRevision",
                    n(probe.expected_revision.next().value()),
                )
                .expression_attribute_values(":idleSince", stamp(idle_since));
            let update = builder
                .build()
                .map_err(|error| RuntimeStoreError::Malformed {
                    reason: error.to_string(),
                })?;
            self.transact(
                vec![
                    TransactWriteItem::builder().put(put).build(),
                    TransactWriteItem::builder().update(update).build(),
                ],
                transaction_token(
                    "probe",
                    probe.generation,
                    &probe.assessment.evidence.observed_at.to_wire(),
                ),
            )
            .await
        })
    }

    fn repair_open_operations<'a>(&'a self, plan: &'a OpenCountRepairPlan) -> StoreFuture<'a, ()> {
        Box::pin(async move {
            let target = keys::head_for_generation(plan.generation);
            // A repair to zero starts the idle clock at the repair instant —
            // nothing is known about when the leak actually drained, so the
            // conservative claim is "idle since now". Any other value keeps
            // the busy evidence and clears a stale idle mark.
            let update = if plan.open_operations == 0 {
                "SET openOperations = :open, revision = :nextRevision, idleSince = :at, \
                 updatedAt = :at, nextEvaluateAt = :next, rtDueSk = :dueSk"
            } else {
                "SET openOperations = :open, revision = :nextRevision, updatedAt = :at, \
                 nextEvaluateAt = :next, rtDueSk = :dueSk REMOVE idleSince"
            };
            let builder = Update::builder()
                .table_name(&self.table)
                .set_key(Some(key(&target.pk, &target.sk)))
                .condition_expression("generationId = :generation AND revision = :revision")
                .update_expression(update)
                .expression_attribute_values(":generation", s(plan.generation.to_string()))
                .expression_attribute_values(":revision", n(plan.expected_revision.value()))
                .expression_attribute_values(
                    ":nextRevision",
                    n(plan.expected_revision.next().value()),
                )
                .expression_attribute_values(":open", n(u64::from(plan.open_operations)))
                .expression_attribute_values(":at", stamp(plan.at))
                .expression_attribute_values(":next", stamp(plan.next_evaluate_at))
                .expression_attribute_values(
                    ":dueSk",
                    s(keys::due_sort(plan.next_evaluate_at, plan.generation)),
                );
            self.conditional_update(builder, Participant::RUNTIME_GENERATION)
                .await
                .map_err(|error| runtime_error(error, Some(plan.expected_revision)))
        })
    }

    fn scan_due(
        &self,
        shard: RuntimeShard,
        now: Timestamp,
        budget: CanonicalPageBudget,
    ) -> StoreFuture<'_, RuntimeDuePage> {
        Box::pin(async move {
            let shard = u8::try_from(shard.0).map_err(|_| RuntimeStoreError::Malformed {
                reason: format!(
                    "runtime shard {} does not fit the table vocabulary",
                    shard.0
                ),
            })?;
            let limit = budget.max_items.min(budget.max_reads).max(1);
            let output = self
                .client
                .query()
                .table_name(&self.table)
                .index_name(keys::DUE_INDEX)
                .key_condition_expression("#pk = :pk AND #sk <= :now")
                .expression_attribute_names("#pk", keys::DUE_PK)
                .expression_attribute_names("#sk", keys::DUE_SK)
                .expression_attribute_values(":pk", s(keys::due_partition_for_shard(shard)))
                .expression_attribute_values(":now", s(format!("{}#", now.to_wire())))
                .limit(i32::try_from(limit).unwrap_or(i32::MAX))
                .send()
                .await
                .map_err(|error| runtime_error(classify(&error, Idempotence::Read), None))?;
            let mut due = Vec::new();
            for item in output.items() {
                let text = |name: &str| item.get(name).and_then(|value| value.as_s().ok());
                let session = text("sessionId")
                    .ok_or_else(|| malformed("due row has no sessionId"))
                    .and_then(|value| {
                        SessionId::parse(value).map_err(|error| malformed(&error.to_string()))
                    })?;
                let generation = text("generationId")
                    .ok_or_else(|| malformed("due row has no generationId"))
                    .and_then(|value| {
                        GenerationId::parse(value).map_err(|error| malformed(&error.to_string()))
                    })?;
                let fence = item
                    .get("fence")
                    .and_then(|value| value.as_n().ok())
                    .and_then(|value| value.parse().ok())
                    .ok_or_else(|| malformed("due row has no numeric fence"))?;
                let revision = item
                    .get("revision")
                    .and_then(|value| value.as_n().ok())
                    .and_then(|value| value.parse().ok())
                    .ok_or_else(|| malformed("due row has no numeric revision"))?;
                due.push(GenerationPointer {
                    session,
                    generation,
                    fence: Fence(fence),
                    revision: Revision::new(revision),
                });
            }
            Ok(RuntimeDuePage {
                due,
                cursor: output
                    .last_evaluated_key()
                    .and_then(|key| key.get(keys::DUE_SK))
                    .and_then(|value| value.as_s().ok())
                    .cloned(),
            })
        })
    }
}

fn generation_view(row: GenerationRow) -> GenerationView {
    GenerationView {
        definition: row.definition,
        head: GenerationHead {
            generation: row.generation,
            size: row.size,
            state: row.state,
            fence: row.fence,
            revision: row.revision,
            open_operations: row.open_operations,
            last_busy_at: row.last_busy_at,
            idle_since: row.idle_since,
            suspend_lock_expires_at: row.suspend_lock_expires_at,
            keepalive_lease: row.keepalive_lease,
            transport_mode: row.transport_mode,
        },
        session: row.session,
        workspace: row.workspace,
        organization: row.organization,
        microvm: row.microvm.or_else(|| row.provider_vm_id.map(MicrovmId)),
        lifetime: row.lifetime,
        accounted_from: row.accounted_from,
        open_intent: row.open_intent,
        suspended_at: row.suspended_at,
        snapshot_ordinal: row.snapshot_ordinal,
        snapshot_bytes: row.snapshot_bytes,
    }
}

fn runtime_error(error: StoreError, expected: Option<Revision>) -> RuntimeStoreError {
    match error {
        StoreError::PreconditionFailed { observed, .. } => RuntimeStoreError::RevisionConflict {
            expected: expected.unwrap_or(Revision::ZERO),
            found: observed
                .as_deref()
                .and_then(|item| item.get("revision"))
                .and_then(|value| value.as_n().ok())
                .and_then(|value| value.parse().ok())
                .map(Revision::new),
        },
        StoreError::Invalid { detail } => RuntimeStoreError::Malformed { reason: detail },
        StoreError::Corrupt(error) => RuntimeStoreError::Malformed {
            reason: error.to_string(),
        },
        StoreError::Key(error) => RuntimeStoreError::Malformed {
            reason: error.to_string(),
        },
        other => RuntimeStoreError::Unavailable {
            reason: other.to_string(),
        },
    }
}

fn malformed(reason: &str) -> RuntimeStoreError {
    RuntimeStoreError::Malformed {
        reason: reason.to_owned(),
    }
}

const fn action_str(action: aex_runtime_control::lifecycle::LifecycleAction) -> &'static str {
    match action {
        aex_runtime_control::lifecycle::LifecycleAction::Launch => "launch",
        aex_runtime_control::lifecycle::LifecycleAction::Suspend => "suspend",
        aex_runtime_control::lifecycle::LifecycleAction::Resume => "resume",
        aex_runtime_control::lifecycle::LifecycleAction::Terminate => "terminate",
    }
}

const fn intent_state_str(state: IntentState) -> &'static str {
    match state {
        IntentState::Dispatched => "dispatched",
        IntentState::Unknown => "unknown",
        IntentState::Settled => "settled",
        IntentState::Quarantined => "quarantined",
    }
}

const fn transport_mode_str(mode: TransportMode) -> &'static str {
    match mode {
        TransportMode::Multiplexed => "multiplexed",
        TransportMode::PerRequest => "per_request",
    }
}

fn transaction_token(kind: &str, generation: GenerationId, intent: &str) -> String {
    blake3::hash(format!("{kind}:{generation}:{intent}").as_bytes())
        .to_hex()
        .as_str()[..32]
        .to_owned()
}

impl RuntimeActivityDynamoStore {
    #[doc(hidden)]
    pub async fn load_current(
        &self,
        session: SessionId,
    ) -> Result<Option<CurrentGeneration>, StoreError> {
        let target = keys::current(session);
        match self.get(&target.pk, &target.sk).await? {
            None => Ok(None),
            Some(item) => Ok(Some(codec::decode_current(&item)?)),
        }
    }

    #[doc(hidden)]
    pub async fn load_generation(
        &self,
        workspace: WorkspaceId,
        session: SessionId,
        generation: GenerationId,
    ) -> Result<Option<GenerationRow>, StoreError> {
        let target = keys::head(session, generation);
        match self.get(&target.pk, &target.sk).await? {
            None => Ok(None),
            Some(item) => Ok(Some(codec::decode_generation(&item, workspace)?)),
        }
    }

    #[doc(hidden)]
    pub async fn create_generation(&self, row: &GenerationRow) -> Result<(), StoreError> {
        self.conditional_put(
            expressions::create_generation(&self.table, row)?,
            Participant::RUNTIME_GENERATION,
        )
        .await
    }

    #[doc(hidden)]
    pub async fn transition(
        &self,
        row: &GenerationRow,
        from_state: GenerationState,
        from_fence: Fence,
        from_revision: Revision,
        now: Timestamp,
    ) -> Result<(), StoreError> {
        self.conditional_update(
            expressions::transition(&self.table, row, from_state, from_fence, from_revision, now)?,
            Participant::RUNTIME_GENERATION,
        )
        .await
    }

    #[doc(hidden)]
    pub async fn record_intent(&self, intent: &LifecycleIntent) -> Result<(), StoreError> {
        self.conditional_put(
            expressions::record_intent(&self.table, intent)?,
            Participant::RUNTIME_INTENT,
        )
        .await
    }

    #[doc(hidden)]
    pub async fn settle_intent(&self, receipt: &LifecycleReceipt) -> Result<(), StoreError> {
        self.conditional_put(
            expressions::settle_intent(&self.table, receipt)?,
            Participant::RUNTIME_RECEIPT,
        )
        .await
    }

    #[doc(hidden)]
    pub async fn record_probe(&self, probe: &IdleProbe) -> Result<(), StoreError> {
        self.conditional_put(
            expressions::record_probe(&self.table, probe)?,
            Participant::RUNTIME_GENERATION,
        )
        .await
    }

    #[doc(hidden)]
    pub async fn point_current(
        &self,
        session: SessionId,
        generation: GenerationId,
        fence: Fence,
        from_revision: Option<Revision>,
        now: Timestamp,
    ) -> Result<(), StoreError> {
        self.conditional_update(
            expressions::point_current(
                &self.table,
                session,
                generation,
                fence,
                from_revision,
                now,
            )?,
            Participant::RUNTIME_CURRENT,
        )
        .await
    }

    #[doc(hidden)]
    pub async fn scan_due(
        &self,
        shard: u8,
        now: Timestamp,
        budget: PageBudget,
    ) -> Result<Vec<DueGeneration>, StoreError> {
        let output = self
            .client
            .query()
            .table_name(&self.table)
            .index_name(keys::DUE_INDEX)
            .key_condition_expression("#pk = :pk AND #sk <= :now")
            .expression_attribute_names("#pk", keys::DUE_PK)
            .expression_attribute_names("#sk", keys::DUE_SK)
            .expression_attribute_values(":pk", s(keys::due_partition_for_shard(shard)))
            // The upper bound carries the separator so the whole of that
            // millisecond is included whatever generation follows it.
            .expression_attribute_values(":now", s(format!("{}#", now.to_wire())))
            .limit(budget.limit())
            .send()
            .await
            .map_err(|error| classify(&error, Idempotence::Read))?;

        let mut due = Vec::new();
        for item in output.items.unwrap_or_default() {
            // An `INCLUDE` projection carries the key attributes plus the
            // declared list and **nothing else** — not even `itemType`. Binding
            // the typed row reader here would therefore fail on every row, so
            // the projection is decoded as what it is: a slim, explicitly named
            // set of attributes.
            let text = |name: &str| item.get(name).and_then(|value| value.as_s().ok()).cloned();
            let stamp = |name: &str| text(name).and_then(|value| Timestamp::parse(&value).ok());
            let identity = |name: &str| -> Result<String, StoreError> {
                text(name).ok_or_else(|| StoreError::Invalid {
                    detail: format!("a due row carries no `{name}`"),
                })
            };
            let session_id = identity("sessionId")?;
            let generation_id = identity("generationId")?;
            let state_text = identity("state")?;
            due.push(DueGeneration {
                session: SessionId::parse(&session_id).map_err(|error| StoreError::Invalid {
                    detail: error.to_string(),
                })?,
                generation: GenerationId::parse(&generation_id).map_err(|error| {
                    StoreError::Invalid {
                        detail: error.to_string(),
                    }
                })?,
                state: keys::state_of(&state_text).ok_or(StoreError::Invalid {
                    detail: "a due row named a state outside the vocabulary".to_owned(),
                })?,
                fence: Fence(
                    item.get("fence")
                        .and_then(|value| value.as_n().ok())
                        .and_then(|text| text.parse::<u64>().ok())
                        .ok_or(StoreError::Invalid {
                            detail: "a due row carries no fence".to_owned(),
                        })?,
                ),
                idle_since: stamp("idleSince"),
                provider_lifetime_expires_at: stamp("providerLifetimeExpiresAt"),
                keepalive_lease_until: stamp("keepaliveLeaseUntil"),
            });
        }
        Ok(due)
    }
}
