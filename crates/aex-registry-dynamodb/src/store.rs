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
use aex_wire::ids::{UploadId, WorkspaceId};
use aex_workspace_domain::registry::RegistryPointer;
use aex_workspace_domain::upload::{Upload, UploadState};
use async_trait::async_trait;
use aws_sdk_dynamodb::Client;
use aws_sdk_dynamodb::types::ReturnValuesOnConditionCheckFailure;
use aws_sdk_dynamodb::types::builders::{PutBuilder, UpdateBuilder};

use crate::{codec, expressions, keys};

/// One page of a registry listing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PointerPage {
    /// The pointers this page names, in name order.
    pub pointers: Vec<RegistryPointer>,
    /// Where a continuation resumes, when there is more.
    pub next: Option<PagePosition>,
}

// TODO(cross-stream): replaced by aex_workspace_domain::ports::RegistryStore
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

    /// Reads one staged upload.
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
        match self.get(&target.pk, &target.sk).await? {
            None => Ok(None),
            Some(item) => Ok(Some(codec::decode_upload(&item, workspace)?)),
        }
    }

    async fn create_upload(&self, upload: &Upload) -> Result<(), StoreError> {
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
