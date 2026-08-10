//! The `regional-registry` port implementation.
//!
//! List-by-kind is a native `Query` over one partition with native pagination,
//! which is why this table carries no index at all (D-22). The previous
//! implementation kept a `workspace-created-index` for the same query and a
//! literal single `"registry-upload-expiry"` due partition beside it; both are
//! gone, and upload expiry rides the sharded `regional-work` due index instead.

use aex_content_domain::identity::{RegistryKind, Revision};
use aex_session_dynamodb::attr::{Item, s};
use aex_session_dynamodb::error::{Idempotence, Resolution, StoreError, classify};
use aex_session_dynamodb::paging::{PageBudget, PagePosition};
use aex_session_dynamodb::plan::{Participant, key};
use aex_session_dynamodb::replay::{IdempotencyScope, Receipt, ReceiptStore, key_digest};
use aex_wire::idempotency::IdempotencyKey;
use aex_wire::ids::{UploadId, WorkspaceId};
use aex_wire::types::Timestamp;
use aex_workspace_domain::registry::RegistryPointer;
use aex_workspace_domain::upload::{Upload, UploadState};
use async_trait::async_trait;
use aws_sdk_dynamodb::Client;
use aws_sdk_dynamodb::types::builders::{DeleteBuilder, PutBuilder, UpdateBuilder};
use aws_sdk_dynamodb::types::{AttributeValue, ReturnValuesOnConditionCheckFailure};

use crate::{codec, expressions, keys};

/// One page of a registry listing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PointerPage {
    /// The pointers this page names, in name order.
    pub pointers: Vec<RegistryPointer>,
    /// Where a continuation resumes, when there is more.
    pub next: Option<PagePosition>,
}

// TODO(cross-stream): `aex-workspace-domain` has no `ports` module and publishes no
// traits. Its registry vocabulary is data only — `aex_workspace_domain::registry`'s
// `RegistryPointer`, `ProposedValue`, `SetOutcome`, `RegistryCommit` and
// `DeleteCommit` — so the trait below has no peer to be replaced by.
/// The `regional-registry` authority.
#[async_trait]
pub trait RegistryStore: Send + Sync + 'static {
    /// Reads one current pointer.
    ///
    /// # Errors
    ///
    /// [`StoreError`] for a transport or decode failure.
    async fn load_pointer(
        &self,
        workspace: WorkspaceId,
        kind: RegistryKind,
        name: &str,
    ) -> Result<Option<RegistryPointer>, StoreError>;

    /// Lists one `(workspace, kind)` in name order.
    ///
    /// # Errors
    ///
    /// As [`RegistryStore::load_pointer`].
    async fn list_pointers(
        &self,
        workspace: WorkspaceId,
        kind: RegistryKind,
        budget: PageBudget,
        from: Option<&PagePosition>,
    ) -> Result<PointerPage, StoreError>;

    /// Writes one pointer, creating it or replacing it under the revision the
    /// caller observed.
    ///
    /// # Errors
    ///
    /// [`StoreError::PreconditionFailed`] naming `registry.pointer` when another
    /// writer moved the revision, which the caller surfaces as `412`.
    async fn put_pointer(
        &self,
        pointer: &RegistryPointer,
        from_revision: Option<Revision>,
    ) -> Result<(), StoreError>;

    /// Reads one staged upload, including every spilled part block.
    ///
    /// # Errors
    ///
    /// As [`RegistryStore::load_pointer`].
    async fn load_upload(
        &self,
        workspace: WorkspaceId,
        upload: UploadId,
    ) -> Result<Option<Upload>, StoreError>;

    /// Stages one upload.
    ///
    /// # Errors
    ///
    /// [`StoreError::PreconditionFailed`] when the identity already exists,
    /// which under `UUIDv7` means a duplicated request.
    async fn create_upload(&self, upload: &Upload) -> Result<(), StoreError>;

    /// Applies one upload transition.
    ///
    /// # Errors
    ///
    /// [`StoreError::PreconditionFailed`] naming `registry.upload` when the
    /// upload was not in the state the transition names.
    async fn transition_upload(
        &self,
        upload: UploadId,
        from: UploadState,
        to: UploadState,
    ) -> Result<(), StoreError>;

    /// Applies one upload transition after the provider effect it records.
    ///
    /// Fenced on `providerUploadId` as well as the state, so a worker whose
    /// decision is stale cannot terminalise a re-created upload (E D-6).
    ///
    /// # Errors
    ///
    /// As [`RegistryStore::transition_upload`].
    async fn transition_upload_fenced(
        &self,
        upload: UploadId,
        from: UploadState,
        to: UploadState,
        provider_upload_id: &str,
    ) -> Result<(), StoreError>;

    /// Settles the per-part digests one grant call declared.
    ///
    /// # Errors
    ///
    /// As [`RegistryStore::transition_upload`].
    async fn record_part_declarations(&self, upload: &Upload) -> Result<(), StoreError>;

    /// Settles an upload as `Ready`, writing the evidence that proves its object
    /// exists (E D-1, E D-3).
    ///
    /// # Errors
    ///
    /// As [`RegistryStore::transition_upload`], plus [`StoreError::Invalid`] when
    /// the upload carries no completion evidence.
    async fn settle_ready(&self, upload: &Upload) -> Result<(), StoreError>;

    /// Removes an upload row, and every part block it spilled, under the
    /// terminal state the sweep observed (E D-5).
    ///
    /// An already-absent row is an idempotent success: the sweep's job is that
    /// the row is gone, not that this call is the one that removed it.
    ///
    /// # Errors
    ///
    /// [`StoreError::PreconditionFailed`] naming `registry.upload` when the row
    /// moved on since the sweep decided.
    async fn delete_upload(&self, upload: &Upload) -> Result<(), StoreError>;

    /// Begins a completion under a manifest identity.
    ///
    /// # Errors
    ///
    /// As [`RegistryStore::transition_upload`]; a retry with a different
    /// manifest loses the `completionIntentHash` condition.
    async fn begin_completion(
        &self,
        upload: UploadId,
        completion_intent_hash: &str,
    ) -> Result<(), StoreError>;

    /// Finishes a completion under the manifest it began with.
    ///
    /// # Errors
    ///
    /// As [`RegistryStore::begin_completion`].
    async fn finish_completion(
        &self,
        upload: UploadId,
        completion_intent_hash: &str,
    ) -> Result<(), StoreError>;
}

/// The adapter.
#[derive(Debug, Clone)]
pub struct RegistryDynamoStore {
    client: Client,
    table: String,
}

impl RegistryDynamoStore {
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
            .set_expression_attribute_names(built.expression_attribute_names().cloned())
            .set_expression_attribute_values(built.expression_attribute_values().cloned())
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

    /// Reads every spilled part block of one upload, in block order.
    async fn upload_part_blocks(&self, upload: UploadId) -> Result<Vec<Item>, StoreError> {
        let target = keys::upload(upload);
        let mut blocks = Vec::new();
        let mut start: Option<std::collections::HashMap<String, AttributeValue>> = None;
        loop {
            let output = self
                .client
                .query()
                .table_name(&self.table)
                .key_condition_expression("#pk = :pk AND begins_with(#sk, :prefix)")
                .expression_attribute_names("#pk", aex_session_dynamodb::attr::PK)
                .expression_attribute_names("#sk", aex_session_dynamodb::attr::SK)
                .expression_attribute_values(":pk", s(target.pk.clone()))
                .expression_attribute_values(":prefix", s(keys::upload_parts_prefix()))
                .consistent_read(true)
                .set_exclusive_start_key(start.clone())
                .send()
                .await
                .map_err(|error| classify(&error, Idempotence::Read))?;
            blocks.extend(output.items.unwrap_or_default());
            match output.last_evaluated_key {
                None => break,
                Some(position) => start = Some(position),
            }
        }
        Ok(blocks)
    }

    async fn conditional_delete(
        &self,
        builder: DeleteBuilder,
        participant: Participant,
    ) -> Result<(), StoreError> {
        let built = builder.build().map_err(|error| StoreError::Invalid {
            detail: error.to_string(),
        })?;
        let outcome = self
            .client
            .delete_item()
            .table_name(&self.table)
            .set_key(Some(built.key().clone()))
            .set_condition_expression(built.condition_expression().map(str::to_owned))
            .set_expression_attribute_names(built.expression_attribute_names().cloned())
            .set_expression_attribute_values(built.expression_attribute_values().cloned())
            .send()
            .await;
        match outcome {
            Ok(_) => Ok(()),
            Err(error) => {
                if let Some(
                    aws_sdk_dynamodb::operation::delete_item::DeleteItemError::ConditionalCheckFailedException(_),
                ) = error.as_service_error()
                {
                    return Err(StoreError::PreconditionFailed {
                        participant,
                        observed: None,
                    });
                }
                Err(classify(&error, Idempotence::Write(Resolution::TargetItem)))
            }
        }
    }

    async fn unconditional_delete(&self, builder: DeleteBuilder) -> Result<(), StoreError> {
        let built = builder.build().map_err(|error| StoreError::Invalid {
            detail: error.to_string(),
        })?;
        self.client
            .delete_item()
            .table_name(&self.table)
            .set_key(Some(built.key().clone()))
            .send()
            .await
            .map_err(|error| classify(&error, Idempotence::Write(Resolution::TargetItem)))?;
        Ok(())
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
impl RegistryStore for RegistryDynamoStore {
    async fn load_pointer(
        &self,
        workspace: WorkspaceId,
        kind: RegistryKind,
        name: &str,
    ) -> Result<Option<RegistryPointer>, StoreError> {
        let target = keys::pointer(workspace, kind, name)?;
        match self.get(&target.pk, &target.sk).await? {
            None => Ok(None),
            Some(item) => Ok(Some(codec::decode_pointer(&item, workspace)?)),
        }
    }

    async fn list_pointers(
        &self,
        workspace: WorkspaceId,
        kind: RegistryKind,
        budget: PageBudget,
        from: Option<&PagePosition>,
    ) -> Result<PointerPage, StoreError> {
        let output = self
            .client
            .query()
            .table_name(&self.table)
            .key_condition_expression("#pk = :pk AND begins_with(#sk, :prefix)")
            .expression_attribute_names("#pk", aex_session_dynamodb::attr::PK)
            .expression_attribute_names("#sk", aex_session_dynamodb::attr::SK)
            .expression_attribute_values(":pk", s(keys::kind_partition(workspace, kind)))
            .expression_attribute_values(":prefix", s(keys::name_prefix()))
            .set_exclusive_start_key(from.map(|position| position.to_exclusive_start(None, None)))
            .limit(budget.limit())
            .send()
            .await
            .map_err(|error| classify(&error, Idempotence::Read))?;

        let mut pointers = Vec::new();
        for item in output.items.unwrap_or_default() {
            pointers.push(codec::decode_pointer(&item, workspace)?);
        }
        let next = match output.last_evaluated_key {
            None => None,
            Some(position) => Some(
                PagePosition::from_last_evaluated(&position, None, None).map_err(|error| {
                    StoreError::Invalid {
                        detail: error.to_string(),
                    }
                })?,
            ),
        };
        Ok(PointerPage { pointers, next })
    }

    async fn put_pointer(
        &self,
        pointer: &RegistryPointer,
        from_revision: Option<Revision>,
    ) -> Result<(), StoreError> {
        let builder = match from_revision {
            None => expressions::create_pointer(&self.table, pointer)?,
            Some(revision) => expressions::replace_pointer(&self.table, pointer, revision)?,
        };
        self.conditional_put(builder, Participant::REGISTRY_POINTER)
            .await
    }

    async fn load_upload(
        &self,
        workspace: WorkspaceId,
        upload: UploadId,
    ) -> Result<Option<Upload>, StoreError> {
        let target = keys::upload(upload);
        let Some(head) = self.get(&target.pk, &target.sk).await? else {
            return Ok(None);
        };
        // A spilled upload is read as one strongly consistent `Query` over its
        // own partition. Decoding the head alone would produce a short part plan
        // that looks complete, so the codec refuses it rather than allowing it.
        let blocks = if head
            .get("partBlockCount")
            .and_then(|value| value.as_n().ok())
            .is_some_and(|count| count != "0")
        {
            self.upload_part_blocks(upload).await?
        } else {
            Vec::new()
        };
        Ok(Some(codec::decode_upload_blocks(&head, &blocks, workspace)?))
    }

    async fn create_upload(&self, upload: &Upload) -> Result<(), StoreError> {
        for block in expressions::create_upload_part_blocks(&self.table, upload) {
            // Part blocks are written before the head, so a head row is never
            // visible without the parts it names. The head write is the
            // conditional one, and it is what makes the upload exist.
            self.conditional_put(block, Participant::REGISTRY_UPLOAD)
                .await?;
        }
        self.conditional_put(
            expressions::create_upload(&self.table, upload),
            Participant::REGISTRY_UPLOAD,
        )
        .await
    }

    async fn transition_upload(
        &self,
        upload: UploadId,
        from: UploadState,
        to: UploadState,
    ) -> Result<(), StoreError> {
        self.conditional_update(
            expressions::transition_upload(&self.table, upload, from, to),
            Participant::REGISTRY_UPLOAD,
        )
        .await
    }

    async fn transition_upload_fenced(
        &self,
        upload: UploadId,
        from: UploadState,
        to: UploadState,
        provider_upload_id: &str,
    ) -> Result<(), StoreError> {
        self.conditional_update(
            expressions::transition_upload_fenced(
                &self.table,
                upload,
                from,
                to,
                provider_upload_id,
            ),
            Participant::REGISTRY_UPLOAD,
        )
        .await
    }

    async fn record_part_declarations(&self, upload: &Upload) -> Result<(), StoreError> {
        self.conditional_update(
            expressions::record_part_declarations(&self.table, upload),
            Participant::REGISTRY_UPLOAD,
        )
        .await
    }

    async fn settle_ready(&self, upload: &Upload) -> Result<(), StoreError> {
        self.conditional_update(
            expressions::settle_ready(&self.table, upload)?,
            Participant::REGISTRY_UPLOAD,
        )
        .await
    }

    async fn delete_upload(&self, upload: &Upload) -> Result<(), StoreError> {
        let blocks = upload
            .parts
            .parts
            .len()
            .div_ceil(codec::PARTS_PER_BLOCK.max(1));
        if upload.parts.parts.len() > codec::PARTS_PER_BLOCK {
            for block in 0..blocks {
                self.unconditional_delete(expressions::delete_upload_part_block(
                    &self.table,
                    upload.id,
                    block,
                ))
                .await?;
            }
        }
        match self
            .conditional_delete(
                expressions::delete_upload(&self.table, upload.id, upload.state),
                Participant::REGISTRY_UPLOAD,
            )
            .await
        {
            Ok(()) => Ok(()),
            // The row is already gone. The sweep's goal is that it is absent.
            Err(StoreError::PreconditionFailed { .. })
                if self
                    .get(&keys::upload(upload.id).pk, &keys::upload(upload.id).sk)
                    .await?
                    .is_none() =>
            {
                Ok(())
            }
            Err(error) => Err(error),
        }
    }

    async fn begin_completion(
        &self,
        upload: UploadId,
        completion_intent_hash: &str,
    ) -> Result<(), StoreError> {
        self.conditional_update(
            expressions::begin_completion(&self.table, upload, completion_intent_hash),
            Participant::REGISTRY_UPLOAD,
        )
        .await
    }

    async fn finish_completion(
        &self,
        upload: UploadId,
        completion_intent_hash: &str,
    ) -> Result<(), StoreError> {
        self.conditional_update(
            expressions::finish_completion(&self.table, upload, completion_intent_hash),
            Participant::REGISTRY_UPLOAD,
        )
        .await
    }
}

#[async_trait]
impl ReceiptStore for RegistryDynamoStore {
    /// Reads one `regional-registry` idempotency receipt.
    ///
    /// This is the whole of E's D-19 on the read side: `upload_create` and
    /// `upload_complete` both carry `idempotency: idempotency_key`, and until this
    /// existed there was nothing for `commit_or_replay` to read, so a replayed
    /// `upload_create` would have opened a **second** multipart upload.
    async fn read_receipt(
        &self,
        workspace: WorkspaceId,
        scope: &IdempotencyScope<'_>,
        key: &IdempotencyKey,
        now: Timestamp,
    ) -> Result<Option<Receipt>, StoreError> {
        let rendered = scope.render();
        let target = keys::receipt(workspace, &rendered, &key_digest(key))?;
        let Some(item) = self.get(&target.pk, &target.sk).await? else {
            return Ok(None);
        };
        let receipt = codec::decode_receipt(&item)?;
        // Expiry is checked here rather than trusted to TTL: AWS reclaims a
        // TTL'd row within 48 hours, so a reader that trusted it would replay an
        // expired receipt for up to two days.
        if codec::receipt_is_live(&receipt, now) {
            Ok(Some(receipt))
        } else {
            Ok(None)
        }
    }
}
