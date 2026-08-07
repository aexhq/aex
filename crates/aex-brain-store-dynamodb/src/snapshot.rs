//! Durable selection plan for verified fold snapshots.
//!
//! The immutable body is deliberately absent from this adapter: it must be published and
//! fetched through the regional content authority. This module owns only the Brain pointer
//! and the atomic proof that its absorbed `(sequence, hash)` exists in the authoritative
//! append-only journal. Keeping this compiler usable before the body adapter exists lets
//! production wiring fail closed without weakening the transaction contract.

use std::collections::BTreeMap;
use std::sync::Arc;

use aex_brain_app::ports::{BoxFuture, FoldSnapshotStore, SnapshotPublishOutcome, StoreError};
use aex_brain_domain::snapshot::{
    FOLD_SNAPSHOT_SCHEMA, FoldSnapshotArtifact, FoldSnapshotPointer, MAX_FOLD_SNAPSHOT_BYTES,
};
use aex_content_aws::object_store::{
    ContentObjectStore, ObjectHead, PutImmutable, S3ContentObjects,
};
use aex_content_aws::{ContentObjectError, ObjectKey};
use aex_session_dynamodb::attr::{CodecError, Item, ItemBuilder, Row, n, s};
use aex_session_dynamodb::plan::{TransactionPlan, key};
use aex_wire::ids::{ContentHash as BodyDigest, WorkspaceId};
use aex_wire::types::Region;
use async_trait::async_trait;
use aws_sdk_dynamodb::Client;

use crate::keys;
use crate::plan::{BrainTables, PlanError, participant};
use crate::translate;

/// The `itemType` of the selected fold-snapshot pointer.
pub const FOLD_SNAPSHOT_POINTER: &str = "brain_fold_snapshot_pointer";

const SNAPSHOT_CONTENT_DOMAIN: &str = "brain-fold-snapshot";

/// Process-scoped deployment facts used to build a workspace-scoped SSE-KMS context.
///
/// Tenant identity is deliberately absent. [`WorkspaceId`] is supplied to each body
/// operation from the session authority claimed for that activation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SnapshotContentContext {
    plane: String,
    region: Region,
}

impl SnapshotContentContext {
    /// Validates the deployment facts used for snapshot content encryption.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::Transport`] for an unsupported plane or region because either
    /// means the runtime composition does not name a real strict-v1 plane.
    pub fn new(plane: &str, region: &str) -> Result<Self, StoreError> {
        if !matches!(plane, "dev" | "prd") {
            return Err(composition_error(
                "snapshot content plane must be `dev` or `prd`",
            ));
        }
        let region = Region::from_name(region)
            .ok_or_else(|| composition_error("snapshot content region is not supported"))?;
        Ok(Self {
            plane: plane.to_owned(),
            region,
        })
    }

    fn encryption_context(&self, workspace: WorkspaceId) -> Result<Vec<u8>, StoreError> {
        let pairs = BTreeMap::from([
            ("aex:domain", SNAPSHOT_CONTENT_DOMAIN.to_owned()),
            ("aex:plane", self.plane.clone()),
            ("aex:region", self.region.as_str().to_owned()),
            ("aex:workspace", workspace.to_string()),
        ]);
        aex_wire::to_jcs_bytes(&pairs).map_err(|_| {
            composition_error("snapshot content encryption context could not be encoded")
        })
    }
}

/// The narrow regional-content seam the fold-snapshot adapter consumes.
///
/// `aex-content-aws` remains the physical S3 authority. This seam exists so the Brain
/// adapter can assert ordering and failure mapping without faking `DynamoDB` or every
/// unrelated multipart/grant operation on [`ContentObjectStore`].
#[async_trait]
pub trait SnapshotBodyStore: Send + Sync + 'static {
    /// Places one immutable workspace-scoped body.
    async fn publish_immutable(
        &self,
        workspace: WorkspaceId,
        digest: BodyDigest,
        body: Vec<u8>,
        encryption_context: Vec<u8>,
    ) -> Result<(), ContentObjectError>;

    /// Reads one immutable body without collecting a provider stream above `max_bytes`.
    async fn read_bounded(
        &self,
        workspace: WorkspaceId,
        digest: BodyDigest,
        max_bytes: u64,
    ) -> Result<Vec<u8>, ContentObjectError>;
}

#[async_trait]
impl SnapshotBodyStore for S3ContentObjects {
    async fn publish_immutable(
        &self,
        workspace: WorkspaceId,
        digest: BodyDigest,
        body: Vec<u8>,
        encryption_context: Vec<u8>,
    ) -> Result<(), ContentObjectError> {
        let body_bytes = u64::try_from(body.len()).map_err(|_| ContentObjectError::Invalid {
            detail: "the snapshot body cannot be described as u64 bytes".to_owned(),
        })?;
        match self
            .put_immutable(PutImmutable {
                workspace,
                digest: &digest,
                plaintext_bytes: body_bytes,
                body,
                encryption_context: &encryption_context,
            })
            .await
        {
            Ok(_) => Ok(()),
            Err(ContentObjectError::CommitAmbiguous { .. }) => {
                let key = ObjectKey::new(workspace, &digest);
                let head = self.head(&key).await?;
                verify_committed_body(&head, digest, body_bytes)
            }
            Err(error) => Err(error),
        }
    }

    async fn read_bounded(
        &self,
        workspace: WorkspaceId,
        digest: BodyDigest,
        max_bytes: u64,
    ) -> Result<Vec<u8>, ContentObjectError> {
        ContentObjectStore::read_bounded(self, workspace, &digest, max_bytes)
            .await
            .map(|object| object.body)
    }
}

fn verify_committed_body(
    head: &ObjectHead,
    digest: BodyDigest,
    expected_bytes: u64,
) -> Result<(), ContentObjectError> {
    let expected_digest = digest.to_wire();
    let expected_digest = expected_digest
        .strip_prefix("sha256:")
        .unwrap_or(expected_digest.as_str());
    if head.content_length == expected_bytes
        && head.declared_digest.as_deref() == Some(expected_digest)
    {
        Ok(())
    } else {
        Err(ContentObjectError::IntegrityMismatch {
            detail: "an ambiguous snapshot put did not resolve to the expected object".to_owned(),
        })
    }
}

/// Production fold-snapshot adapter over `DynamoDB` pointer selection and regional S3 content.
#[derive(Clone)]
pub struct AwsFoldSnapshotStore {
    dynamo: Client,
    tables: BrainTables,
    bodies: Arc<dyn SnapshotBodyStore>,
    context: SnapshotContentContext,
}

impl core::fmt::Debug for AwsFoldSnapshotStore {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("AwsFoldSnapshotStore")
            .field("tables", &self.tables)
            .field("context", &self.context)
            .finish_non_exhaustive()
    }
}

impl AwsFoldSnapshotStore {
    /// Binds the production regional-content implementation.
    #[must_use]
    pub fn new(
        dynamo: Client,
        tables: BrainTables,
        bodies: S3ContentObjects,
        context: SnapshotContentContext,
    ) -> Self {
        Self::with_body_store(dynamo, tables, Arc::new(bodies), context)
    }

    /// Binds the same adapter to the narrow body seam used by deterministic tests.
    #[must_use]
    pub fn with_body_store(
        dynamo: Client,
        tables: BrainTables,
        bodies: Arc<dyn SnapshotBodyStore>,
        context: SnapshotContentContext,
    ) -> Self {
        Self {
            dynamo,
            tables,
            bodies,
            context,
        }
    }

    async fn latest(
        &self,
        agent_key: aex_brain_domain::ids::AgentKey,
    ) -> Result<Option<FoldSnapshotPointer>, StoreError> {
        let selected =
            keys::fold_snapshot(&agent_key).map_err(|error| StoreError::Undecodable {
                location: "fold snapshot pointer key".to_owned(),
                reason: error.to_string(),
            })?;
        let output = self
            .dynamo
            .get_item()
            .table_name(&self.tables.session_authority)
            .set_key(Some(key(&selected.pk, &selected.sk)))
            .consistent_read(true)
            .send()
            .await
            .map_err(|error| crate::journal::transport("load_fold_snapshot_pointer", &error))?;
        output
            .item
            .map(|item| {
                decode_pointer(agent_key, &item).map_err(|error| StoreError::Undecodable {
                    location: "fold snapshot pointer".to_owned(),
                    reason: error.to_string(),
                })
            })
            .transpose()
    }
}

impl FoldSnapshotStore for AwsFoldSnapshotStore {
    fn load_latest<'a>(
        &'a self,
        agent_key: &'a aex_brain_domain::ids::AgentKey,
    ) -> BoxFuture<'a, Result<Option<FoldSnapshotPointer>, StoreError>> {
        Box::pin(async move { self.latest(*agent_key).await })
    }

    fn load_body<'a>(
        &'a self,
        workspace: WorkspaceId,
        pointer: &'a FoldSnapshotPointer,
        max_bytes: usize,
    ) -> BoxFuture<'a, Result<Vec<u8>, StoreError>> {
        Box::pin(async move {
            let max_bytes = max_bytes.min(MAX_FOLD_SNAPSHOT_BYTES);
            if pointer.body_bytes > u64::try_from(max_bytes).unwrap_or(u64::MAX) {
                return Err(StoreError::SnapshotBodyTooLarge {
                    declared: pointer.body_bytes,
                    max: max_bytes,
                });
            }
            let body = self
                .bodies
                .read_bounded(workspace, pointer.body_digest, pointer.body_bytes)
                .await
                .map_err(|error| map_content_read(error, pointer, max_bytes))?;
            if u64::try_from(body.len()).ok() != Some(pointer.body_bytes) {
                return Err(StoreError::SnapshotRejected {
                    diagnostic: aex_brain_app::ports::SnapshotDiagnostic::Rejected(
                        aex_brain_domain::snapshot::FoldSnapshotError::BodyLengthMismatch {
                            declared: pointer.body_bytes,
                            actual: body.len(),
                        },
                    ),
                });
            }
            if BodyDigest::of(&body) != pointer.body_digest {
                return Err(StoreError::SnapshotRejected {
                    diagnostic: aex_brain_app::ports::SnapshotDiagnostic::Rejected(
                        aex_brain_domain::snapshot::FoldSnapshotError::BodyDigestMismatch,
                    ),
                });
            }
            Ok(body)
        })
    }

    fn publish<'a>(
        &'a self,
        workspace: WorkspaceId,
        artifact: &'a FoldSnapshotArtifact,
    ) -> BoxFuture<'a, Result<SnapshotPublishOutcome, StoreError>> {
        Box::pin(async move {
            let pointer = artifact.pointer();
            validate_artifact(artifact)?;
            let previous = self.latest(pointer.agent).await?;
            if let Some(outcome) = selected_outcome(pointer, previous.as_ref())? {
                return Ok(outcome);
            }

            let encryption_context = self.context.encryption_context(workspace)?;
            self.bodies
                .publish_immutable(
                    workspace,
                    pointer.body_digest,
                    artifact.body().to_vec(),
                    encryption_context,
                )
                .await
                .map_err(map_content_write)?;

            let publication = compile_publication(&self.tables, previous.as_ref(), pointer)
                .map_err(map_plan_error)?;
            let participants = publication.participants().to_vec();
            let request = publication
                .compile(&self.dynamo)
                .map_err(|error| map_shared_error(&error))?;
            match request.send().await {
                Ok(_) if previous.as_ref() == Some(pointer) => {
                    Ok(SnapshotPublishOutcome::AlreadyCurrent)
                }
                Ok(_) => Ok(SnapshotPublishOutcome::Published),
                Err(error) => {
                    let mapped = match error.as_service_error() {
                        Some(service) => {
                            aex_session_dynamodb::error::decode_cancellation(service, &participants)
                        }
                        None => aex_session_dynamodb::error::classify(
                            &error,
                            aex_session_dynamodb::error::Idempotence::Write(
                                aex_session_dynamodb::error::Resolution::TargetItem,
                            ),
                        ),
                    };
                    self.resolve_publication_failure(pointer, previous.as_ref(), mapped)
                        .await
                }
            }
        })
    }
}

impl AwsFoldSnapshotStore {
    async fn resolve_publication_failure(
        &self,
        proposed: &FoldSnapshotPointer,
        previous: Option<&FoldSnapshotPointer>,
        error: aex_session_dynamodb::error::StoreError,
    ) -> Result<SnapshotPublishOutcome, StoreError> {
        use aex_session_dynamodb::error::StoreError as Shared;
        match error {
            Shared::PreconditionFailed { participant, .. }
                if participant == participant::FOLD_SNAPSHOT_JOURNAL_POINT
                    || participant == participant::FOLD_SNAPSHOT_CONTROL =>
            {
                Err(StoreError::SnapshotHistoricalMismatch)
            }
            Shared::PreconditionFailed { participant, .. }
                if participant == participant::FOLD_SNAPSHOT_POINTER =>
            {
                let selected = self.latest(proposed.agent).await?;
                resolve_changed_pointer(proposed, previous, selected.as_ref(), false)
            }
            Shared::CommitAmbiguous { .. } => {
                let selected = self.latest(proposed.agent).await?;
                resolve_changed_pointer(proposed, previous, selected.as_ref(), true)
            }
            other => Err(map_shared_error(&other)),
        }
    }
}

fn validate_artifact(artifact: &FoldSnapshotArtifact) -> Result<(), StoreError> {
    let pointer = artifact.pointer();
    if usize::try_from(pointer.body_bytes).ok() != Some(artifact.body().len()) {
        return Err(StoreError::SnapshotRejected {
            diagnostic: aex_brain_app::ports::SnapshotDiagnostic::Rejected(
                aex_brain_domain::snapshot::FoldSnapshotError::BodyLengthMismatch {
                    declared: pointer.body_bytes,
                    actual: artifact.body().len(),
                },
            ),
        });
    }
    if BodyDigest::of(artifact.body()) != pointer.body_digest {
        return Err(StoreError::SnapshotRejected {
            diagnostic: aex_brain_app::ports::SnapshotDiagnostic::Rejected(
                aex_brain_domain::snapshot::FoldSnapshotError::BodyDigestMismatch,
            ),
        });
    }
    Ok(())
}

fn selected_outcome(
    proposed: &FoldSnapshotPointer,
    selected: Option<&FoldSnapshotPointer>,
) -> Result<Option<SnapshotPublishOutcome>, StoreError> {
    let Some(selected) = selected else {
        return Ok(None);
    };
    if selected.absorbed.seq > proposed.absorbed.seq {
        return Ok(Some(SnapshotPublishOutcome::Superseded {
            current: selected.absorbed,
        }));
    }
    if selected.absorbed.seq == proposed.absorbed.seq && selected != proposed {
        return Err(StoreError::SnapshotPointerConflict {
            seq: proposed.absorbed.seq,
        });
    }
    Ok(None)
}

fn resolve_changed_pointer(
    proposed: &FoldSnapshotPointer,
    previous: Option<&FoldSnapshotPointer>,
    selected: Option<&FoldSnapshotPointer>,
    ambiguous: bool,
) -> Result<SnapshotPublishOutcome, StoreError> {
    if selected == Some(proposed) {
        return Ok(SnapshotPublishOutcome::AlreadyCurrent);
    }
    if let Some(outcome) = selected_outcome(proposed, selected)? {
        return Ok(outcome);
    }
    let unchanged = selected == previous;
    Err(StoreError::Transport {
        reason: if ambiguous && unchanged {
            "fold snapshot pointer commit remains unknown after a strong target read".to_owned()
        } else {
            "fold snapshot pointer changed concurrently before publication".to_owned()
        },
        retryable: !ambiguous,
    })
}

fn map_content_read(
    error: ContentObjectError,
    pointer: &FoldSnapshotPointer,
    max_bytes: usize,
) -> StoreError {
    match error {
        ContentObjectError::InvalidRange { requested, .. } => {
            if requested > u64::try_from(max_bytes).unwrap_or(u64::MAX) {
                StoreError::SnapshotBodyTooLarge {
                    declared: requested,
                    max: max_bytes,
                }
            } else if let Ok(actual) = usize::try_from(requested) {
                StoreError::SnapshotRejected {
                    diagnostic: aex_brain_app::ports::SnapshotDiagnostic::Rejected(
                        aex_brain_domain::snapshot::FoldSnapshotError::BodyLengthMismatch {
                            declared: pointer.body_bytes,
                            actual,
                        },
                    ),
                }
            } else {
                content_error(error)
            }
        }
        other => content_error(other),
    }
}

fn map_content_write(error: ContentObjectError) -> StoreError {
    content_error(error)
}

fn content_error(error: ContentObjectError) -> StoreError {
    let retryable = error.retryable();
    let reason = match &error {
        ContentObjectError::DigestCollision { .. } => "content digest collision",
        ContentObjectError::ContentMissing { .. } => "content object is missing",
        ContentObjectError::IntegrityMismatch { .. } => "content object failed integrity checks",
        ContentObjectError::InvalidRange { .. } => "content object exceeded its read bound",
        ContentObjectError::Contended => "content object write contended",
        ContentObjectError::Throttled => "content object store throttled",
        ContentObjectError::Forbidden => "content object action was denied",
        ContentObjectError::CommitAmbiguous { .. } => "content object commit remains unknown",
        ContentObjectError::Unavailable { .. } => "content object store was unavailable",
        ContentObjectError::Invalid { .. } => "content object request was invalid",
    }
    .to_owned();
    match error {
        ContentObjectError::DigestCollision { key }
        | ContentObjectError::ContentMissing { digest: key }
        | ContentObjectError::IntegrityMismatch { detail: key } => StoreError::ContentUnavailable {
            reference: redact_reference(&key),
            reason,
        },
        _ => StoreError::Transport { reason, retryable },
    }
}

fn redact_reference(value: &str) -> String {
    BodyDigest::of(value.as_bytes()).to_wire()
}

fn map_plan_error(error: PlanError) -> StoreError {
    match error {
        PlanError::Key(error) => StoreError::Undecodable {
            location: "fold snapshot pointer key".to_owned(),
            reason: error.to_string(),
        },
        PlanError::Store(error) => map_shared_error(&error),
        PlanError::Envelope(error) => StoreError::Transport {
            reason: error.to_string(),
            retryable: false,
        },
    }
}

fn map_shared_error(error: &aex_session_dynamodb::error::StoreError) -> StoreError {
    use aex_session_dynamodb::error::StoreError as Shared;
    match error {
        Shared::Corrupt(error) => StoreError::Undecodable {
            location: "fold snapshot publication".to_owned(),
            reason: error.to_string(),
        },
        other => StoreError::Transport {
            reason: other.to_string(),
            retryable: other.retryable(),
        },
    }
}

fn composition_error(reason: &str) -> StoreError {
    StoreError::Transport {
        reason: reason.to_owned(),
        retryable: false,
    }
}

/// Why a selected pointer row cannot become a typed snapshot pointer.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SnapshotPointerCodecError {
    /// A required attribute was absent or had the wrong `DynamoDB` type.
    #[error(transparent)]
    Codec(#[from] CodecError),
    /// The expected Brain key could not be rendered canonically.
    #[error(transparent)]
    Key(#[from] keys::BrainKeyError),
    /// A typed hash attribute had invalid wire bytes.
    #[error("fold snapshot pointer `{attribute}` is malformed: {reason}")]
    Malformed {
        /// Which closed attribute.
        attribute: &'static str,
        /// Why it failed strict parsing.
        reason: String,
    },
    /// The row key or repeated agent identity does not match the requested agent.
    #[error("fold snapshot pointer addresses a different agent")]
    AgentMismatch,
}

/// Encodes the selected pointer row exactly once.
///
/// # Errors
///
/// Returns [`keys::BrainKeyError`] when the domain agent identity is not canonical `UUIDv7`.
pub fn encode_pointer(pointer: &FoldSnapshotPointer) -> Result<Item, keys::BrainKeyError> {
    let selected = keys::fold_snapshot(&pointer.agent)?;
    let (session, agent) = translate::agent_key(&pointer.agent)?;
    Ok(ItemBuilder::new(FOLD_SNAPSHOT_POINTER)
        .set(aex_session_dynamodb::attr::PK, s(selected.pk))
        .set(aex_session_dynamodb::attr::SK, s(selected.sk))
        .set("sessionId", s(session.to_string()))
        .set("agentId", s(agent.to_string()))
        .set("schema", s(pointer.schema.clone()))
        .set("absorbedSeq", n(pointer.absorbed.seq.get()))
        .set("absorbedHash", s(pointer.absorbed.hash.to_hex()))
        .set("configDigest", s(pointer.config_digest.to_wire()))
        .set("bodyDigest", s(pointer.body_digest.to_wire()))
        .set("bodyBytes", n(pointer.body_bytes))
        .build())
}

/// Decodes one strongly read pointer and rechecks its key plus repeated agent identity.
///
/// # Errors
///
/// Refuses missing/wrong-typed attributes, non-lowercase or wrong-length hashes, malformed
/// SHA-256 identities, and any physical/repeated agent mismatch.
pub fn decode_pointer(
    expected: aex_brain_domain::ids::AgentKey,
    item: &Item,
) -> Result<FoldSnapshotPointer, SnapshotPointerCodecError> {
    let row = Row::bind(item, FOLD_SNAPSHOT_POINTER)?;
    let selected = keys::fold_snapshot(&expected)?;
    let (session, agent) = translate::agent_key(&expected).map_err(keys::BrainKeyError::from)?;
    let session = session.to_string();
    let agent = agent.to_string();
    if row.string(aex_session_dynamodb::attr::PK)? != selected.pk
        || row.string(aex_session_dynamodb::attr::SK)? != selected.sk
        || row.string("sessionId")? != session
        || row.string("agentId")? != agent
    {
        return Err(SnapshotPointerCodecError::AgentMismatch);
    }
    let absorbed_hash = parse_journal_hash(row.string("absorbedHash")?)?;
    let config_digest =
        aex_wire::ids::ContentHash::parse(row.string("configDigest")?).map_err(|error| {
            SnapshotPointerCodecError::Malformed {
                attribute: "configDigest",
                reason: error.to_string(),
            }
        })?;
    let body_digest =
        aex_wire::ids::ContentHash::parse(row.string("bodyDigest")?).map_err(|error| {
            SnapshotPointerCodecError::Malformed {
                attribute: "bodyDigest",
                reason: error.to_string(),
            }
        })?;
    Ok(FoldSnapshotPointer {
        schema: row.string("schema")?.to_owned(),
        agent: expected,
        absorbed: aex_brain_domain::snapshot::JournalPoint {
            seq: aex_brain_domain::ids::JournalSeq(row.u64("absorbedSeq")?),
            hash: absorbed_hash,
        },
        config_digest,
        body_digest,
        body_bytes: row.u64("bodyBytes")?,
    })
}

fn parse_journal_hash(
    text: &str,
) -> Result<aex_brain_domain::ids::ContentHash, SnapshotPointerCodecError> {
    if text.len() != 64 {
        return Err(SnapshotPointerCodecError::Malformed {
            attribute: "absorbedHash",
            reason: "expected 64 lowercase hexadecimal characters".to_owned(),
        });
    }
    let mut bytes = [0_u8; 32];
    for (index, pair) in text.as_bytes().chunks_exact(2).enumerate() {
        let high = lower_hex(pair[0]).ok_or_else(|| SnapshotPointerCodecError::Malformed {
            attribute: "absorbedHash",
            reason: "expected lowercase hexadecimal".to_owned(),
        })?;
        let low = lower_hex(pair[1]).ok_or_else(|| SnapshotPointerCodecError::Malformed {
            attribute: "absorbedHash",
            reason: "expected lowercase hexadecimal".to_owned(),
        })?;
        bytes[index] = (high << 4) | low;
    }
    Ok(aex_brain_domain::ids::ContentHash(bytes))
}

const fn lower_hex(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        _ => None,
    }
}

/// Compiles the three-condition monotonic pointer publication.
///
/// Publication proves the exact immutable journal row and that the current control tail is
/// at least that far advanced. It intentionally does **not** require the current head to
/// equal the snapshot cut: exact-head publication would starve continuously active agents.
/// The final update advances only from the complete pointer tuple the caller strongly read,
/// or accepts an exact same-cut/body retry. A slower publisher therefore cannot roll the
/// pointer back, and a malformed older row cannot be silently overwritten.
///
/// The caller must first publish `pointer.body_digest` through the regional content
/// authority and strongly read/decode the selected pointer into `previous`. `None` means the
/// strongly consistent read observed true absence. This transaction never writes body bytes
/// or S3 directly.
///
/// # Errors
///
/// [`PlanError::Key`] when the agent key has no canonical wire form, and
/// [`PlanError::Store`] when schema/body metadata is invalid or the shared transaction
/// compiler rejects the plan.
pub fn compile_publication(
    tables: &BrainTables,
    previous: Option<&FoldSnapshotPointer>,
    pointer: &FoldSnapshotPointer,
) -> Result<TransactionPlan, PlanError> {
    validate_publication(previous, pointer)?;
    let table = tables.session_authority.as_str();
    let journal = keys::journal(&pointer.agent, pointer.absorbed.seq)?;
    let control = keys::control(&pointer.agent)?;
    let selected = keys::fold_snapshot(&pointer.agent)?;
    let (session, agent) =
        translate::agent_key(&pointer.agent).map_err(keys::BrainKeyError::from)?;
    let token = format!(
        "fold-snapshot:{session}:{agent}:{previous_identity}:{}:{}",
        pointer.absorbed.seq.get(),
        pointer.body_digest.to_wire(),
        previous_identity = publication_precondition_identity(previous),
    );
    let mut plan = TransactionPlan::new(token);

    append_publication_proofs(&mut plan, table, &journal, &control, pointer)?;
    append_pointer_update(
        &mut plan, table, &selected, &session, &agent, previous, pointer,
    )?;
    Ok(plan)
}

fn append_publication_proofs(
    plan: &mut TransactionPlan,
    table: &str,
    journal: &keys::Key,
    control: &keys::Key,
    pointer: &FoldSnapshotPointer,
) -> Result<(), PlanError> {
    plan.condition_check(
        participant::FOLD_SNAPSHOT_JOURNAL_POINT,
        aws_sdk_dynamodb::types::ConditionCheck::builder()
            .table_name(table)
            .set_key(Some(key(&journal.pk, &journal.sk)))
            .condition_expression(
                "itemType = :journalType AND seq = :absorbedSeq AND entryId = :absorbedHash",
            )
            .expression_attribute_values(
                ":journalType",
                s(aex_session_dynamodb::codec::JOURNAL_ENTRY),
            )
            .expression_attribute_values(":absorbedSeq", n(pointer.absorbed.seq.get()))
            .expression_attribute_values(":absorbedHash", s(pointer.absorbed.hash.to_hex())),
    )?;
    plan.condition_check(
        participant::FOLD_SNAPSHOT_CONTROL,
        aws_sdk_dynamodb::types::ConditionCheck::builder()
            .table_name(table)
            .set_key(Some(key(&control.pk, &control.sk)))
            .condition_expression(
                "itemType = :controlType AND hasJournal = :hasJournal AND \
                 journalTail >= :absorbedSeq",
            )
            .expression_attribute_values(
                ":controlType",
                s(aex_session_dynamodb::codec::AGENT_CONTROL),
            )
            .expression_attribute_values(":hasJournal", aex_session_dynamodb::attr::boolean(true))
            .expression_attribute_values(":absorbedSeq", n(pointer.absorbed.seq.get())),
    )?;
    Ok(())
}

fn append_pointer_update(
    plan: &mut TransactionPlan,
    table: &str,
    selected: &keys::Key,
    session: &impl ToString,
    agent: &impl ToString,
    previous: Option<&FoldSnapshotPointer>,
    pointer: &FoldSnapshotPointer,
) -> Result<(), PlanError> {
    let selected_update = aws_sdk_dynamodb::types::Update::builder()
        .table_name(table)
        .set_key(Some(key(&selected.pk, &selected.sk)));
    let selected_update = if let Some(previous) = previous {
        selected_update
            .condition_expression(
                "itemType = :itemType AND sessionId = :sessionId AND agentId = :agentId AND \
                 schema = :previousSchema AND absorbedSeq = :previousAbsorbedSeq AND \
                 absorbedHash = :previousAbsorbedHash AND \
                 configDigest = :previousConfigDigest AND bodyDigest = :previousBodyDigest AND \
                 bodyBytes = :previousBodyBytes",
            )
            .expression_attribute_values(":previousSchema", s(previous.schema.clone()))
            .expression_attribute_values(":previousAbsorbedSeq", n(previous.absorbed.seq.get()))
            .expression_attribute_values(
                ":previousAbsorbedHash",
                s(previous.absorbed.hash.to_hex()),
            )
            .expression_attribute_values(
                ":previousConfigDigest",
                s(previous.config_digest.to_wire()),
            )
            .expression_attribute_values(":previousBodyDigest", s(previous.body_digest.to_wire()))
            .expression_attribute_values(":previousBodyBytes", n(previous.body_bytes))
    } else {
        selected_update.condition_expression("attribute_not_exists(pk)")
    };
    plan.update(
        participant::FOLD_SNAPSHOT_POINTER,
        selected_update
            .update_expression(
                "SET itemType = :itemType, sessionId = :sessionId, agentId = :agentId, \
                 schema = :schema, absorbedSeq = :absorbedSeq, absorbedHash = :absorbedHash, \
                 configDigest = :configDigest, bodyDigest = :bodyDigest, bodyBytes = :bodyBytes",
            )
            .expression_attribute_values(":itemType", s(FOLD_SNAPSHOT_POINTER))
            .expression_attribute_values(":sessionId", s(session.to_string()))
            .expression_attribute_values(":agentId", s(agent.to_string()))
            .expression_attribute_values(":schema", s(pointer.schema.clone()))
            .expression_attribute_values(":absorbedSeq", n(pointer.absorbed.seq.get()))
            .expression_attribute_values(":absorbedHash", s(pointer.absorbed.hash.to_hex()))
            .expression_attribute_values(":configDigest", s(pointer.config_digest.to_wire()))
            .expression_attribute_values(":bodyDigest", s(pointer.body_digest.to_wire()))
            .expression_attribute_values(":bodyBytes", n(pointer.body_bytes)),
    )?;
    Ok(())
}

fn validate_publication(
    previous: Option<&FoldSnapshotPointer>,
    pointer: &FoldSnapshotPointer,
) -> Result<(), PlanError> {
    validate_pointer("next", pointer)?;
    let Some(previous) = previous else {
        return Ok(());
    };
    validate_pointer("previous", previous)?;
    if previous.agent != pointer.agent {
        return invalid("previous and next fold snapshot pointers address different agents");
    }
    if previous.absorbed.seq > pointer.absorbed.seq {
        return invalid("a fold snapshot publication cannot roll the selected sequence back");
    }
    if previous.absorbed.seq == pointer.absorbed.seq && previous != pointer {
        return invalid(
            "one fold snapshot cut cannot select different metadata or immutable bytes",
        );
    }
    Ok(())
}

fn publication_precondition_identity(previous: Option<&FoldSnapshotPointer>) -> String {
    previous.map_or_else(
        || "absent".to_owned(),
        |previous| {
            format!(
                "{}:{}:{}:{}:{}:{}",
                previous.schema,
                previous.absorbed.seq.get(),
                previous.absorbed.hash.to_hex(),
                previous.config_digest.to_wire(),
                previous.body_digest.to_wire(),
                previous.body_bytes
            )
        },
    )
}

fn validate_pointer(label: &str, pointer: &FoldSnapshotPointer) -> Result<(), PlanError> {
    if pointer.schema != FOLD_SNAPSHOT_SCHEMA {
        return invalid(format!(
            "{label} fold snapshot uses unsupported schema `{}`",
            pointer.schema
        ));
    }
    if pointer.body_bytes == 0
        || pointer.body_bytes > u64::try_from(MAX_FOLD_SNAPSHOT_BYTES).unwrap_or(u64::MAX)
    {
        return invalid(format!(
            "{label} fold snapshot body length {} is outside 1..={MAX_FOLD_SNAPSHOT_BYTES}",
            pointer.body_bytes
        ));
    }
    Ok(())
}

fn invalid<T>(detail: impl Into<String>) -> Result<T, PlanError> {
    Err(PlanError::Store(
        aex_session_dynamodb::StoreError::Invalid {
            detail: detail.into(),
        },
    ))
}

#[cfg(test)]
mod tests {
    use super::{SnapshotPointerCodecError, compile_publication, decode_pointer, encode_pointer};
    use crate::keys;
    use crate::plan::{BrainTables, participant};
    use aex_brain_domain::ids::{AgentId, AgentKey, ContentHash, JournalSeq, SessionId};
    use aex_brain_domain::snapshot::{FOLD_SNAPSHOT_SCHEMA, FoldSnapshotPointer, JournalPoint};
    use aex_wire::ids::{ContentHash as BodyDigest, Uuid7};

    fn v7(millis: u64, seed: u8) -> uuid::Uuid {
        uuid::Uuid::from_bytes(*Uuid7::compose(millis, [seed; 10]).as_bytes())
    }

    fn pointer() -> FoldSnapshotPointer {
        FoldSnapshotPointer {
            schema: FOLD_SNAPSHOT_SCHEMA.to_owned(),
            agent: AgentKey::new(
                SessionId(v7(1_767_225_600_000, 1)),
                AgentId(v7(1_767_225_600_001, 2)),
            ),
            absorbed: JournalPoint {
                seq: JournalSeq(41),
                hash: ContentHash([3; 32]),
            },
            config_digest: BodyDigest::from_bytes([4; 32]),
            body_digest: BodyDigest::from_bytes([5; 32]),
            body_bytes: 123_456,
        }
    }

    #[test]
    fn publication_proves_history_and_never_requires_exact_current_head() {
        let pointer = pointer();
        let mut previous = pointer.clone();
        previous.absorbed.seq = JournalSeq(40);
        previous.absorbed.hash = ContentHash([6; 32]);
        previous.config_digest = BodyDigest::from_bytes([7; 32]);
        previous.body_digest = BodyDigest::from_bytes([8; 32]);
        previous.body_bytes = 120_000;
        let plan = compile_publication(
            &BrainTables {
                session_authority: "session-authority".to_owned(),
                regional_work: "regional-work".to_owned(),
            },
            Some(&previous),
            &pointer,
        )
        .expect("the plan compiles");
        assert_eq!(
            plan.participants(),
            [
                participant::FOLD_SNAPSHOT_JOURNAL_POINT,
                participant::FOLD_SNAPSHOT_CONTROL,
                participant::FOLD_SNAPSHOT_POINTER,
            ]
        );
        assert_eq!(plan.len(), 3);

        let journal = plan.actions()[0]
            .condition_check()
            .expect("the first action is a journal guard");
        assert_eq!(
            journal.condition_expression(),
            "itemType = :journalType AND seq = :absorbedSeq AND entryId = :absorbedHash"
        );
        assert_eq!(
            journal.key(),
            &aex_session_dynamodb::plan::key(
                &keys::journal(&pointer.agent, pointer.absorbed.seq)
                    .expect("v7")
                    .pk,
                &keys::journal(&pointer.agent, pointer.absorbed.seq)
                    .expect("v7")
                    .sk,
            )
        );

        let control = plan.actions()[1]
            .condition_check()
            .expect("the second action is a control guard");
        assert_eq!(
            control.condition_expression(),
            "itemType = :controlType AND hasJournal = :hasJournal AND journalTail >= :absorbedSeq"
        );
        assert!(!control.condition_expression().contains("journalTail ="));

        let selected = plan.actions()[2]
            .update()
            .expect("the final action advances the pointer");
        let condition = selected
            .condition_expression()
            .expect("update is conditional");
        assert!(condition.contains("itemType = :itemType"));
        assert!(condition.contains("sessionId = :sessionId"));
        assert!(condition.contains("agentId = :agentId"));
        assert!(condition.contains("absorbedSeq = :previousAbsorbedSeq"));
        assert!(condition.contains("absorbedHash = :previousAbsorbedHash"));
        assert!(condition.contains("configDigest = :previousConfigDigest"));
        assert!(condition.contains("bodyDigest = :previousBodyDigest"));
        assert!(condition.contains("bodyBytes = :previousBodyBytes"));
        assert!(!condition.contains("journalTail"));
    }

    #[test]
    fn publication_requires_true_absence_or_the_exact_strongly_read_pointer() {
        let tables = BrainTables {
            session_authority: "session-authority".to_owned(),
            regional_work: "regional-work".to_owned(),
        };
        let pointer = pointer();
        let absent = compile_publication(&tables, None, &pointer).expect("absence compiles");
        assert_eq!(
            absent.actions()[2]
                .update()
                .expect("pointer update")
                .condition_expression(),
            Some("attribute_not_exists(pk)")
        );

        let exact_retry =
            compile_publication(&tables, Some(&pointer), &pointer).expect("exact retry compiles");
        assert!(
            exact_retry.actions()[2]
                .update()
                .expect("pointer update")
                .condition_expression()
                .expect("condition")
                .contains("bodyDigest = :previousBodyDigest")
        );

        let mut corrupt_previous = pointer.clone();
        corrupt_previous.schema = "aex.brain.fold.corrupt".to_owned();
        assert!(compile_publication(&tables, Some(&corrupt_previous), &pointer).is_err());

        let mut conflicting_cut = pointer.clone();
        conflicting_cut.body_digest = BodyDigest::from_bytes([9; 32]);
        assert!(compile_publication(&tables, Some(&pointer), &conflicting_cut).is_err());

        let mut stale = pointer.clone();
        stale.absorbed.seq = JournalSeq(40);
        assert!(compile_publication(&tables, Some(&pointer), &stale).is_err());
    }

    #[test]
    fn publication_refuses_an_unknown_schema_or_impossible_body_size_before_aws() {
        let tables = BrainTables {
            session_authority: "session-authority".to_owned(),
            regional_work: "regional-work".to_owned(),
        };
        let mut invalid = pointer();
        invalid.schema = "aex.brain.fold.unknown".to_owned();
        assert!(matches!(
            compile_publication(&tables, None, &invalid),
            Err(crate::plan::PlanError::Store(
                aex_session_dynamodb::StoreError::Invalid { .. }
            ))
        ));

        invalid = pointer();
        invalid.body_bytes = 0;
        assert!(matches!(
            compile_publication(&tables, None, &invalid),
            Err(crate::plan::PlanError::Store(
                aex_session_dynamodb::StoreError::Invalid { .. }
            ))
        ));

        invalid.body_bytes = u64::try_from(aex_brain_domain::snapshot::MAX_FOLD_SNAPSHOT_BYTES)
            .expect("the hard ceiling fits u64")
            .saturating_add(1);
        assert!(matches!(
            compile_publication(&tables, None, &invalid),
            Err(crate::plan::PlanError::Store(
                aex_session_dynamodb::StoreError::Invalid { .. }
            ))
        ));
    }

    #[test]
    fn pointer_codec_round_trips_and_rejects_substitution() {
        let pointer = pointer();
        let mut item = encode_pointer(&pointer).expect("v7 pointer encodes");
        assert_eq!(
            decode_pointer(pointer.agent, &item).expect("pointer decodes"),
            pointer
        );

        item.insert(
            "absorbedHash".to_owned(),
            aex_session_dynamodb::attr::s("AA".repeat(32)),
        );
        assert!(matches!(
            decode_pointer(pointer.agent, &item),
            Err(SnapshotPointerCodecError::Malformed {
                attribute: "absorbedHash",
                ..
            })
        ));
    }
}
