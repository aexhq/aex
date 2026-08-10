//! Typed key-store configuration and read-only introspection.
//!
//! This crate deliberately implements **no write path** (D-20). Branch-key
//! creation and rotation are provider operations invoked only by
//! `regional-secret-key-admin`; reimplementing them here would be a silent
//! divergence risk with no upside, and the divergence would only show up as a
//! key store the provider tooling could no longer version.
//!
//! What is here: the binding a caller hands to the key-store implementation, and
//! the reads an administrator needs to make rotation idempotent and to notice
//! drift.

use aex_session_dynamodb::error::{Idempotence, StoreError, classify};
use aex_session_dynamodb::paging::PageBudget;
use async_trait::async_trait;
use aws_sdk_dynamodb::Client;
use aws_sdk_dynamodb::types::AttributeValue;

use crate::branch_key::{self, BranchKeyId, BranchKeyRecord, RecordKind};

/// Everything a key-store implementation needs to address this table.
///
/// The **logical key store name is the physical table name**. It is
/// cryptographically bound into every record and cannot be changed after first
/// use, so it is not a naming convenience: renaming the table orphans every
/// secret ever sealed under it. It is recorded in private composition and
/// asserted at startup.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyStoreBinding {
    table: String,
    kms_key_arn: String,
    grant_tokens: Vec<String>,
}

impl KeyStoreBinding {
    /// Binds a key store to a physical table and a root key.
    #[must_use]
    pub fn new(table: impl Into<String>, kms_key_arn: impl Into<String>) -> Self {
        Self {
            table: table.into(),
            kms_key_arn: kms_key_arn.into(),
            grant_tokens: Vec::new(),
        }
    }

    /// Adds the grant tokens a constrained role presents.
    #[must_use]
    pub fn with_grant_tokens(mut self, tokens: Vec<String>) -> Self {
        self.grant_tokens = tokens;
        self
    }

    /// The physical table name.
    #[must_use]
    pub fn table(&self) -> &str {
        &self.table
    }

    /// The logical key store name, which **equals** the physical table name.
    #[must_use]
    pub fn logical_key_store_name(&self) -> &str {
        &self.table
    }

    /// The root key branch keys are wrapped under.
    #[must_use]
    pub fn kms_key_arn(&self) -> &str {
        &self.kms_key_arn
    }

    /// The grant tokens.
    #[must_use]
    pub fn grant_tokens(&self) -> &[String] {
        &self.grant_tokens
    }
}

/// One active branch key, as an administrator **and** as the seal path sees it.
///
/// It used to be purely an administrator's view and dropped the one field a
/// seal needs. It no longer does (D-4): `wrapped_material` is the record's `enc`
/// attribute, and `SecretCrypto::seal` takes exactly those bytes.
#[derive(Clone, PartialEq, Eq)]
pub struct ActiveBranchKey {
    /// Which branch key.
    pub branch_key_id: BranchKeyId,
    /// Which versioned record it points at.
    pub version: Option<String>,
    /// When it was created, which is what makes rotation idempotent by attested
    /// input.
    pub create_time: String,
    /// The root key it is wrapped under, which is what makes drift visible.
    pub kms_arn: String,
    /// The hierarchy version.
    pub hierarchy_version: u64,
    /// The branch key **as the store holds it**: wrapped under the root key and
    /// never openable here.
    ///
    /// Renamed from the record's `enc` at this boundary so nothing downstream
    /// can read it as plaintext. It is opened by `KMS` inside `aex-secret-aws`
    /// and by nothing else, and it travels into the row it seals so a later
    /// rotation cannot orphan it (D-5).
    pub wrapped_material: Vec<u8>,
}

impl std::fmt::Debug for ActiveBranchKey {
    /// Renders every identifier and no material.
    ///
    /// The bytes are wrapped rather than plaintext, so printing them would leak
    /// nothing openable — but a wrapped key in a log is still key material in a
    /// log, and the byte count is the only part of it that is ever diagnostic.
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ActiveBranchKey")
            .field("branch_key_id", &self.branch_key_id)
            .field("version", &self.version)
            .field("create_time", &self.create_time)
            .field("kms_arn", &self.kms_arn)
            .field("hierarchy_version", &self.hierarchy_version)
            .field(
                "wrapped_material",
                &format_args!("<wrapped, {} bytes>", self.wrapped_material.len()),
            )
            .finish()
    }
}

impl From<BranchKeyRecord> for ActiveBranchKey {
    fn from(record: BranchKeyRecord) -> Self {
        Self {
            branch_key_id: record.branch_key_id,
            version: record.version,
            create_time: record.create_time,
            kms_arn: record.kms_arn,
            hierarchy_version: record.hierarchy_version,
            wrapped_material: record.enc,
        }
    }
}

/// Read-only introspection over the branch key store.
///
/// There is no write method on this trait, and that is the point.
#[async_trait]
pub trait BranchKeyStoreReader: Send + Sync + 'static {
    /// Describes one workspace's active branch key.
    ///
    /// # Errors
    ///
    /// [`StoreError`] for a transport or decode failure.
    async fn describe_active(
        &self,
        branch_key_id: &BranchKeyId,
    ) -> Result<Option<ActiveBranchKey>, StoreError>;

    /// Lists the branch key identities the store holds.
    ///
    /// # Errors
    ///
    /// As [`BranchKeyStoreReader::describe_active`].
    async fn list_branch_key_ids(&self, budget: PageBudget)
    -> Result<Vec<BranchKeyId>, StoreError>;
}

/// The adapter.
#[derive(Debug, Clone)]
pub struct KeyStoreReader {
    client: Client,
    binding: KeyStoreBinding,
}

impl KeyStoreReader {
    /// Binds a reader to a client and a key store.
    #[must_use]
    pub const fn new(client: Client, binding: KeyStoreBinding) -> Self {
        Self { client, binding }
    }

    /// The binding.
    #[must_use]
    pub const fn binding(&self) -> &KeyStoreBinding {
        &self.binding
    }
}

#[async_trait]
impl BranchKeyStoreReader for KeyStoreReader {
    async fn describe_active(
        &self,
        branch_key_id: &BranchKeyId,
    ) -> Result<Option<ActiveBranchKey>, StoreError> {
        let output = self
            .client
            .get_item()
            .table_name(self.binding.table())
            .key(
                branch_key::BRANCH_KEY_ID,
                AttributeValue::S(branch_key_id.as_str().to_owned()),
            )
            .key(
                branch_key::TYPE,
                AttributeValue::S(RecordKind::Active.as_sort_key()),
            )
            .consistent_read(true)
            .send()
            .await
            .map_err(|error| classify(&error, Idempotence::Read))?;
        match output.item {
            None => Ok(None),
            Some(item) => {
                let record = branch_key::decode(&item).map_err(|error| StoreError::Invalid {
                    detail: error.to_string(),
                })?;
                Ok(Some(record.into()))
            }
        }
    }

    async fn list_branch_key_ids(
        &self,
        budget: PageBudget,
    ) -> Result<Vec<BranchKeyId>, StoreError> {
        // A `Scan` is right here and nowhere else: the store holds one partition
        // per workspace with no secondary index, and this read exists for an
        // administrator's drift check rather than for any request path.
        let output = self
            .client
            .scan()
            .table_name(self.binding.table())
            .projection_expression("#id, #type")
            .expression_attribute_names("#id", branch_key::BRANCH_KEY_ID)
            .expression_attribute_names("#type", branch_key::TYPE)
            .limit(budget.limit())
            .send()
            .await
            .map_err(|error| classify(&error, Idempotence::Read))?;

        let mut ids = Vec::new();
        for item in output.items.unwrap_or_default() {
            let is_active = item
                .get(branch_key::TYPE)
                .and_then(|value| value.as_s().ok())
                .is_some_and(|text| text == branch_key::ACTIVE);
            if !is_active {
                continue;
            }
            if let Some(id) = item
                .get(branch_key::BRANCH_KEY_ID)
                .and_then(|value| value.as_s().ok())
            {
                ids.push(BranchKeyId::from_stored(id.clone()));
            }
        }
        ids.sort();
        Ok(ids)
    }
}

#[cfg(test)]
mod tests {
    use super::KeyStoreBinding;

    #[test]
    fn the_logical_key_store_name_is_the_physical_table_name() {
        let binding = KeyStoreBinding::new(
            "dev-eu-west-1-regional-secret-keystore",
            "arn:aws:kms:eu-west-1:000000000000:key/keystore",
        );
        assert_eq!(binding.logical_key_store_name(), binding.table());
        assert_eq!(
            binding.logical_key_store_name(),
            "dev-eu-west-1-regional-secret-keystore",
            "the logical name is bound into every record and can never change \
             after first use"
        );
    }

    #[test]
    fn grant_tokens_are_carried_rather_than_assumed_absent() {
        let binding = KeyStoreBinding::new("t", "arn").with_grant_tokens(vec!["token".to_owned()]);
        assert_eq!(binding.grant_tokens(), ["token"]);
    }
}
