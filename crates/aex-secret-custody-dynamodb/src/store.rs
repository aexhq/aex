//! The `regional-secret-custody` port implementation.
//!
//! `list_secrets` reads the metadata partition and returns [`SecretMetadata`],
//! which cannot hold a ciphertext. Reading sealed bytes takes a separate,
//! deliberate call into another partition, so "the list endpoint leaked a
//! secret" is not a bug that can be written here.

use aex_secret_domain::custody::CustodyRevision;
use aex_secret_domain::secret::{SecretName, SecretRevision, SourceGeneration};
use aex_session_dynamodb::attr::{Item, s};
use aex_session_dynamodb::error::{
    Idempotence, Resolution, StoreError, classify, decode_cancellation,
};
use aex_session_dynamodb::paging::{PageBudget, PagePosition};
use aex_session_dynamodb::plan::{Participant, TransactionPlan, key};
use aex_session_dynamodb::replay::{Receipt, decode_receipt_row, receipt_is_live};
use aex_wire::ids::{ProviderCredentialId, SessionId, WorkspaceId};
use aex_wire::types::Timestamp;
use async_trait::async_trait;
use aws_sdk_dynamodb::Client;
use aws_sdk_dynamodb::types::ReturnValuesOnConditionCheckFailure;
use aws_sdk_dynamodb::types::builders::UpdateBuilder;

use crate::codec::{
    self, CallAuthorization, CustodyBinding, CustodyHead, ProviderCredential, RedactionManifest,
    SecretMetadata, StoredGeneration,
};
use crate::keys;

/// One bounded page and the position a continuation resumes from.
///
/// `next` is `Some` exactly when the authority reported more rows behind this
/// page, so a caller can never mistake a full page for the end of a collection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Page<T> {
    /// The rows, in key order.
    pub items: Vec<T>,
    /// Where the next page starts, when there is one.
    pub next: Option<PagePosition>,
}

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

    /// Reads one provider-credential binding by identity.
    ///
    /// The identity alone does not name a key — the sort key carries the
    /// provider — so this walks the workspace's directory partition under a
    /// bounded budget rather than scanning the table.
    ///
    /// # Errors
    ///
    /// As [`SecretCustodyStore::load_secret`].
    async fn load_provider_credential(
        &self,
        workspace: WorkspaceId,
        credential: ProviderCredentialId,
    ) -> Result<Option<ProviderCredential>, StoreError>;

    /// Lists one workspace's secrets from a continuation.
    ///
    /// Returns the page plus the position a continuation resumes from, which is
    /// `None` exactly when the page is the last one. A list that could not name
    /// its own continuation would silently truncate.
    ///
    /// # Errors
    ///
    /// As [`SecretCustodyStore::load_secret`].
    async fn page_secrets(
        &self,
        workspace: WorkspaceId,
        budget: PageBudget,
        after: Option<&PagePosition>,
    ) -> Result<Page<SecretMetadata>, StoreError>;

    /// Lists one workspace's provider-credential bindings from a continuation.
    ///
    /// # Errors
    ///
    /// As [`SecretCustodyStore::page_secrets`].
    async fn page_provider_credentials(
        &self,
        workspace: WorkspaceId,
        budget: PageBudget,
        after: Option<&PagePosition>,
    ) -> Result<Page<ProviderCredential>, StoreError>;

    /// Reads one durable idempotency receipt, or `None` when it is absent or
    /// expired.
    ///
    /// # Errors
    ///
    /// As [`SecretCustodyStore::load_secret`].
    async fn load_receipt(
        &self,
        workspace: WorkspaceId,
        scope: &str,
        key_sha256_hex: &str,
        now: Timestamp,
    ) -> Result<Option<Receipt>, StoreError>;

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

    /// Reads one provider binding by its complete `DynamoDB` key.
    ///
    /// Brain dispatch already knows the provider from its immutable session
    /// configuration. Preserving it here turns the hot-path lookup into one
    /// strongly consistent point read instead of probing the six-provider
    /// directory.
    ///
    /// # Errors
    ///
    /// [`StoreError`] for a transport, key, or decode failure.
    pub async fn load_provider_credential_for_provider(
        &self,
        workspace: WorkspaceId,
        provider: aex_wire::models::ProviderId,
        credential: ProviderCredentialId,
    ) -> Result<Option<ProviderCredential>, StoreError> {
        let target = keys::provider_credential(workspace, provider.as_str(), credential)?;
        match self.get(&target.pk, &target.sk).await? {
            None => Ok(None),
            Some(item) => Ok(Some(codec::decode_provider_credential(&item, workspace)?)),
        }
    }

    /// Reads one immutable session binding by its complete custody key.
    ///
    /// # Errors
    ///
    /// [`StoreError`] for a transport, key, or decode failure.
    pub async fn load_custody_binding(
        &self,
        workspace: WorkspaceId,
        session: SessionId,
        revision: CustodyRevision,
        name: &SecretName,
    ) -> Result<Option<CustodyBinding>, StoreError> {
        let target = keys::binding(session, revision, name.as_str())?;
        match self.get(&target.pk, &target.sk).await? {
            None => Ok(None),
            Some(item) => Ok(Some(codec::decode_binding(&item, workspace)?)),
        }
    }

    /// Atomically revalidates the mutable secret and custody fences and writes
    /// one immutable managed-call authorization before any decrypt occurs.
    ///
    /// # Errors
    ///
    /// [`StoreError::PreconditionFailed`] when revocation, rebind, deletion, or
    /// a duplicate authorization wins; other variants retain their usual
    /// transport and corruption meanings.
    pub async fn authorize_managed_call(
        &self,
        authorization: &CallAuthorization,
        bound_source_revision: SecretRevision,
    ) -> Result<(), StoreError> {
        let plan = crate::expressions::authorize_managed_call(
            &self.table,
            authorization,
            bound_source_revision,
        )?;
        SecretCustodyStore::commit(self, &plan).await
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
        Ok(self.query_page(partition, prefix, budget, None).await?.0)
    }

    async fn query_page(
        &self,
        partition: &str,
        prefix: &str,
        budget: PageBudget,
        after: Option<&PagePosition>,
    ) -> Result<(Vec<Item>, Option<PagePosition>), StoreError> {
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
            .set_exclusive_start_key(after.map(|position| position.to_exclusive_start(None, None)))
            .send()
            .await
            .map_err(|error| classify(&error, Idempotence::Read))?;
        // The table has no index, so a continuation is the base key alone.
        let next = match output.last_evaluated_key {
            None => None,
            Some(key) => Some(PagePosition::from_last_evaluated(&key, None, None).map_err(
                |error| {
                    // A continuation the authority returned that this adapter
                    // cannot resume is corruption, not a customer condition:
                    // answering the page without it would silently truncate.
                    StoreError::Corrupt(aex_session_dynamodb::attr::CodecError::Malformed {
                        item_type: "page_continuation",
                        attribute: "lastEvaluatedKey",
                        reason: error.to_string(),
                    })
                },
            )?),
        };
        Ok((output.items.unwrap_or_default(), next))
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

    async fn load_provider_credential(
        &self,
        workspace: WorkspaceId,
        credential: ProviderCredentialId,
    ) -> Result<Option<ProviderCredential>, StoreError> {
        // The sort key is `CRED#{provider}#{credential}`, so the identity alone
        // names a suffix rather than a key. Six providers is a closed set, so the
        // read is six bounded point reads and never a scan.
        for provider in aex_wire::models::ProviderId::ALL {
            if let Some(binding) = self
                .load_provider_credential_for_provider(workspace, *provider, credential)
                .await?
            {
                return Ok(Some(binding));
            }
        }
        Ok(None)
    }

    async fn page_secrets(
        &self,
        workspace: WorkspaceId,
        budget: PageBudget,
        after: Option<&PagePosition>,
    ) -> Result<Page<SecretMetadata>, StoreError> {
        let (items, next) = self
            .query_page(
                &keys::secret_partition(workspace),
                keys::secret_prefix(),
                budget,
                after,
            )
            .await?;
        Ok(Page {
            items: items
                .iter()
                .map(|item| codec::decode_secret(item, workspace).map_err(StoreError::from))
                .collect::<Result<Vec<_>, _>>()?,
            next,
        })
    }

    async fn page_provider_credentials(
        &self,
        workspace: WorkspaceId,
        budget: PageBudget,
        after: Option<&PagePosition>,
    ) -> Result<Page<ProviderCredential>, StoreError> {
        let (items, next) = self
            .query_page(
                &format!("PCR#{workspace}"),
                keys::provider_credential_prefix(),
                budget,
                after,
            )
            .await?;
        Ok(Page {
            items: items
                .iter()
                .map(|item| {
                    codec::decode_provider_credential(item, workspace).map_err(StoreError::from)
                })
                .collect::<Result<Vec<_>, _>>()?,
            next,
        })
    }

    async fn load_receipt(
        &self,
        workspace: WorkspaceId,
        scope: &str,
        key_sha256_hex: &str,
        now: Timestamp,
    ) -> Result<Option<Receipt>, StoreError> {
        let target = keys::receipt(workspace, scope, key_sha256_hex)?;
        let Some(item) = self.get(&target.pk, &target.sk).await? else {
            return Ok(None);
        };
        let receipt = decode_receipt_row(&item)?;
        // `regional-secret-custody` disables TTL deliberately, so the explicit
        // expiry is the only fence. An expired receipt is absent, never a stale
        // hit.
        Ok(receipt_is_live(&receipt, now).then_some(receipt))
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
