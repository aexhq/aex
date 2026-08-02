//! AWS implementation of the exclusive key-administration port.
//!
//! A KMS ciphertext is generated before the `DynamoDB` transaction. A crash in
//! that interval leaks no plaintext and creates no authority row. The two row
//! mutations (immutable version plus active pointer) then commit atomically.

use std::collections::{BTreeMap, HashMap};

use aex_secret_keystore_dynamodb::branch_key::{
    self, ACTIVE, BRANCH_KEY_ID, BranchKeyId, CREATE_TIME, CUSTOM_CONTEXT_PREFIX, ENC,
    HIERARCHY_VERSION, KMS_ARN, RecordKind, TYPE, VERSION,
};
use aex_wire::types::Timestamp;
use async_trait::async_trait;
use aws_sdk_dynamodb::Client as DynamoClient;
use aws_sdk_dynamodb::types::{AttributeValue, Put, TransactWriteItem};
use aws_sdk_kms::Client as KmsClient;
use aws_sdk_kms::primitives::Blob;
use aws_sdk_kms::types::DataKeySpec;
use aws_smithy_types::error::metadata::ProvideErrorMetadata;
use zeroize::Zeroizing;

use crate::admin::{ActiveGeneration, AdminRunError, ApplyOutcome, GenerationSpec, KeyAdminPort};

const CONTEXT_PLANE: &str = "aex:plane";
const CONTEXT_REGION: &str = "aex:region";
const CONTEXT_WORKSPACE: &str = "aex:workspace";
const CONTEXT_ATTESTATION: &str = "aex:attestation";
const CONTEXT_REASON_DIGEST: &str = "aex:rotation-reason-sha256";

/// Exact production adapter over the isolated keystore table and root KMS key.
#[derive(Debug, Clone)]
pub struct AwsKeyAdmin {
    dynamodb: DynamoClient,
    kms: KmsClient,
    table: String,
    kms_arn: String,
    plane: String,
    region: String,
}

impl AwsKeyAdmin {
    /// Binds the adapter to its only two AWS authorities.
    #[must_use]
    pub fn new(
        dynamodb: DynamoClient,
        kms: KmsClient,
        table: impl Into<String>,
        kms_arn: impl Into<String>,
        plane: impl Into<String>,
        region: impl Into<String>,
    ) -> Self {
        Self {
            dynamodb,
            kms,
            table: table.into(),
            kms_arn: kms_arn.into(),
            plane: plane.into(),
            region: region.into(),
        }
    }

    async fn record(
        &self,
        branch_key_id: &BranchKeyId,
    ) -> Result<Option<branch_key::BranchKeyRecord>, AdminRunError> {
        let output = self
            .dynamodb
            .get_item()
            .table_name(&self.table)
            .key(
                BRANCH_KEY_ID,
                AttributeValue::S(branch_key_id.as_str().to_owned()),
            )
            .key(TYPE, AttributeValue::S(ACTIVE.to_owned()))
            .consistent_read(true)
            .send()
            .await
            .map_err(|error| provider("DynamoDB.GetItem", &error))?;
        output
            .item
            .map(|item| {
                branch_key::decode(&item).map_err(|_| AdminRunError::InvalidStored {
                    reason: "the active record does not match the keystore schema",
                })
            })
            .transpose()
    }

    fn active_from(record: branch_key::BranchKeyRecord) -> Result<ActiveGeneration, AdminRunError> {
        if record.kind != RecordKind::Active {
            return Err(AdminRunError::InvalidStored {
                reason: "the active key resolved to a non-active record",
            });
        }
        let version = record.version.ok_or(AdminRunError::InvalidStored {
            reason: "the active key has no version pointer",
        })?;
        Ok(ActiveGeneration {
            version,
            hierarchy_version: record.hierarchy_version,
            kms_arn: record.kms_arn,
        })
    }

    fn context(&self, generation: &GenerationSpec) -> BTreeMap<String, String> {
        let mut context = BTreeMap::from([
            (CONTEXT_PLANE.to_owned(), self.plane.clone()),
            (CONTEXT_REGION.to_owned(), self.region.clone()),
            (
                CONTEXT_WORKSPACE.to_owned(),
                generation.branch_key_id.as_str().to_owned(),
            ),
            (
                CONTEXT_ATTESTATION.to_owned(),
                generation.attestation.to_string(),
            ),
        ]);
        if let Some(digest) = &generation.reason_digest {
            context.insert(CONTEXT_REASON_DIGEST.to_owned(), digest.clone());
        }
        context
    }

    async fn wrapped_generation(
        &self,
        generation: &GenerationSpec,
    ) -> Result<(Vec<u8>, BTreeMap<String, String>), AdminRunError> {
        let context = self.context(generation);
        let mut request = self
            .kms
            .generate_data_key_without_plaintext()
            .key_id(&self.kms_arn)
            .key_spec(DataKeySpec::Aes256);
        for (name, value) in &context {
            request = request.encryption_context(name, value);
        }
        let output = request
            .send()
            .await
            .map_err(|error| provider("KMS.GenerateDataKeyWithoutPlaintext", &error))?;
        let wrapped = output
            .ciphertext_blob
            .ok_or(AdminRunError::InvalidStored {
                reason: "KMS returned no wrapped branch key",
            })?
            .into_inner();
        if wrapped.is_empty() {
            return Err(AdminRunError::InvalidStored {
                reason: "KMS returned an empty wrapped branch key",
            });
        }
        Ok((wrapped, context))
    }

    fn item(
        &self,
        generation: &GenerationSpec,
        kind: &str,
        wrapped: &[u8],
        context: &BTreeMap<String, String>,
        created_at: &str,
    ) -> HashMap<String, AttributeValue> {
        let mut item = HashMap::from([
            (
                BRANCH_KEY_ID.to_owned(),
                AttributeValue::S(generation.branch_key_id.as_str().to_owned()),
            ),
            (TYPE.to_owned(), AttributeValue::S(kind.to_owned())),
            (
                ENC.to_owned(),
                AttributeValue::B(Blob::new(wrapped.to_vec())),
            ),
            (KMS_ARN.to_owned(), AttributeValue::S(self.kms_arn.clone())),
            (
                CREATE_TIME.to_owned(),
                AttributeValue::S(created_at.to_owned()),
            ),
            (
                HIERARCHY_VERSION.to_owned(),
                AttributeValue::N(generation.hierarchy_version.to_string()),
            ),
            (
                VERSION.to_owned(),
                AttributeValue::S(generation.version.clone()),
            ),
        ]);
        for (name, value) in context {
            item.insert(
                format!("{CUSTOM_CONTEXT_PREFIX}{name}"),
                AttributeValue::S(value.clone()),
            );
        }
        item
    }

    fn writes(
        &self,
        generation: &GenerationSpec,
        expected: Option<&ActiveGeneration>,
        wrapped: &[u8],
        context: &BTreeMap<String, String>,
        created_at: &str,
    ) -> Result<(Put, Put), AdminRunError> {
        let version = Put::builder()
            .table_name(&self.table)
            .set_item(Some(self.item(
                generation,
                &generation.version,
                wrapped,
                context,
                created_at,
            )))
            .condition_expression("attribute_not_exists(#branch) AND attribute_not_exists(#type)")
            .expression_attribute_names("#branch", BRANCH_KEY_ID)
            .expression_attribute_names("#type", TYPE)
            .build()
            .map_err(|_| AdminRunError::InvalidStored {
                reason: "the immutable generation write could not be built",
            })?;

        let mut active = Put::builder()
            .table_name(&self.table)
            .set_item(Some(
                self.item(generation, ACTIVE, wrapped, context, created_at),
            ))
            .expression_attribute_names("#version", VERSION);
        active = match expected {
            None => active.condition_expression("attribute_not_exists(#version)"),
            Some(expected) => active
                .condition_expression(
                    "#version = :expected_version AND #hierarchy = :expected_hierarchy",
                )
                .expression_attribute_names("#hierarchy", HIERARCHY_VERSION)
                .expression_attribute_values(
                    ":expected_version",
                    AttributeValue::S(expected.version.clone()),
                )
                .expression_attribute_values(
                    ":expected_hierarchy",
                    AttributeValue::N(expected.hierarchy_version.to_string()),
                ),
        };
        let active = active.build().map_err(|_| AdminRunError::InvalidStored {
            reason: "the active generation write could not be built",
        })?;
        Ok((version, active))
    }

    async fn commit(
        &self,
        generation: &GenerationSpec,
        expected: Option<&ActiveGeneration>,
    ) -> Result<ApplyOutcome, AdminRunError> {
        let (wrapped, context) = self.wrapped_generation(generation).await?;
        let created_at = Timestamp::from_datetime_trunc_ms(time::OffsetDateTime::now_utc())
            .map_err(|_| AdminRunError::InvalidStored {
                reason: "the system clock is outside the canonical timestamp range",
            })?
            .to_string();
        let (version, active) =
            self.writes(generation, expected, &wrapped, &context, &created_at)?;

        let result = self
            .dynamodb
            .transact_write_items()
            .client_request_token(generation.attestation.to_string())
            .transact_items(TransactWriteItem::builder().put(version).build())
            .transact_items(TransactWriteItem::builder().put(active).build())
            .send()
            .await;
        match result {
            Ok(_) => Ok(ApplyOutcome::Applied),
            Err(error) if conditional_conflict(&error) => Ok(ApplyOutcome::Conflict),
            Err(error) => Err(provider("DynamoDB.TransactWriteItems", &error)),
        }
    }
}

#[async_trait]
impl KeyAdminPort for AwsKeyAdmin {
    async fn active(
        &self,
        branch_key_id: &BranchKeyId,
    ) -> Result<Option<ActiveGeneration>, AdminRunError> {
        self.record(branch_key_id)
            .await?
            .map(Self::active_from)
            .transpose()
    }

    async fn create(&self, generation: &GenerationSpec) -> Result<ApplyOutcome, AdminRunError> {
        if generation.hierarchy_version != 1 {
            return Err(AdminRunError::InvalidStored {
                reason: "creation did not target hierarchy generation one",
            });
        }
        self.commit(generation, None).await
    }

    async fn rotate(
        &self,
        expected: &ActiveGeneration,
        generation: &GenerationSpec,
    ) -> Result<ApplyOutcome, AdminRunError> {
        let Some(next_hierarchy) = expected.hierarchy_version.checked_add(1) else {
            return Err(AdminRunError::GenerationOverflow);
        };
        if expected.kms_arn != self.kms_arn || generation.hierarchy_version != next_hierarchy {
            return Err(AdminRunError::InvalidStored {
                reason: "rotation did not extend the bound root-key lineage exactly once",
            });
        }
        self.commit(generation, Some(expected)).await
    }

    async fn verify(
        &self,
        branch_key_id: &BranchKeyId,
        expected: &ActiveGeneration,
    ) -> Result<(), AdminRunError> {
        let record = self
            .record(branch_key_id)
            .await?
            .ok_or(AdminRunError::LineageMissing)?;
        let observed = Self::active_from(record.clone())?;
        if &observed != expected || observed.kms_arn != self.kms_arn {
            return Err(AdminRunError::ConcurrentMutation);
        }
        let mut request = self
            .kms
            .decrypt()
            .key_id(&self.kms_arn)
            .ciphertext_blob(Blob::new(record.enc));
        for (name, value) in record.custom_context {
            request = request.encryption_context(name, value);
        }
        let output = request
            .send()
            .await
            .map_err(|error| provider("KMS.Decrypt", &error))?;
        let plaintext = output.plaintext.ok_or(AdminRunError::InvalidStored {
            reason: "KMS verified no branch-key material",
        })?;
        let material = Zeroizing::new(plaintext.into_inner());
        if material.len() != 32 {
            return Err(AdminRunError::InvalidStored {
                reason: "KMS verified branch-key material with the wrong AES-256 length",
            });
        }
        Ok(())
    }
}

fn provider<E: ProvideErrorMetadata>(operation: &'static str, error: &E) -> AdminRunError {
    AdminRunError::Provider {
        operation,
        code: error.code().unwrap_or("Unknown").to_owned(),
    }
}

fn conditional_conflict(
    error: &aws_sdk_dynamodb::error::SdkError<
        aws_sdk_dynamodb::operation::transact_write_items::TransactWriteItemsError,
    >,
) -> bool {
    match error.as_service_error() {
        Some(
            aws_sdk_dynamodb::operation::transact_write_items::TransactWriteItemsError::TransactionCanceledException(cancelled),
        ) => {
            cancelled
                .cancellation_reasons()
                .iter()
                .any(|reason| reason.code() == Some("ConditionalCheckFailed"))
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use aex_wire::ids::{OperationId, PrefixedId as _, Uuid7};

    use super::*;

    fn generation() -> GenerationSpec {
        GenerationSpec {
            branch_key_id: BranchKeyId::from_stored("wsp_test"),
            version: "branch:version:op_test".to_owned(),
            hierarchy_version: 7,
            attestation: OperationId::from_uuid7(Uuid7::compose(1, [1; 10])),
            reason_digest: Some("ab".repeat(32)),
        }
    }

    #[test]
    fn persisted_context_contains_only_bounded_identity_and_reason_digest() {
        let config = aws_config::SdkConfig::builder().build();
        let admin = AwsKeyAdmin::new(
            DynamoClient::new(&config),
            KmsClient::new(&config),
            "keystore",
            "arn:aws:kms:eu-west-1:000000000000:key/root",
            "prd",
            "eu-west-1",
        );
        let generation = generation();
        let context = admin.context(&generation);
        assert_eq!(context[CONTEXT_WORKSPACE], "wsp_test");
        assert_eq!(context[CONTEXT_REASON_DIGEST].len(), 64);
        assert_eq!(context.len(), 5);
        assert!(context.values().all(|value| !value.contains("incident")));
    }

    #[test]
    fn active_and_version_rows_carry_identical_wrapped_material() {
        let config = aws_config::SdkConfig::builder().build();
        let admin = AwsKeyAdmin::new(
            DynamoClient::new(&config),
            KmsClient::new(&config),
            "keystore",
            "arn:aws:kms:eu-west-1:000000000000:key/root",
            "prd",
            "eu-west-1",
        );
        let generation = generation();
        let context = admin.context(&generation);
        let active = admin.item(&generation, ACTIVE, b"wrapped", &context, "now");
        let version = admin.item(
            &generation,
            &generation.version,
            b"wrapped",
            &context,
            "now",
        );
        assert_eq!(active[ENC], version[ENC]);
        assert_eq!(active[VERSION], version[VERSION]);
        assert_eq!(active[HIERARCHY_VERSION], version[HIERARCHY_VERSION]);
        assert_ne!(active[TYPE], version[TYPE]);
    }

    #[test]
    fn creation_and_rotation_compile_exact_atomic_fences() {
        let config = aws_config::SdkConfig::builder().build();
        let admin = AwsKeyAdmin::new(
            DynamoClient::new(&config),
            KmsClient::new(&config),
            "keystore",
            "arn:aws:kms:eu-west-1:000000000000:key/root",
            "prd",
            "eu-west-1",
        );
        let generation = generation();
        let context = admin.context(&generation);
        let (immutable, create_active) = admin
            .writes(&generation, None, b"wrapped", &context, "now")
            .expect("create writes");
        assert_eq!(
            immutable.condition_expression(),
            Some("attribute_not_exists(#branch) AND attribute_not_exists(#type)")
        );
        assert_eq!(
            create_active.condition_expression(),
            Some("attribute_not_exists(#version)")
        );

        let expected = ActiveGeneration {
            version: "branch:version:previous".to_owned(),
            hierarchy_version: 6,
            kms_arn: admin.kms_arn.clone(),
        };
        let (_, rotate_active) = admin
            .writes(&generation, Some(&expected), b"wrapped", &context, "now")
            .expect("rotation writes");
        assert_eq!(
            rotate_active.condition_expression(),
            Some("#version = :expected_version AND #hierarchy = :expected_hierarchy")
        );
        let values = rotate_active.expression_attribute_values().expect("values");
        assert_eq!(
            values[":expected_version"],
            AttributeValue::S(expected.version)
        );
        assert_eq!(
            values[":expected_hierarchy"],
            AttributeValue::N("6".to_owned())
        );
    }
}
