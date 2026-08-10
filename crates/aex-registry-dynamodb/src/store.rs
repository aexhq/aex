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
use aex_session_dynamodb::plan::{Participant, TransactionPlan, key};
use aex_session_dynamodb::replay::{
    DecodeReceipt, IdempotencyScope, Receipt, ReceiptBody, ReceiptStore, decode_receipt_row,
    key_digest,
};
use aex_wire::idempotency::IdempotencyKey;
use aex_wire::ids::{UploadId, WorkspaceId};
use aex_wire::types::{ETag, Timestamp};
use aex_workspace_domain::registry::{
    RegistryCommit, RegistryPointer, RegistryRow, SetOutcome, ValueDocument, etag_of,
};
use aex_workspace_domain::upload::{Upload, UploadState};
use async_trait::async_trait;
use aws_sdk_dynamodb::Client;
use aws_sdk_dynamodb::types::builders::{DeleteBuilder, PutBuilder, UpdateBuilder};
use aws_sdk_dynamodb::types::{AttributeValue, ReturnValuesOnConditionCheckFailure};

use crate::{codec, expressions, keys};

/// One page of a registry listing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PointerPage {
    /// The collection rows this page names, in name order.
    ///
    /// Rows, not pointers: the query projects the collection columns and never
    /// reads `valueDoc`, so a full page cannot carry a thousand value documents.
    pub rows: Vec<RegistryRow>,
    /// Where a continuation resumes, when there is more.
    pub next: Option<PagePosition>,
}

/// The replayable answer of one registry `set`.
///
/// Only the facts that cannot be re-derived are stored. `sha256`, `sizeBytes`
/// and the `ETag` are all functions of the value document, the kind and the
/// revision, so storing them too would let a receipt disagree with itself. The
/// workspace is not stored either: a receipt is only ever read under the key of
/// the workspace that wrote it, so carrying it would be a second, forgeable
/// copy of a fact the key already fixes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SetReceipt {
    /// What the winning call did.
    pub outcome: SetOutcome,
    /// Which registry.
    pub kind: RegistryKind,
    /// Which name.
    pub name: aex_wire::ids::ResourceName,
    /// The revision the winner produced.
    pub revision: Revision,
    /// The value document the winner stored.
    pub value_doc: ValueDocument,
    /// When the pointer was first written.
    pub created_at: Timestamp,
    /// When the winner replaced it.
    pub updated_at: Timestamp,
}

/// The `responseKind` a registry set receipt carries.
pub const SET_RESPONSE_KIND: &str = "registry.set";

const SET_OUTCOMES: [(SetOutcome, &str); 3] = [
    (SetOutcome::Created, "created"),
    (SetOutcome::Replaced, "replaced"),
    (SetOutcome::Unchanged, "unchanged"),
];

impl SetReceipt {
    /// The answer one commit will replay.
    #[must_use]
    pub fn of(outcome: SetOutcome, pointer: &RegistryPointer) -> Self {
        Self {
            outcome,
            kind: pointer.row.kind,
            name: pointer.row.name.clone(),
            revision: pointer.row.revision,
            value_doc: pointer.value_doc.clone(),
            created_at: pointer.row.created_at,
            updated_at: pointer.row.updated_at,
        }
    }

    /// Rebuilds the pointer this answer describes, under the workspace whose
    /// receipt partition it was read from.
    #[must_use]
    pub fn pointer(&self, workspace: WorkspaceId) -> RegistryPointer {
        let sha256 = self.value_doc.digest();
        RegistryPointer {
            row: RegistryRow {
                workspace,
                kind: self.kind,
                name: self.name.clone(),
                revision: self.revision,
                etag: etag_of(self.kind, self.revision, &sha256),
                sha256,
                size_bytes: self.value_doc.size_bytes(),
                created_at: self.created_at,
                updated_at: self.updated_at,
            },
            value_doc: self.value_doc.clone(),
        }
    }

    /// The stored body of this answer.
    ///
    /// # Errors
    ///
    /// [`StoreError::Invalid`] when the answer could not be serialized.
    pub fn to_body(&self) -> Result<ReceiptBody, StoreError> {
        let outcome = SET_OUTCOMES
            .into_iter()
            .find_map(|(value, text)| (value == self.outcome).then_some(text))
            .unwrap_or_else(|| unreachable!("every set outcome has a spelling"));
        let body = serde_json::json!({
            "outcome": outcome,
            "kind": self.kind.as_str(),
            "name": self.name.as_str(),
            "revision": self.revision.0,
            "valueDoc": self.value_doc.as_str(),
            "createdAt": self.created_at.to_string(),
            "updatedAt": self.updated_at.to_string(),
        });
        Ok(ReceiptBody::Inline(serde_json::to_vec(&body).map_err(
            |error| StoreError::Invalid {
                detail: error.to_string(),
            },
        )?))
    }
}

impl DecodeReceipt for SetReceipt {
    fn decode_receipt(receipt: &Receipt) -> Result<Self, StoreError> {
        if receipt.response_kind != SET_RESPONSE_KIND {
            return Err(corrupt(
                "responseKind",
                &format!(
                    "a `{}` receipt is not a registry set answer",
                    receipt.response_kind
                ),
            ));
        }
        let ReceiptBody::Inline(bytes) = &receipt.response else {
            return Err(corrupt(
                "response",
                "a registry set answer is stored inline",
            ));
        };
        let body: serde_json::Value = serde_json::from_slice(bytes)
            .map_err(|error| corrupt("response", &error.to_string()))?;
        let text = |field: &'static str| -> Result<String, StoreError> {
            body.get(field)
                .and_then(serde_json::Value::as_str)
                .map(str::to_owned)
                .ok_or_else(|| corrupt(field, "a registry set answer names it"))
        };
        let spelling = text("outcome")?;
        let outcome = SET_OUTCOMES
            .into_iter()
            .find_map(|(value, name)| (name == spelling).then_some(value))
            .ok_or_else(|| corrupt("outcome", &format!("`{spelling}` is not a set outcome")))?;
        Ok(Self {
            outcome,
            kind: RegistryKind::parse(&text("kind")?)
                .ok_or_else(|| corrupt("kind", "outside the registry kind vocabulary"))?,
            name: aex_wire::ids::ResourceName::parse(&text("name")?)
                .map_err(|error| corrupt("name", &error.to_string()))?,
            revision: Revision(
                body.get("revision")
                    .and_then(serde_json::Value::as_u64)
                    .ok_or_else(|| corrupt("revision", "a registry set answer names it"))?,
            ),
            value_doc: ValueDocument::new(
                aex_wire::CanonicalJson::parse(&text("valueDoc")?)
                    .map_err(|error| corrupt("valueDoc", &error.to_string()))?,
            ),
            created_at: Timestamp::parse(&text("createdAt")?)
                .map_err(|error| corrupt("createdAt", &error.to_string()))?,
            updated_at: Timestamp::parse(&text("updatedAt")?)
                .map_err(|error| corrupt("updatedAt", &error.to_string()))?,
        })
    }
}

fn corrupt(attribute: &'static str, reason: &str) -> StoreError {
    StoreError::Corrupt(aex_session_dynamodb::attr::CodecError::Malformed {
        item_type: codec::IDEMPOTENCY_RECEIPT,
        attribute,
        reason: reason.to_owned(),
    })
}

/// Everything one registry `set` commits, beyond the pointer itself.
#[derive(Debug, Clone)]
pub struct SetCommit<'a> {
    /// What the domain decided.
    pub commit: &'a RegistryCommit,
    /// The revision the caller observed, when it observed one.
    pub from_revision: Option<Revision>,
    /// The `registry.entries` cap, claimed only on a create.
    pub entries_cap: u64,
    /// Whether this set creates the name.
    pub creates: bool,
    /// The durable receipt this set writes.
    pub receipt: &'a Receipt,
}

/// What a `commit_set` did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SetCommitted {
    /// This call committed.
    Committed,
    /// An earlier call with the same key and the same intent committed; this is
    /// that answer.
    Replayed(Box<SetReceipt>),
}

/// What a `commit_delete` did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeleteCommitted {
    /// The pointer was removed.
    Removed,
    /// The name was already absent. Delete is idempotent.
    Absent,
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

    /// Commits one registry `set` as a single transaction over this table.
    ///
    /// Every participant lives here: the conditional pointer put, the upload
    /// consume when one is consumed, the durable idempotency receipt and — on a
    /// create only — the `registry.entries` claim. Nothing is read first: the
    /// receipt's own `attribute_not_exists` condition is the replay fence, and
    /// the row it returns on failure is what distinguishes a replay from a
    /// conflict without a second round trip (D-8).
    ///
    /// # Errors
    ///
    /// [`StoreError::PreconditionFailed`] naming `registry.pointer` when another
    /// writer moved the revision (surfaced as `412`) or `registry.count` when the
    /// workspace is at its cap (surfaced as `limit_exceeded`);
    /// [`StoreError::IdempotencyConflict`] when the key was reused with a
    /// different intent.
    async fn commit_set(
        &self,
        workspace: WorkspaceId,
        commit: SetCommit<'_>,
    ) -> Result<SetCommitted, StoreError>;

    /// Removes one pointer, honouring `If-Match`, and releases its entry.
    ///
    /// # Errors
    ///
    /// [`StoreError::PreconditionFailed`] naming `registry.pointer` when an
    /// `If-Match` did not match; the observed row carries the current tag.
    async fn commit_delete(
        &self,
        workspace: WorkspaceId,
        kind: RegistryKind,
        name: &str,
        if_match: Option<&ETag>,
    ) -> Result<DeleteCommitted, StoreError>;

    /// Reads one `(workspace, kind)` entry count.
    ///
    /// # Errors
    ///
    /// As [`RegistryStore::load_pointer`].
    async fn load_count(
        &self,
        workspace: WorkspaceId,
        kind: RegistryKind,
    ) -> Result<u64, StoreError>;

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

    /// Writes every spilled part block of an upload before its head row exists.
    ///
    /// Blocks first, head second: a head row that named blocks nobody had written
    /// would decode as a short part plan that looks complete, which is exactly
    /// how a completion silently drops parts.
    ///
    /// # Errors
    ///
    /// As [`RegistryStore::create_upload`].
    async fn stage_part_blocks(&self, upload: &Upload) -> Result<(), StoreError>;

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

    /// Begins a completion under a manifest identity, persisting the manifest.
    ///
    /// # Errors
    ///
    /// As [`RegistryStore::transition_upload`]; a retry with a different
    /// manifest loses the `completionIntentHash` condition.
    async fn begin_completion(
        &self,
        upload: &Upload,
        completion_intent_hash: &str,
    ) -> Result<(), StoreError>;

    /// Commits one compiled cross-table transaction.
    ///
    /// The staged-upload admission is three items across two tables — the upload
    /// row, the `registry.upload_expiry` due item and the idempotency receipt —
    /// and it has to be one transaction so a replay is answered from the receipt
    /// **before** any provider call (E D-2, E D-19).
    ///
    /// # Errors
    ///
    /// [`StoreError::PreconditionFailed`] naming the participant that lost, and
    /// [`StoreError::CommitAmbiguous`] for a transport failure, which is never a
    /// silent retry.
    async fn commit_admission(&self, plan: &TransactionPlan) -> Result<(), StoreError>;

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
        self.read(pk, sk, true).await
    }

    /// Reads one item.
    ///
    /// Registry reads are eventually consistent everywhere (D-12): every
    /// mutation is fenced by a conditional write, so a stale pre-read cannot
    /// produce a wrong write — it produces a lost condition, which is a `412`.
    /// Upload rows keep the strong read their own state machine was written
    /// against.
    async fn read(&self, pk: &str, sk: &str, strong: bool) -> Result<Option<Item>, StoreError> {
        let output = self
            .client
            .get_item()
            .table_name(&self.table)
            .set_key(Some(key(pk, sk)))
            .consistent_read(strong)
            .send()
            .await
            .map_err(|error| classify(&error, Idempotence::Read))?;
        Ok(output.item)
    }

    async fn run(&self, plan: &TransactionPlan) -> Result<(), StoreError> {
        let request = plan.compile(&self.client)?;
        match request.send().await {
            Ok(_) => Ok(()),
            Err(error) => Err(match error.as_service_error() {
                Some(service) => {
                    aex_session_dynamodb::error::decode_cancellation(service, plan.participants())
                }
                None => classify(&error, Idempotence::Write(Resolution::IdempotencyReceipt)),
            }),
        }
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
        match self.read(&target.pk, &target.sk, false).await? {
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
            .expression_attribute_names("#name", "name")
            .expression_attribute_values(":pk", s(keys::kind_partition(workspace, kind)))
            .expression_attribute_values(":prefix", s(keys::name_prefix()))
            .projection_expression(codec::ROW_PROJECTION)
            .set_exclusive_start_key(from.map(|position| position.to_exclusive_start(None, None)))
            .limit(budget.limit())
            .send()
            .await
            .map_err(|error| classify(&error, Idempotence::Read))?;

        let mut rows = Vec::new();
        for item in output.items.unwrap_or_default() {
            rows.push(codec::decode_row(&item, workspace)?);
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
        Ok(PointerPage { rows, next })
    }

    async fn commit_set(
        &self,
        workspace: WorkspaceId,
        commit: SetCommit<'_>,
    ) -> Result<SetCommitted, StoreError> {
        let pointer = &commit.commit.pointer;
        let mut plan = TransactionPlan::new(format!(
            "rst-{}-{}",
            pointer.row.kind.as_str(),
            pointer.row.etag
        ));
        let pointer_put = match commit.from_revision {
            None => expressions::create_pointer(&self.table, pointer)?,
            Some(revision) => expressions::replace_pointer(&self.table, pointer, revision)?,
        };
        plan.put(Participant::REGISTRY_POINTER, pointer_put)?;
        if let Some(upload) = commit.commit.consumed_upload {
            plan.update(
                Participant::REGISTRY_UPLOAD,
                expressions::consume_upload(
                    &self.table,
                    upload,
                    pointer.row.kind,
                    pointer.row.name.as_str(),
                )?,
            )?;
        }
        if commit.creates {
            plan.update(
                Participant::REGISTRY_COUNT,
                expressions::claim_entry(
                    &self.table,
                    workspace,
                    pointer.row.kind,
                    commit.entries_cap,
                ),
            )?;
        }
        plan.put(
            Participant::REGISTRY_IDEMPOTENCY,
            expressions::put_receipt(&self.table, workspace, commit.receipt)?,
        )?;

        match self.run(&plan).await {
            Ok(()) => Ok(SetCommitted::Committed),
            // The receipt lost its `attribute_not_exists`, so an earlier call
            // with this key already committed. Its row came back with the
            // failure, so the answer costs no extra read (D-8).
            Err(StoreError::PreconditionFailed {
                participant,
                observed,
            }) if participant == Participant::REGISTRY_IDEMPOTENCY => {
                let Some(item) = observed else {
                    // The only way this condition fails is that the item exists,
                    // so an absent row here is the provider contradicting itself.
                    return Err(StoreError::CommitAmbiguous {
                        resolve_by: Resolution::IdempotencyReceipt,
                    });
                };
                let stored = decode_receipt_row(&item)?;
                if stored.intent != commit.receipt.intent {
                    return Err(StoreError::IdempotencyConflict);
                }
                Ok(SetCommitted::Replayed(Box::new(
                    SetReceipt::decode_receipt(&stored)?,
                )))
            }
            Err(error) => Err(error),
        }
    }

    async fn commit_delete(
        &self,
        workspace: WorkspaceId,
        kind: RegistryKind,
        name: &str,
        if_match: Option<&ETag>,
    ) -> Result<DeleteCommitted, StoreError> {
        let mut plan = TransactionPlan::new(format!("rdl-{workspace}-{}-{name}", kind.as_str()));
        plan.delete(
            Participant::REGISTRY_POINTER,
            expressions::delete_pointer(&self.table, workspace, kind, name, if_match)?,
        )?;
        plan.update(
            Participant::REGISTRY_COUNT,
            expressions::release_entry(&self.table, workspace, kind),
        )?;
        match self.run(&plan).await {
            Ok(()) => Ok(DeleteCommitted::Removed),
            Err(StoreError::PreconditionFailed {
                participant,
                observed,
            }) if participant == Participant::REGISTRY_POINTER => match observed {
                // No row to observe: the name was already absent, and delete is
                // idempotent (D-9). `204`, not `404` — the route declares none.
                None => Ok(DeleteCommitted::Absent),
                // A row that failed the condition failed the `If-Match`.
                Some(item) => Err(StoreError::PreconditionFailed {
                    participant,
                    observed: Some(item),
                }),
            },
            Err(error) => Err(error),
        }
    }

    async fn load_count(
        &self,
        workspace: WorkspaceId,
        kind: RegistryKind,
    ) -> Result<u64, StoreError> {
        let target = keys::count(workspace, kind);
        match self.read(&target.pk, &target.sk, false).await? {
            None => Ok(0),
            Some(item) => Ok(codec::decode_count(&item, workspace)?),
        }
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
        Ok(Some(codec::decode_upload_blocks(
            &head, &blocks, workspace,
        )?))
    }

    async fn create_upload(&self, upload: &Upload) -> Result<(), StoreError> {
        // Part blocks are written before the head, so a head row is never
        // visible without the parts it names. The head write is the conditional
        // one, and it is what makes the upload exist.
        RegistryStore::stage_part_blocks(self, upload).await?;
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

    async fn stage_part_blocks(&self, upload: &Upload) -> Result<(), StoreError> {
        for block in expressions::create_upload_part_blocks(&self.table, upload) {
            self.conditional_put(block, Participant::REGISTRY_UPLOAD)
                .await?;
        }
        Ok(())
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
        upload: &Upload,
        completion_intent_hash: &str,
    ) -> Result<(), StoreError> {
        // A spilled upload keeps its manifest in the blocks. They are written
        // first, so the head row's intent is never visible without the manifest
        // it names.
        if upload.parts.parts.len() > codec::PARTS_PER_BLOCK {
            let blocks = upload.parts.parts.len().div_ceil(codec::PARTS_PER_BLOCK);
            for block in 0..blocks {
                self.conditional_update(
                    expressions::record_completion_manifest(&self.table, upload, block),
                    Participant::REGISTRY_UPLOAD,
                )
                .await?;
            }
        }
        self.conditional_update(
            expressions::begin_completion(&self.table, upload, completion_intent_hash),
            Participant::REGISTRY_UPLOAD,
        )
        .await
    }

    async fn commit_admission(&self, plan: &TransactionPlan) -> Result<(), StoreError> {
        let request = plan.compile(&self.client)?;
        match request.send().await {
            Ok(_) => Ok(()),
            Err(error) => {
                if let Some(service) = error.as_service_error() {
                    return Err(aex_session_dynamodb::error::decode_cancellation(
                        service,
                        plan.participants(),
                    ));
                }
                Err(classify(
                    &error,
                    Idempotence::Write(Resolution::IdempotencyReceipt),
                ))
            }
        }
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
