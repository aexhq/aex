//! The `regional-secret-custody` port implementation.
//!
//! `list_secrets` reads the metadata partition and returns [`SecretMetadata`],
//! which cannot hold a ciphertext. Reading sealed bytes takes a separate,
//! deliberate call into another partition, so "the list endpoint leaked a
//! secret" is not a bug that can be written here.

use aex_secret_domain::custody::CustodyRevision;
use aex_secret_domain::secret::{SecretName, SourceGeneration};
use aex_session_dynamodb::attr::{Item, s};
use aex_session_dynamodb::error::{
    Idempotence, Resolution, StoreError, classify, decode_cancellation,
};
use aex_session_dynamodb::paging::PageBudget;
use aex_session_dynamodb::plan::{Participant, TransactionPlan, key};
use aex_wire::ids::{SessionId, WorkspaceId};
use aex_wire::types::Timestamp;
use async_trait::async_trait;
use aws_sdk_dynamodb::Client;
use aws_sdk_dynamodb::types::ReturnValuesOnConditionCheckFailure;
use aws_sdk_dynamodb::types::builders::UpdateBuilder;

use crate::codec::{
    self, CustodyHead, ProviderCredential, RedactionManifest, SecretMetadata, StoredGeneration,
};
use crate::keys;

// TODO(cross-stream): `aex-secret-domain` has no `ports` module and publishes no
// traits; see the same note in `aex-secret-aws`. This port has no peer to be replaced
// by.
/// The `regional-secret-custody` authority.
#[async_trait]
pub trait SecretCustodyStore: Send + Sync + 'static {
    /// Reads one secret's metadata. Never a ciphertext.
    ///
    /// # Errors
    ///
    /// [`StoreError`] for a transport or decode failure.
    async fn load_secret(
        &self,
        workspace: WorkspaceId,
        name: &SecretName,
    ) -> Result<Option<SecretMetadata>, StoreError>;

    /// Lists one workspace's secrets. Never a ciphertext.
    ///
    /// # Errors
    ///
    /// As [`SecretCustodyStore::load_secret`].
    async fn list_secrets(
        &self,
        workspace: WorkspaceId,
        budget: PageBudget,
    ) -> Result<Vec<SecretMetadata>, StoreError>;

    /// Reads one hidden source generation, which is the only row holding sealed
    /// bytes.
    ///
    /// # Errors
    ///
    /// As [`SecretCustodyStore::load_secret`].
    async fn load_generation(
        &self,
        workspace: WorkspaceId,
        name: &SecretName,
        generation: SourceGeneration,
    ) -> Result<Option<StoredGeneration>, StoreError>;

    /// Reads one session's custody head.
    ///
    /// # Errors
    ///
    /// As [`SecretCustodyStore::load_secret`].
    async fn load_custody(
        &self,
        workspace: WorkspaceId,
        session: SessionId,
    ) -> Result<Option<CustodyHead>, StoreError>;

    /// Reads the redaction manifest a collector redacts from.
    ///
    /// # Errors
    ///
    /// As [`SecretCustodyStore::load_secret`].
    async fn load_manifest(
        &self,
        workspace: WorkspaceId,
        session: SessionId,
    ) -> Result<Option<RedactionManifest>, StoreError>;

    /// Lists one workspace's provider-credential bindings.
    ///
    /// # Errors
    ///
    /// As [`SecretCustodyStore::load_secret`].
    async fn list_provider_credentials(
        &self,
        workspace: WorkspaceId,
        budget: PageBudget,
    ) -> Result<Vec<ProviderCredential>, StoreError>;

    /// Commits one compiled transaction.
    ///
    /// # Errors
    ///
    /// [`StoreError::PreconditionFailed`] naming the participant that lost. On
    /// `secret.metadata` in a use path that is `secret_revoked`, and the caller
    /// **must not** decrypt.
    async fn commit(&self, plan: &TransactionPlan) -> Result<(), StoreError>;

    /// Commits one conditional update.
    ///
    /// # Errors
    ///
    /// As [`SecretCustodyStore::commit`].
    async fn commit_update(
        &self,
        builder: UpdateBuilder,
        participant: Participant,
    ) -> Result<(), StoreError>;
}

/// The adapter.
#[derive(Debug, Clone)]
pub struct CustodyStore {
    client: Client,
    table: String,
}

impl CustodyStore {
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

    async fn query_prefix(
        &self,
        partition: &str,
        prefix: &str,
        budget: PageBudget,
    ) -> Result<Vec<Item>, StoreError> {
        let output = self
            .client
            .query()
            .table_name(&self.table)
            .key_condition_expression("#pk = :pk AND begins_with(#sk, :prefix)")
            .expression_attribute_names("#pk", aex_session_dynamodb::attr::PK)
            .expression_attribute_names("#sk", aex_session_dynamodb::attr::SK)
            .expression_attribute_values(":pk", s(partition.to_owned()))
            .expression_attribute_values(":prefix", s(prefix.to_owned()))
            .consistent_read(true)
            .limit(budget.limit())
            .send()
            .await
            .map_err(|error| classify(&error, Idempotence::Read))?;
        Ok(output.items.unwrap_or_default())
    }
}

#[async_trait]
impl SecretCustodyStore for CustodyStore {
    async fn load_secret(
        &self,
        workspace: WorkspaceId,
        name: &SecretName,
    ) -> Result<Option<SecretMetadata>, StoreError> {
        let target = keys::secret(workspace, name.as_str())?;
        match self.get(&target.pk, &target.sk).await? {
            None => Ok(None),
            Some(item) => Ok(Some(codec::decode_secret(&item, workspace)?)),
        }
    }

    async fn list_secrets(
        &self,
        workspace: WorkspaceId,
        budget: PageBudget,
    ) -> Result<Vec<SecretMetadata>, StoreError> {
        let items = self
            .query_prefix(
                &keys::secret_partition(workspace),
                keys::secret_prefix(),
                budget,
            )
            .await?;
        items
            .iter()
            .map(|item| codec::decode_secret(item, workspace).map_err(StoreError::from))
            .collect()
    }

    async fn load_generation(
        &self,
        workspace: WorkspaceId,
        name: &SecretName,
        generation: SourceGeneration,
    ) -> Result<Option<StoredGeneration>, StoreError> {
        let target = keys::generation(workspace, name.as_str(), generation)?;
        match self.get(&target.pk, &target.sk).await? {
            None => Ok(None),
            Some(item) => Ok(Some(codec::decode_generation(&item, workspace)?)),
        }
    }

    async fn load_custody(
        &self,
        workspace: WorkspaceId,
        session: SessionId,
    ) -> Result<Option<CustodyHead>, StoreError> {
        let target = keys::custody_head(session);
        match self.get(&target.pk, &target.sk).await? {
            None => Ok(None),
            Some(item) => Ok(Some(codec::decode_custody_head(&item, workspace)?)),
        }
    }

    async fn load_manifest(
        &self,
        workspace: WorkspaceId,
        session: SessionId,
    ) -> Result<Option<RedactionManifest>, StoreError> {
        let target = keys::redaction_manifest(session);
        match self.get(&target.pk, &target.sk).await? {
            None => Ok(None),
            Some(item) => Ok(Some(codec::decode_manifest(&item, workspace)?)),
        }
    }

    async fn list_provider_credentials(
        &self,
        workspace: WorkspaceId,
        budget: PageBudget,
    ) -> Result<Vec<ProviderCredential>, StoreError> {
        let items = self
            .query_prefix(
                &format!("PCR#{workspace}"),
                keys::provider_credential_prefix(),
                budget,
            )
            .await?;
        items
            .iter()
            .map(|item| {
                codec::decode_provider_credential(item, workspace).map_err(StoreError::from)
            })
            .collect()
    }

    async fn commit(&self, plan: &TransactionPlan) -> Result<(), StoreError> {
        let request = plan.compile(&self.client)?;
        match request.send().await {
            Ok(_) => Ok(()),
            Err(error) => Err(match error.as_service_error() {
                Some(service) => decode_cancellation(service, plan.participants()),
                None => classify(&error, Idempotence::Write(Resolution::TargetItem)),
            }),
        }
    }

    async fn commit_update(
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

/// Writes one redaction manifest.
///
/// Exposed separately from the port because the manifest is written by the
/// custody path and read by a role that holds nothing else on this table.
///
/// # Errors
///
/// [`StoreError`] when the write fails.
pub async fn put_manifest(
    store: &CustodyStore,
    manifest: &RedactionManifest,
    now: Timestamp,
) -> Result<(), StoreError> {
    let _ = now;
    store
        .client
        .put_item()
        .table_name(&store.table)
        .set_item(Some(codec::encode_manifest(manifest)))
        .send()
        .await
        .map_err(|error| classify(&error, Idempotence::Write(Resolution::TargetItem)))?;
    Ok(())
}

/// The custody revision a session with no custody row reports.
pub const NO_CUSTODY: CustodyRevision = CustodyRevision::NONE;
