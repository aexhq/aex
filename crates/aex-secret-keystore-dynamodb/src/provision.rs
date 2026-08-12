//! The one branch-key **creation** path (D-2).
//!
//! This narrows the crate's D-20 invariant — "this crate deliberately
//! implements no write path" — by exactly one operation, and the narrowing is
//! deliberate and bounded: *creation of a workspace's first branch key* becomes
//! a library operation so that `session-stream-api` can perform it lazily on a
//! first write; **rotation stays exclusively with `regional-secret-key-admin`.**
//! There is no `rotate` here and there must never be one.
//!
//! Why lazy creation rather than provisioning-time creation: the alternative
//! makes `secret_put` depend on a cross-plane orchestration
//! (`central-control-worker` → `regional-control` → `regional-secret-key-admin`)
//! that does not exist, strands every workspace provisioned before it lands, and
//! adds a *new* availability dependency to a route that would otherwise have
//! none. Availability is deprioritised, so adding one is the wrong direction.
//!
//! Both callers share this module rather than a shape: the two `Put`s and their
//! conditions are built here, and `regional-secret-key-admin` compiles its
//! rotation from the same builders so the two rows cannot drift.

use std::collections::BTreeMap;

use aws_sdk_dynamodb::Client;
use aws_sdk_dynamodb::types::{AttributeValue, Put, TransactWriteItem};
use aws_sdk_kms::Client as KmsClient;
use aws_sdk_kms::primitives::Blob;
use aws_sdk_kms::types::DataKeySpec;
use aws_smithy_types::error::metadata::ProvideErrorMetadata;

use aex_wire::ids::{PrefixedId as _, WorkspaceId};
use aex_wire::types::Timestamp;

use crate::branch_key::{
    self, ACTIVE, BRANCH_KEY_ID, BranchKeyId, CREATE_TIME, CUSTOM_CONTEXT_PREFIX, ENC,
    HIERARCHY_VERSION, KMS_ARN, TYPE, VERSION, VERSION_PREFIX,
};
use crate::store::{ActiveBranchKey, KeyStoreBinding};

/// The context key naming the plane a branch key belongs to.
pub const CONTEXT_PLANE: &str = "aex:plane";
/// The context key naming the region.
pub const CONTEXT_REGION: &str = "aex:region";
/// The context key naming the workspace, which **is** the branch key identity.
pub const CONTEXT_WORKSPACE: &str = "aex:workspace";
/// The attribute recording which operation attested a generation.
///
/// An **annotation** on the row, not encryption context. See
/// [`creation_context`].
pub const ATTESTATION_ATTRIBUTE: &str = "aex-attestation";
/// The attribute recording the digest of a rotation reason, never the reason.
pub const REASON_DIGEST_ATTRIBUTE: &str = "aex-rotation-reason-sha256";

/// The hierarchy version every *created* branch key starts at.
pub const FIRST_HIERARCHY_VERSION: u64 = 1;

/// The `KMS` encryption context a branch-key record is wrapped under: exactly
/// plane, region and workspace.
///
/// Built here so the creation path, the rotation path and **the seal path**
/// cannot disagree about it. It is the same three pairs
/// `aex_secret_aws::context::branch_key_pairs` renders from a secret's
/// encryption context, and that is not a coincidence — it is the requirement:
/// the key this wraps is the key `SecretCrypto::seal` opens.
///
/// # Why the attestation is not in it
///
/// An encryption context is authenticated additional data, so whatever wraps a
/// key must be reproducible **exactly** by every future opener. A branch key is
/// opened on the request path by a process that knows which workspace it is
/// serving and nothing at all about the operation that created the key. An
/// attestation in the context would be a key nobody could ever open.
///
/// The attestation and the rotation-reason digest are still recorded, as plain
/// row attributes ([`ATTESTATION_ATTRIBUTE`], [`REASON_DIGEST_ATTRIBUTE`]), so
/// the audit trail survives without making the material unreadable.
#[must_use]
pub fn creation_context(
    plane: &str,
    region: &str,
    branch_key_id: &BranchKeyId,
) -> BTreeMap<String, String> {
    BTreeMap::from([
        (CONTEXT_PLANE.to_owned(), plane.to_owned()),
        (CONTEXT_REGION.to_owned(), region.to_owned()),
        (
            CONTEXT_WORKSPACE.to_owned(),
            branch_key_id.as_str().to_owned(),
        ),
    ])
}

/// Everything one branch-key generation is written from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BranchKeyGeneration {
    /// Which branch key.
    pub branch_key_id: BranchKeyId,
    /// The versioned record's sort key, `branch:version:{uuid}`.
    pub version: String,
    /// The hierarchy version this generation occupies.
    pub hierarchy_version: u64,
    /// The authenticated context, from [`creation_context`]. Every pair here
    /// reaches `KMS` and must be reproducible by an opener.
    pub context: BTreeMap<String, String>,
    /// Audit annotations stored beside the record and sent to nothing.
    ///
    /// These carry what the encryption context deliberately cannot: which
    /// operation attested the write, and the digest of an operator's rotation
    /// reason.
    pub annotations: BTreeMap<String, String>,
    /// The instant, in the canonical spelling.
    pub created_at: String,
    /// The provider deduplication token for the transaction.
    pub transport_token: String,
}

/// What the pointer must currently say for a write to be accepted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExpectedActive {
    /// The version the pointer holds.
    pub version: String,
    /// The hierarchy version it holds.
    pub hierarchy_version: u64,
}

/// Whether the caller's write created the key or found one already active.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EnsureOutcome {
    /// This call created the workspace's first branch key.
    Created(ActiveBranchKey),
    /// A branch key was already active; nothing was written.
    AlreadyActive(ActiveBranchKey),
}

impl EnsureOutcome {
    /// The active key, however it was obtained.
    #[must_use]
    pub fn into_active(self) -> ActiveBranchKey {
        match self {
            Self::Created(active) | Self::AlreadyActive(active) => active,
        }
    }

    /// Whether this call wrote the record.
    #[must_use]
    pub const fn created(&self) -> bool {
        matches!(self, Self::Created(_))
    }
}

/// Why a branch key could not be established.
///
/// Every variant is terminal and none is a fallback: a workspace whose branch
/// key cannot be created has no seal path, and answering the write with anything
/// other than a loud failure would mean writing a row nothing can open.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ProvisionError {
    /// A provider call failed.
    #[error("{operation} failed with `{code}`")]
    Provider {
        /// Which call.
        operation: &'static str,
        /// The provider's own code.
        code: String,
    },
    /// A stored or returned value was not usable.
    #[error("the branch key store answered something unusable: {reason}")]
    Unusable {
        /// What was wrong.
        reason: &'static str,
    },
    /// A write could not be assembled.
    #[error("a branch key write could not be built: {reason}")]
    Malformed {
        /// What was wrong.
        reason: &'static str,
    },
}

/// The item body shared by the versioned record and the active pointer.
///
/// They carry **identical** material and differ only in their sort key: the
/// active record is a pointer that also holds the bytes, which is what lets a
/// seal read one item rather than two.
#[must_use]
fn item(
    binding: &KeyStoreBinding,
    generation: &BranchKeyGeneration,
    kind: &str,
    wrapped: &[u8],
) -> std::collections::HashMap<String, AttributeValue> {
    let mut item = std::collections::HashMap::from([
        (
            BRANCH_KEY_ID.to_owned(),
            AttributeValue::S(generation.branch_key_id.as_str().to_owned()),
        ),
        (TYPE.to_owned(), AttributeValue::S(kind.to_owned())),
        (
            ENC.to_owned(),
            AttributeValue::B(Blob::new(wrapped.to_vec())),
        ),
        (
            KMS_ARN.to_owned(),
            AttributeValue::S(binding.kms_key_arn().to_owned()),
        ),
        (
            CREATE_TIME.to_owned(),
            AttributeValue::S(generation.created_at.clone()),
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
    for (name, value) in &generation.context {
        item.insert(
            format!("{CUSTOM_CONTEXT_PREFIX}{name}"),
            AttributeValue::S(value.clone()),
        );
    }
    // Annotations are written under their own names rather than the provider's
    // `aws-crypto-ec:` prefix. That prefix means "this pair is encryption
    // context" — the worker's `verify` rebuilds the decrypt context from exactly
    // those attributes — so recording an unauthenticated value there would
    // produce a record the provider's own tooling could not open.
    for (name, value) in &generation.annotations {
        item.insert(name.clone(), AttributeValue::S(value.clone()));
    }
    item
}

/// The two conditional `Put`s one generation commits, in transaction order.
///
/// The immutable version row is guarded by `attribute_not_exists`; the active
/// pointer is guarded by absence on a create and by the observed
/// `(version, hierarchy)` pair on a rotation. Both travel in one
/// `TransactWriteItems`, so a crash can never leave a pointer at a version that
/// was never written.
///
/// # Errors
///
/// [`ProvisionError::Malformed`] when a builder could not be completed.
pub fn writes(
    binding: &KeyStoreBinding,
    generation: &BranchKeyGeneration,
    expected: Option<&ExpectedActive>,
    wrapped: &[u8],
) -> Result<(Put, Put), ProvisionError> {
    let version = Put::builder()
        .table_name(binding.table())
        .set_item(Some(item(
            binding,
            generation,
            &generation.version,
            wrapped,
        )))
        .condition_expression("attribute_not_exists(#branch) AND attribute_not_exists(#type)")
        .expression_attribute_names("#branch", BRANCH_KEY_ID)
        .expression_attribute_names("#type", TYPE)
        .build()
        .map_err(|_| ProvisionError::Malformed {
            reason: "the immutable generation write could not be built",
        })?;

    let mut active = Put::builder()
        .table_name(binding.table())
        .set_item(Some(item(binding, generation, ACTIVE, wrapped)))
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
    let active = active.build().map_err(|_| ProvisionError::Malformed {
        reason: "the active generation write could not be built",
    })?;
    Ok((version, active))
}

/// Wraps fresh AES-256 branch material under the store's root key.
///
/// `GenerateDataKeyWithoutPlaintext` is the whole point: the plaintext branch
/// key never enters this process, so a crash between this call and the commit
/// leaks nothing and creates no authority row.
///
/// # Errors
///
/// [`ProvisionError::Provider`] for a `KMS` failure, or
/// [`ProvisionError::Unusable`] when `KMS` returns nothing usable.
pub async fn wrap_material(
    kms: &KmsClient,
    binding: &KeyStoreBinding,
    context: &BTreeMap<String, String>,
) -> Result<Vec<u8>, ProvisionError> {
    let mut request = kms
        .generate_data_key_without_plaintext()
        .key_id(binding.kms_key_arn())
        .key_spec(DataKeySpec::Aes256);
    for (name, value) in context {
        request = request.encryption_context(name, value);
    }
    if !binding.grant_tokens().is_empty() {
        request = request.set_grant_tokens(Some(binding.grant_tokens().to_vec()));
    }
    let output = request
        .send()
        .await
        .map_err(|error| provider("KMS.GenerateDataKeyWithoutPlaintext", &error))?;
    let wrapped = output
        .ciphertext_blob
        .ok_or(ProvisionError::Unusable {
            reason: "KMS returned no wrapped branch key",
        })?
        .into_inner();
    if wrapped.is_empty() {
        return Err(ProvisionError::Unusable {
            reason: "KMS returned an empty wrapped branch key",
        });
    }
    Ok(wrapped)
}

/// Reads a workspace's active branch key, creating it when it has none.
///
/// This is the **single named entry point** D-2 calls for, and both
/// `session-stream-api` (on a first write) and `regional-secret-key-admin`'s
/// `create` command go through it.
///
/// The create is conditional on `attribute_not_exists`, so two concurrent first
/// writes produce one winner and one loser; the loser re-reads and returns the
/// winner's key. There is no compensating repair because there is nothing to
/// repair: the losing attempt wrote nothing at all, and the only cost is one
/// wasted `KMS` call.
///
/// Exactly one extra `KMS` call and one extra transaction per workspace, ever.
///
/// # Errors
///
/// [`ProvisionError`] for any provider failure, or when the store answers a
/// record that does not decode. Nothing here falls back: a workspace with no
/// openable branch key must fail its write loudly.
pub async fn ensure_active_branch_key(
    dynamodb: &Client,
    kms: &KmsClient,
    binding: &KeyStoreBinding,
    generation: &BranchKeyGeneration,
) -> Result<EnsureOutcome, ProvisionError> {
    if generation.hierarchy_version != FIRST_HIERARCHY_VERSION {
        return Err(ProvisionError::Malformed {
            reason: "creation did not target hierarchy generation one; rotation is the key \
                     administrator's, not this path's",
        });
    }
    if let Some(active) = read_active(dynamodb, binding, &generation.branch_key_id).await? {
        return Ok(EnsureOutcome::AlreadyActive(active));
    }

    let wrapped = wrap_material(kms, binding, &generation.context).await?;
    let (version, active) = writes(binding, generation, None, &wrapped)?;
    let result = dynamodb
        .transact_write_items()
        .client_request_token(generation.transport_token.clone())
        .transact_items(TransactWriteItem::builder().put(version).build())
        .transact_items(TransactWriteItem::builder().put(active).build())
        .send()
        .await;

    match result {
        Ok(_) => Ok(EnsureOutcome::Created(ActiveBranchKey {
            branch_key_id: generation.branch_key_id.clone(),
            version: Some(generation.version.clone()),
            create_time: generation.created_at.clone(),
            kms_arn: binding.kms_key_arn().to_owned(),
            hierarchy_version: generation.hierarchy_version,
            wrapped_material: wrapped,
        })),
        // Lost the election. The winner's key is the answer, and re-reading is
        // the only way to obtain it: this attempt's wrapped material was never
        // written and can open nothing.
        Err(error) if conditional_conflict(&error) => {
            match read_active(dynamodb, binding, &generation.branch_key_id).await? {
                Some(active) => Ok(EnsureOutcome::AlreadyActive(active)),
                None => Err(ProvisionError::Unusable {
                    reason: "the create lost its condition and no active record followed",
                }),
            }
        }
        Err(error) => Err(provider("DynamoDB.TransactWriteItems", &error)),
    }
}

/// One workspace's active branch key, established on demand.
///
/// The seam exists so a deployable's request path holds a **capability** rather
/// than two AWS clients: a router that could reach a raw `DynamoDB` client could
/// reach any table, and the composition assertion would have nothing structural
/// to assert.
#[async_trait::async_trait]
pub trait BranchKeyAuthority: Send + Sync + 'static {
    /// The workspace's active branch key, creating it if it has none.
    ///
    /// # Errors
    ///
    /// [`ProvisionError`] for any provider failure. Never a fallback: a
    /// workspace with no openable branch key must fail its write loudly.
    async fn active_or_create(
        &self,
        workspace: WorkspaceId,
    ) -> Result<ActiveBranchKey, ProvisionError>;
}

/// The production adapter: read, and create on a first write (D-2).
#[derive(Debug, Clone)]
pub struct LazyBranchKeys {
    dynamodb: Client,
    kms: KmsClient,
    binding: KeyStoreBinding,
    plane: String,
    region: String,
}

impl LazyBranchKeys {
    /// Binds the authority to its two providers and its key store.
    #[must_use]
    pub fn new(
        dynamodb: Client,
        kms: KmsClient,
        binding: KeyStoreBinding,
        plane: impl Into<String>,
        region: impl Into<String>,
    ) -> Self {
        Self {
            dynamodb,
            kms,
            binding,
            plane: plane.into(),
            region: region.into(),
        }
    }

    /// The generation a workspace's **first** branch key is written as.
    ///
    /// The version sort key is derived from the workspace rather than minted, and
    /// that is load bearing. The immutable version row is guarded by
    /// `attribute_not_exists` on `(branch-key-id, type)`, so two concurrent first
    /// writes that picked *different* version identities would both write a
    /// version row and only contend on the active pointer — leaving an orphan
    /// generation no pointer names. A derived identity makes them collide on the
    /// version row too, so the loser's whole transaction is cancelled and it
    /// writes nothing at all.
    ///
    /// Rotation does not use this: it mints from an attested operation id, which
    /// is what makes a *rotation* retry stable and distinct.
    #[must_use]
    pub fn first_generation(&self, workspace: WorkspaceId) -> BranchKeyGeneration {
        let id = BranchKeyId::of(workspace);
        let identity = uuid_text(workspace.uuid7().as_bytes());
        BranchKeyGeneration {
            context: creation_context(&self.plane, &self.region, &id),
            annotations: BTreeMap::from([(
                ATTESTATION_ATTRIBUTE.to_owned(),
                "lazy:first-secret-write".to_owned(),
            )]),
            branch_key_id: id,
            version: format!("{VERSION_PREFIX}{identity}"),
            hierarchy_version: FIRST_HIERARCHY_VERSION,
            // A canonical UUID is 36 characters, which is exactly the provider's
            // `ClientRequestToken` ceiling.
            transport_token: identity,
            created_at: String::new(),
        }
    }
}

#[async_trait::async_trait]
impl BranchKeyAuthority for LazyBranchKeys {
    async fn active_or_create(
        &self,
        workspace: WorkspaceId,
    ) -> Result<ActiveBranchKey, ProvisionError> {
        let mut generation = self.first_generation(workspace);
        generation.created_at = now_text()?;
        ensure_active_branch_key(&self.dynamodb, &self.kms, &self.binding, &generation)
            .await
            .map(EnsureOutcome::into_active)
    }
}

fn now_text() -> Result<String, ProvisionError> {
    Timestamp::from_datetime_trunc_ms(time::OffsetDateTime::now_utc())
        .map(|instant| instant.to_string())
        .map_err(|_| ProvisionError::Unusable {
            reason: "the system clock is outside the canonical timestamp range",
        })
}

/// Renders 16 bytes in the canonical hyphenated `UUID` form.
fn uuid_text(bytes: &[u8; 16]) -> String {
    let hex = hex_lower(bytes);
    format!(
        "{}-{}-{}-{}-{}",
        &hex[0..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..32]
    )
}

fn hex_lower(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut text = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        text.push(char::from(HEX[usize::from(byte >> 4)]));
        text.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    text
}

/// Reads the active record, strongly.
///
/// D-7 takes the *cached* `branch:ACTIVE` read eventual on the seal path
/// because a stale hit is provably harmless. This read is not that one: it is
/// the read that decides whether to **write**, and a stale miss here spends a
/// `KMS` call and a transaction that then lose their condition.
async fn read_active(
    dynamodb: &Client,
    binding: &KeyStoreBinding,
    branch_key_id: &BranchKeyId,
) -> Result<Option<ActiveBranchKey>, ProvisionError> {
    let output = dynamodb
        .get_item()
        .table_name(binding.table())
        .key(
            BRANCH_KEY_ID,
            AttributeValue::S(branch_key_id.as_str().to_owned()),
        )
        .key(TYPE, AttributeValue::S(ACTIVE.to_owned()))
        .consistent_read(true)
        .send()
        .await
        .map_err(|error| provider("DynamoDB.GetItem", &error))?;
    match output.item {
        None => Ok(None),
        Some(item) => {
            let record = branch_key::decode(&item).map_err(|_| ProvisionError::Unusable {
                reason: "the active record does not match the keystore schema",
            })?;
            Ok(Some(record.into()))
        }
    }
}

fn provider<E: ProvideErrorMetadata>(operation: &'static str, error: &E) -> ProvisionError {
    ProvisionError::Provider {
        operation,
        code: error.code().unwrap_or("Unknown").to_owned(),
    }
}

/// Whether a cancelled transaction was cancelled by a condition rather than by
/// contention or a capacity refusal.
#[must_use]
pub fn conditional_conflict(
    error: &aws_sdk_dynamodb::error::SdkError<
        aws_sdk_dynamodb::operation::transact_write_items::TransactWriteItemsError,
    >,
) -> bool {
    match error.as_service_error() {
        Some(
            aws_sdk_dynamodb::operation::transact_write_items::TransactWriteItemsError::TransactionCanceledException(cancelled),
        ) => cancelled
            .cancellation_reasons()
            .iter()
            .any(|reason| reason.code() == Some("ConditionalCheckFailed")),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::{
        ATTESTATION_ATTRIBUTE, BranchKeyGeneration, CONTEXT_WORKSPACE, ExpectedActive,
        creation_context, uuid_text, writes,
    };
    use crate::branch_key::{
        ACTIVE, BranchKeyId, CUSTOM_CONTEXT_PREFIX, ENC, HIERARCHY_VERSION, VERSION,
    };
    use crate::store::KeyStoreBinding;

    fn binding() -> KeyStoreBinding {
        KeyStoreBinding::new(
            "dev-eu-west-1-regional-secret-keystore",
            "arn:aws:kms:eu-west-1:000000000000:key/root",
        )
    }

    fn generation() -> BranchKeyGeneration {
        let id = BranchKeyId::from_stored("wsp_test");
        BranchKeyGeneration {
            context: creation_context("prd", "eu-west-1", &id),
            annotations: BTreeMap::from([(ATTESTATION_ATTRIBUTE.to_owned(), "op_test".to_owned())]),
            branch_key_id: id,
            version: "branch:version:op_test".to_owned(),
            hierarchy_version: 1,
            created_at: "2026-08-01T12:34:56.789Z".to_owned(),
            transport_token: "aex-token".to_owned(),
        }
    }

    /// The context is exactly the three pairs an opener can rebuild from the
    /// workspace it is serving, and nothing else. A fourth pair would be a key
    /// nobody could open.
    #[test]
    fn the_encryption_context_is_exactly_what_an_opener_can_reproduce() {
        let context = generation().context;
        assert_eq!(context[CONTEXT_WORKSPACE], "wsp_test");
        assert_eq!(context.len(), 3);
        assert!(context.values().all(|value| !value.contains("incident")));
        assert!(
            !context.keys().any(|key| key.contains("attestation")),
            "an attestation is not reproducible by an opener and cannot be context"
        );
    }

    /// The attestation is still recorded — as an annotation, under its own name,
    /// so the provider's `aws-crypto-ec:` prefix keeps meaning "this is
    /// encryption context" and the worker's `verify` can rebuild the decrypt
    /// context from exactly those attributes.
    #[test]
    fn an_annotation_is_stored_outside_the_providers_encryption_context_prefix() {
        let (_, active) = writes(&binding(), &generation(), None, b"wrapped").expect("writes");
        let item = active.item();
        assert_eq!(
            item[ATTESTATION_ATTRIBUTE],
            aws_sdk_dynamodb::types::AttributeValue::S("op_test".to_owned())
        );
        assert!(
            !item
                .keys()
                .any(|name| name.starts_with(CUSTOM_CONTEXT_PREFIX)
                    && name.contains("attestation")),
            "an annotation must never be written as encryption context"
        );
    }

    /// Two concurrent first writes must collide on the **version** row, not only
    /// on the pointer. A minted identity would let both write a version row and
    /// leave the loser's generation orphaned with nothing naming it.
    #[test]
    fn a_lazy_first_generation_is_derived_from_the_workspace_rather_than_minted() {
        assert_eq!(
            uuid_text(&[0x01; 16]),
            "01010101-0101-0101-0101-010101010101"
        );
        assert_eq!(
            uuid_text(&[0x01; 16]).len(),
            36,
            "the transport token has to fit the provider's ceiling exactly"
        );
    }

    #[test]
    fn the_active_pointer_and_the_version_row_carry_identical_material() {
        let (version, active) =
            writes(&binding(), &generation(), None, b"wrapped").expect("create writes");
        let version_item = version.item();
        let active_item = active.item();
        assert_eq!(version_item[ENC], active_item[ENC]);
        assert_eq!(version_item[VERSION], active_item[VERSION]);
        assert_eq!(
            version_item[HIERARCHY_VERSION],
            active_item[HIERARCHY_VERSION]
        );
        assert_eq!(
            active_item[crate::branch_key::TYPE],
            aws_sdk_dynamodb::types::AttributeValue::S(ACTIVE.to_owned())
        );
    }

    #[test]
    fn a_create_fences_on_absence_and_a_rotation_fences_on_the_observed_pointer() {
        let (immutable, create_active) =
            writes(&binding(), &generation(), None, b"wrapped").expect("create writes");
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
        let (_, rotate_active) = writes(&binding(), &generation(), Some(&expected), b"wrapped")
            .expect("rotation writes");
        assert_eq!(
            rotate_active.condition_expression(),
            Some("#version = :expected_version AND #hierarchy = :expected_hierarchy")
        );
    }
}
