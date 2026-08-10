//! AWS implementation of the exclusive key-administration port.
//!
//! A KMS ciphertext is generated before the `DynamoDB` transaction. A crash in
//! that interval leaks no plaintext and creates no authority row. The two row
//! mutations (immutable version plus active pointer) then commit atomically.
//!
//! **The row shape, the encryption context and the create path are not written
//! here** (D-2). They live in `aex_secret_keystore_dynamodb::provision`, because
//! `regional-secret-api` creates a workspace's first branch key lazily on its
//! first write and two implementations of the same two rows is how a version row
//! and an active pointer eventually disagree about their authenticated context.
//! This adapter supplies the attestation, owns **rotation**, and delegates
//! everything else.

use std::collections::BTreeMap;

use aex_secret_keystore_dynamodb::branch_key::{
    self, ACTIVE, BRANCH_KEY_ID, BranchKeyId, RecordKind, TYPE,
};
use aex_secret_keystore_dynamodb::provision::{
    self, BranchKeyGeneration, EnsureOutcome, ExpectedActive, ProvisionError,
};
use aex_secret_keystore_dynamodb::store::KeyStoreBinding;
use aex_wire::types::Timestamp;
use async_trait::async_trait;
use aws_sdk_dynamodb::Client as DynamoClient;
use aws_sdk_dynamodb::types::{AttributeValue, TransactWriteItem};
use aws_sdk_kms::Client as KmsClient;
use aws_sdk_kms::primitives::Blob;
use aws_smithy_types::error::metadata::ProvideErrorMetadata;
use zeroize::Zeroizing;

use crate::admin::{ActiveGeneration, AdminRunError, ApplyOutcome, GenerationSpec, KeyAdminPort};

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

    fn binding(&self) -> KeyStoreBinding {
        KeyStoreBinding::new(self.table.clone(), self.kms_arn.clone())
    }

    fn context(&self, generation: &GenerationSpec) -> BTreeMap<String, String> {
        provision::creation_context(&self.plane, &self.region, &generation.branch_key_id)
    }

    /// The audit annotations one generation records outside its encryption
    /// context.
    ///
    /// The attestation and the rotation reason used to be encryption-context
    /// pairs. They cannot be: the request path opens this key knowing only which
    /// workspace it serves, so a context carrying an operation id would be a key
    /// nobody could open. The values are kept, as row attributes.
    fn annotations(generation: &GenerationSpec) -> BTreeMap<String, String> {
        let mut annotations = BTreeMap::from([(
            provision::ATTESTATION_ATTRIBUTE.to_owned(),
            generation.attestation.to_string(),
        )]);
        if let Some(digest) = &generation.reason_digest {
            annotations.insert(
                provision::REASON_DIGEST_ATTRIBUTE.to_owned(),
                digest.clone(),
            );
        }
        annotations
    }

    /// Projects the worker's attested spec onto the shared generation record.
    ///
    /// # Errors
    ///
    /// [`AdminRunError::InvalidStored`] when the system clock cannot produce a
    /// canonical instant, which is a broken host rather than a conflict.
    fn generation_of(&self, spec: &GenerationSpec) -> Result<BranchKeyGeneration, AdminRunError> {
        let created_at = Timestamp::from_datetime_trunc_ms(time::OffsetDateTime::now_utc())
            .map_err(|_| AdminRunError::InvalidStored {
                reason: "the system clock is outside the canonical timestamp range",
            })?
            .to_string();
        Ok(BranchKeyGeneration {
            branch_key_id: spec.branch_key_id.clone(),
            version: spec.version.clone(),
            hierarchy_version: spec.hierarchy_version,
            context: self.context(spec),
            annotations: Self::annotations(spec),
            created_at,
            transport_token: spec.attestation.to_string(),
        })
    }

    /// Commits one **rotation**, which is this worker's alone.
    ///
    /// Creation is not here: it is
    /// [`provision::ensure_active_branch_key`], shared with
    /// `regional-secret-api`.
    async fn rotate_onto(
        &self,
        spec: &GenerationSpec,
        expected: &ActiveGeneration,
    ) -> Result<ApplyOutcome, AdminRunError> {
        let binding = self.binding();
        let generation = self.generation_of(spec)?;
        let wrapped = provision::wrap_material(&self.kms, &binding, &generation.context).await?;
        let (version, active) = provision::writes(
            &binding,
            &generation,
            Some(&ExpectedActive {
                version: expected.version.clone(),
                hierarchy_version: expected.hierarchy_version,
            }),
            &wrapped,
        )?;

        let result = self
            .dynamodb
            .transact_write_items()
            .client_request_token(generation.transport_token.clone())
            .transact_items(TransactWriteItem::builder().put(version).build())
            .transact_items(TransactWriteItem::builder().put(active).build())
            .send()
            .await;
        match result {
            Ok(_) => Ok(ApplyOutcome::Applied),
            Err(error) if provision::conditional_conflict(&error) => Ok(ApplyOutcome::Conflict),
            Err(error) => Err(provider("DynamoDB.TransactWriteItems", &error)),
        }
    }
}

impl From<ProvisionError> for AdminRunError {
    fn from(error: ProvisionError) -> Self {
        match error {
            ProvisionError::Provider { operation, code } => Self::Provider { operation, code },
            ProvisionError::Unusable { reason } | ProvisionError::Malformed { reason } => {
                Self::InvalidStored { reason }
            }
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

    /// Creates a workspace's first branch key through the **one** create path.
    ///
    /// `regional-secret-api` calls the same function on a first write (D-2), so
    /// an operator-driven create and a lazy create produce byte-identical rows
    /// under a byte-identical encryption context. An already-active key is a
    /// conflict here, which the one-shot reports as the exit code for an
    /// idempotent no-op.
    async fn create(&self, generation: &GenerationSpec) -> Result<ApplyOutcome, AdminRunError> {
        if generation.hierarchy_version != 1 {
            return Err(AdminRunError::InvalidStored {
                reason: "creation did not target hierarchy generation one",
            });
        }
        let outcome = aex_secret_keystore_dynamodb::ensure_active_branch_key(
            &self.dynamodb,
            &self.kms,
            &self.binding(),
            &self.generation_of(generation)?,
        )
        .await?;
        Ok(match outcome {
            EnsureOutcome::Created(_) => ApplyOutcome::Applied,
            EnsureOutcome::AlreadyActive(_) => ApplyOutcome::Conflict,
        })
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
        self.rotate_onto(generation, expected).await
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

#[cfg(test)]
mod tests {
    use aex_secret_keystore_dynamodb::branch_key::{ENC, HIERARCHY_VERSION, VERSION};
    use aex_secret_keystore_dynamodb::provision::{CONTEXT_WORKSPACE, ExpectedActive, writes};
    use aex_wire::ids::{OperationId, PrefixedId as _, Uuid7};

    use super::*;

    fn admin() -> AwsKeyAdmin {
        let config = aws_config::SdkConfig::builder().build();
        AwsKeyAdmin::new(
            DynamoClient::new(&config),
            KmsClient::new(&config),
            "keystore",
            "arn:aws:kms:eu-west-1:000000000000:key/root",
            "prd",
            "eu-west-1",
        )
    }

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
    fn persisted_context_contains_only_the_identity_an_opener_can_reproduce() {
        let context = admin().context(&generation());
        assert_eq!(context[CONTEXT_WORKSPACE], "wsp_test");
        assert_eq!(context.len(), 3);
        assert!(context.values().all(|value| !value.contains("incident")));
    }

    /// The attestation and the reason digest survive as annotations. They left
    /// the encryption context because the request path opens this key knowing
    /// only its workspace, and a context it cannot rebuild is a key it cannot
    /// use — but a rotation with no recorded reason would be worse.
    #[test]
    fn the_attestation_and_reason_digest_are_recorded_as_annotations() {
        let annotations = AwsKeyAdmin::annotations(&generation());
        assert_eq!(
            annotations[provision::ATTESTATION_ATTRIBUTE],
            generation().attestation.to_string()
        );
        assert_eq!(annotations[provision::REASON_DIGEST_ATTRIBUTE].len(), 64);
        assert!(
            annotations
                .values()
                .all(|value| !value.contains("incident")),
            "a reason digest, never the reason"
        );
    }

    /// The worker's attested spec and the shared generation record must name the
    /// same rows: the attestation is the transport token, and the version sort
    /// key and hierarchy travel unchanged.
    #[test]
    fn the_attested_spec_projects_onto_the_shared_generation_without_reinterpretation() {
        let admin = admin();
        let spec = generation();
        let shared = admin.generation_of(&spec).expect("a canonical instant");
        assert_eq!(shared.branch_key_id, spec.branch_key_id);
        assert_eq!(shared.version, spec.version);
        assert_eq!(shared.hierarchy_version, spec.hierarchy_version);
        assert_eq!(shared.transport_token, spec.attestation.to_string());
        assert_eq!(shared.context, admin.context(&spec));
    }

    #[test]
    fn active_and_version_rows_carry_identical_wrapped_material() {
        let admin = admin();
        let shared = admin.generation_of(&generation()).expect("an instant");
        let (version, active) =
            writes(&admin.binding(), &shared, None, b"wrapped").expect("create writes");
        let active = active.item();
        let version = version.item();
        assert_eq!(active[ENC], version[ENC]);
        assert_eq!(active[VERSION], version[VERSION]);
        assert_eq!(active[HIERARCHY_VERSION], version[HIERARCHY_VERSION]);
        assert_ne!(active[TYPE], version[TYPE]);
    }

    #[test]
    fn creation_and_rotation_compile_exact_atomic_fences() {
        let admin = admin();
        let shared = admin.generation_of(&generation()).expect("an instant");
        let (immutable, create_active) =
            writes(&admin.binding(), &shared, None, b"wrapped").expect("create writes");
        assert_eq!(
            immutable.condition_expression(),
            Some("attribute_not_exists(#branch) AND attribute_not_exists(#type)")
        );
        assert_eq!(
            create_active.condition_expression(),
            Some("attribute_not_exists(#version)")
        );

        let expected = ExpectedActive {
            version: "branch:version:previous".to_owned(),
            hierarchy_version: 6,
        };
        let (_, rotate_active) = writes(&admin.binding(), &shared, Some(&expected), b"wrapped")
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
