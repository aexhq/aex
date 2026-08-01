//! The `runtime-activity` port implementation.
//!
//! The reaper's scan reads the slim due-index projection, so a sweep for
//! generations that need evaluating never pulls a whole head back — and the
//! projection has no attribute that could carry customer content in the first
//! place, which is what makes this table's `NEW_IMAGE` stream safe.

use aex_hands_protocol::rpc::Fence;
use aex_runtime_control::generation::{GenerationState, Revision};
use aex_session_dynamodb::attr::{Item, s};
use aex_session_dynamodb::error::{Idempotence, Resolution, StoreError, classify};
use aex_session_dynamodb::paging::PageBudget;
use aex_session_dynamodb::plan::{Participant, key};
use aex_wire::ids::{GenerationId, PrefixedId, SessionId, WorkspaceId};
use aex_wire::types::Timestamp;
use async_trait::async_trait;
use aws_sdk_dynamodb::Client;
use aws_sdk_dynamodb::types::ReturnValuesOnConditionCheckFailure;
use aws_sdk_dynamodb::types::builders::{PutBuilder, UpdateBuilder};

use crate::codec::{
    self, CurrentGeneration, GenerationRow, IdleProbe, LifecycleIntent, LifecycleReceipt,
};
use crate::{expressions, keys};

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

// TODO(cross-stream): replaced by aex_runtime_control::ports::RuntimeActivityStore
/// The `runtime-activity` authority.
#[async_trait]
pub trait RuntimeActivityStore: Send + Sync + 'static {
    /// Resolves the exact generation a session currently points at.
    ///
    /// # Errors
    ///
    /// [`StoreError`] for a transport or decode failure.
    async fn load_current(
        &self,
        session: SessionId,
    ) -> Result<Option<CurrentGeneration>, StoreError>;

    /// Reads one generation head.
    ///
    /// # Errors
    ///
    /// As [`RuntimeActivityStore::load_current`].
    async fn load_generation(
        &self,
        workspace: WorkspaceId,
        session: SessionId,
        generation: GenerationId,
    ) -> Result<Option<GenerationRow>, StoreError>;

    /// Writes a generation head that must not already exist.
    ///
    /// # Errors
    ///
    /// [`StoreError::PreconditionFailed`] when the identity already exists.
    async fn create_generation(&self, row: &GenerationRow) -> Result<(), StoreError>;

    /// Moves a generation under its exact fence and revision.
    ///
    /// # Errors
    ///
    /// [`StoreError::PreconditionFailed`] when a newer fence, revision or state
    /// won. The caller **discards** its result rather than retrying: a superseded
    /// lifecycle reply must not revive a generation the system moved past.
    async fn transition(
        &self,
        row: &GenerationRow,
        from_state: GenerationState,
        from_fence: Fence,
        from_revision: Revision,
        now: Timestamp,
    ) -> Result<(), StoreError>;

    /// Records a lifecycle intent.
    ///
    /// # Errors
    ///
    /// As [`RuntimeActivityStore::create_generation`].
    async fn record_intent(&self, intent: &LifecycleIntent) -> Result<(), StoreError>;

    /// Settles a lifecycle intent with an immutable receipt.
    ///
    /// # Errors
    ///
    /// [`StoreError::PreconditionFailed`] when the intent already settled, which
    /// is an idempotent replay rather than a second outcome.
    async fn settle_intent(&self, receipt: &LifecycleReceipt) -> Result<(), StoreError>;

    /// Records one true-idle probe.
    ///
    /// # Errors
    ///
    /// As [`RuntimeActivityStore::create_generation`].
    async fn record_probe(&self, probe: &IdleProbe) -> Result<(), StoreError>;

    /// Points a session at a generation.
    ///
    /// # Errors
    ///
    /// [`StoreError::PreconditionFailed`] when another writer moved the pointer.
    async fn point_current(
        &self,
        session: SessionId,
        generation: GenerationId,
        fence: Fence,
        from_revision: Option<Revision>,
        now: Timestamp,
    ) -> Result<(), StoreError>;

    /// Scans one shard for generations due for evaluation.
    ///
    /// # Errors
    ///
    /// As [`RuntimeActivityStore::load_current`].
    async fn scan_due(
        &self,
        shard: u8,
        now: Timestamp,
        budget: PageBudget,
    ) -> Result<Vec<DueGeneration>, StoreError>;
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

#[async_trait]
impl RuntimeActivityStore for RuntimeActivityDynamoStore {
    async fn load_current(
        &self,
        session: SessionId,
    ) -> Result<Option<CurrentGeneration>, StoreError> {
        let target = keys::current(session);
        match self.get(&target.pk, &target.sk).await? {
            None => Ok(None),
            Some(item) => Ok(Some(codec::decode_current(&item)?)),
        }
    }

    async fn load_generation(
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

    async fn create_generation(&self, row: &GenerationRow) -> Result<(), StoreError> {
        self.conditional_put(
            expressions::create_generation(&self.table, row)?,
            Participant::RUNTIME_GENERATION,
        )
        .await
    }

    async fn transition(
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

    async fn record_intent(&self, intent: &LifecycleIntent) -> Result<(), StoreError> {
        self.conditional_put(
            expressions::record_intent(&self.table, intent)?,
            Participant::RUNTIME_INTENT,
        )
        .await
    }

    async fn settle_intent(&self, receipt: &LifecycleReceipt) -> Result<(), StoreError> {
        self.conditional_put(
            expressions::settle_intent(&self.table, receipt)?,
            Participant::RUNTIME_RECEIPT,
        )
        .await
    }

    async fn record_probe(&self, probe: &IdleProbe) -> Result<(), StoreError> {
        self.conditional_put(
            expressions::record_probe(&self.table, probe)?,
            Participant::RUNTIME_GENERATION,
        )
        .await
    }

    async fn point_current(
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

    async fn scan_due(
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
