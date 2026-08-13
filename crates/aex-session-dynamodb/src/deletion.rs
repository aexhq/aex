//! Physical support for irreversible session deletion.
//!
//! This module deliberately does not orchestrate or mount `session_delete`.
//! It gives that future application slice three storage primitives which are
//! otherwise easy to get subtly wrong:
//!
//! - an immutable progress root bound to one operation and deletion epoch;
//! - immutable, content-free receipts from every deletion owner;
//! - an enumerable directory for idempotency receipts whose authority rows do
//!   not live in the session partition.
//!
//! Every write compiler is conditional. A caller may append the completion
//! guards and tombstone put to the same final operation/work transaction, but
//! executing the tombstone put on its own is intentionally not offered.

use std::collections::BTreeMap;

use aex_operation_domain::DeletionEpoch;
use aex_session_domain::{DeleteEvidence, SessionTombstone};
use aex_wire::ids::{OperationId, SessionId, WorkspaceId};
use aex_wire::types::Timestamp;
use aws_sdk_dynamodb::types::builders::{ConditionCheckBuilder, DeleteBuilder, PutBuilder};
use aws_sdk_dynamodb::types::{Get, TransactGetItem};
use sha2::{Digest as _, Sha256};

use crate::attr::{CodecError, Item, ItemBuilder, PK, Row, SK, n, s, stamp};
use crate::error::{Idempotence, StoreError, classify};
use crate::plan::{IMMUTABLE, Participant, TransactionPlan, key};
use crate::{codec, keys};

/// One bounded receipt-directory page. Deletion drains at most this many rows
/// before yielding to its durable continuation.
pub const RECEIPT_DIRECTORY_PAGE_MAX: u8 = 25;
/// Maximum payload rows one guarded deletion transaction removes before yielding.
pub const DELETION_PAGE_MAX: usize = 25;

const PROGRESS_STATE: &str = "deleting";
const RECEIPT_ITEM_TYPE: &str = "idempotency_receipt";
const RECEIPT_SK: &str = "RECEIPT";

const SESSION_HEAD_GUARD: Participant = Participant::new("session.delete_head_guard");
const DELETION_PROGRESS: Participant = Participant::new("session.delete_progress");
const DELETION_EVIDENCE: Participant = Participant::new("session.delete_evidence");
const RECEIPT_DIRECTORY: Participant = Participant::new("session.delete_receipt_directory");
const RECEIPT_PAYLOAD: Participant = Participant::new("session.delete_receipt_payload");
const OWNED_PAYLOAD: Participant = Participant::new("session.delete_owned_payload");

/// One exact row already validated by its owning authority's bounded query.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DeletionTarget {
    /// Physical table.
    pub table: String,
    /// Exact partition key.
    pub pk: String,
    /// Exact sort key.
    pub sk: String,
    /// Expected closed-vocabulary item type.
    pub item_type: String,
}

/// A content-free digest of an owner's durable proof.
///
/// The proof itself stays with its owning authority. The deletion authority
/// retains only this digest, so completing a cascade cannot accidentally copy
/// telemetry, a message, provider output, or another user payload into the
/// tombstone path.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct EvidenceDigest([u8; 32]);

impl EvidenceDigest {
    /// Digests one canonical owner proof.
    #[must_use]
    pub fn from_proof(proof: &[u8]) -> Self {
        Self(Sha256::digest(proof).into())
    }

    fn parse(value: &str) -> Option<Self> {
        let bytes = hex::decode(value).ok()?;
        <[u8; 32]>::try_from(bytes).ok().map(Self)
    }

    fn as_hex(self) -> String {
        hex::encode(self.0)
    }
}

/// Every independent deletion fact required by the domain completion barrier.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum DeletionOwner {
    /// The exact runtime generation is terminal.
    GenerationTerminated,
    /// Session-head user content is gone.
    SessionContent,
    /// Message, sealed-message, run, event and receipt payloads are gone.
    Messages,
    /// Brain journal, effect, mailbox and scheduler content is gone.
    BrainUserContent,
    /// Aggregate billing evidence remains durably handed off.
    BillingAggregate,
    /// The content-free audit fact remains durably handed off.
    AuditFact,
}

impl DeletionOwner {
    /// Canonical owner order used by atomic snapshots and final guards.
    pub const ALL: [Self; 6] = [
        Self::GenerationTerminated,
        Self::SessionContent,
        Self::Messages,
        Self::BrainUserContent,
        Self::BillingAggregate,
        Self::AuditFact,
    ];

    /// Stable physical spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::GenerationTerminated => "generation_terminated",
            Self::SessionContent => "session_content_removed",
            Self::Messages => "messages_removed",
            Self::BrainUserContent => "brain_user_content_removed",
            Self::BillingAggregate => "billing_aggregate_retained",
            Self::AuditFact => "audit_fact_retained",
        }
    }

    fn parse(value: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|owner| owner.as_str() == value)
    }
}

/// Immutable root binding a deletion continuation to its elected operation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SessionDeletionProgress {
    /// Owning workspace.
    pub workspace: WorkspaceId,
    /// Session being deleted.
    pub session: SessionId,
    /// Canonical `session_delete` operation.
    pub operation: OperationId,
    /// Deletion epoch elected on the session head.
    pub epoch: DeletionEpoch,
    /// When this physical cascade was initialized.
    pub started_at: Timestamp,
}

impl SessionDeletionProgress {
    fn validate(self) -> Result<Self, StoreError> {
        if self.epoch.0 == 0 {
            return Err(StoreError::Invalid {
                detail: "a session deletion progress row requires a non-zero epoch".to_owned(),
            });
        }
        Ok(self)
    }
}

/// One immutable owner receipt bound to the exact progress identity.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SessionDeletionEvidence {
    /// Owning workspace.
    pub workspace: WorkspaceId,
    /// Session being deleted.
    pub session: SessionId,
    /// Canonical `session_delete` operation.
    pub operation: OperationId,
    /// Exact deletion epoch.
    pub epoch: DeletionEpoch,
    /// Owner whose proof completed.
    pub owner: DeletionOwner,
    /// Digest of the owner authority's canonical proof.
    pub proof: EvidenceDigest,
    /// When the owner proof became durable.
    pub completed_at: Timestamp,
}

impl SessionDeletionEvidence {
    fn belongs_to(self, progress: SessionDeletionProgress) -> bool {
        self.workspace == progress.workspace
            && self.session == progress.session
            && self.operation == progress.operation
            && self.epoch == progress.epoch
    }
}

/// An atomic, status-sensitive read of progress and all owner receipts.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SessionDeletionSnapshot {
    /// Exact progress root.
    pub progress: SessionDeletionProgress,
    /// Owner receipts present in the same serializable snapshot.
    pub evidence: BTreeMap<DeletionOwner, SessionDeletionEvidence>,
}

impl SessionDeletionSnapshot {
    /// Projects only durable owner rows into the domain evidence vocabulary.
    #[must_use]
    pub fn domain_evidence(&self) -> DeleteEvidence {
        let present = |owner| self.evidence.contains_key(&owner);
        DeleteEvidence {
            generation_terminated: present(DeletionOwner::GenerationTerminated),
            session_content_removed: present(DeletionOwner::SessionContent),
            messages_removed: present(DeletionOwner::Messages),
            brain_user_content_removed: present(DeletionOwner::BrainUserContent),
            billing_aggregate_retained: present(DeletionOwner::BillingAggregate),
            audit_fact_retained: present(DeletionOwner::AuditFact),
        }
    }

    /// Whether every required owner receipt was present atomically.
    #[must_use]
    pub fn is_complete(&self) -> bool {
        self.domain_evidence().missing().is_none()
    }
}

/// Enumerable locator for one separately keyed session idempotency receipt.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SessionReceiptDirectoryEntry {
    /// Owning workspace.
    pub workspace: WorkspaceId,
    /// Owning session.
    pub session: SessionId,
    /// Exact receipt partition key.
    pub receipt_pk: String,
    /// Exact receipt sort key.
    pub receipt_sk: String,
    /// Canonical idempotency scope encoded in the receipt key and row.
    pub scope: String,
    /// Digest used by the directory sort key.
    pub target: EvidenceDigest,
    /// When the receipt and locator were admitted atomically.
    pub created_at: Timestamp,
}

impl SessionReceiptDirectoryEntry {
    /// Creates a locator for the canonical session-table receipt key.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::Invalid`] when the target is not an idempotency
    /// receipt in the asserted workspace.
    pub fn new(
        workspace: WorkspaceId,
        session: SessionId,
        receipt_partition_key: impl Into<String>,
        receipt_sort_key: impl Into<String>,
        created_at: Timestamp,
    ) -> Result<Self, StoreError> {
        let receipt_partition_key = receipt_partition_key.into();
        let receipt_sort_key = receipt_sort_key.into();
        let Some(scope) = receipt_scope(workspace, session, &receipt_partition_key) else {
            return Err(StoreError::Invalid {
                detail:
                    "a session receipt locator must name an exact session or create receipt in its workspace"
                        .to_owned(),
            });
        };
        if receipt_sort_key != RECEIPT_SK {
            return Err(StoreError::Invalid {
                detail:
                    "a session receipt locator must name an exact session or create receipt in its workspace"
                        .to_owned(),
            });
        }
        let scope = scope.to_owned();
        let target = receipt_target(&receipt_partition_key, &receipt_sort_key);
        Ok(Self {
            workspace,
            session,
            receipt_pk: receipt_partition_key,
            receipt_sk: receipt_sort_key,
            scope,
            target,
            created_at,
        })
    }

    fn key(&self) -> keys::Key {
        keys::receipt_directory(self.session, &self.target.as_hex())
    }

    fn validate(&self) -> Result<(), StoreError> {
        if receipt_scope(self.workspace, self.session, &self.receipt_pk)
            != Some(self.scope.as_str())
            || self.receipt_sk != RECEIPT_SK
            || self.target != receipt_target(&self.receipt_pk, &self.receipt_sk)
        {
            return Err(StoreError::Invalid {
                detail: "the receipt locator identity is internally inconsistent".to_owned(),
            });
        }
        Ok(())
    }
}

/// Derives the exact enumerable locator from the same canonical receipt codec
/// used by the receipt write.
///
/// Keeping this derivation beside the directory codec prevents an application
/// planner from guessing `DynamoDB` key templates. The caller still emits the
/// locator as its own logical action so transaction accounting remains exact.
///
/// # Errors
///
/// Returns [`StoreError`] when the receipt is not session-owned or its physical
/// key cannot be rendered canonically.
pub fn receipt_directory_entry(
    workspace: WorkspaceId,
    session: SessionId,
    receipt: &aex_session_domain::IdempotencyReceipt,
) -> Result<SessionReceiptDirectoryEntry, StoreError> {
    if receipt.key.scope() == "session.create"
        && !matches!(
            &receipt.outcome,
            aex_session_domain::ReceiptOutcome::Resource { kind, id, .. }
                if *kind == aex_session_domain::ResourceKind::Session
                    && id.0 == session.to_string()
        )
    {
        return Err(StoreError::Invalid {
            detail: "a create receipt locator must name the exact created session".to_owned(),
        });
    }
    let row = crate::codec::receipt_of(receipt).map_err(|error| StoreError::Invalid {
        detail: format!("an idempotency receipt could not be projected: {error}"),
    })?;
    let item =
        crate::codec::encode_receipt(workspace, &row).map_err(|error| StoreError::Invalid {
            detail: format!("an idempotency receipt key is unusable: {error}"),
        })?;
    let encoded = Row::bind(&item, RECEIPT_ITEM_TYPE)?;
    SessionReceiptDirectoryEntry::new(
        workspace,
        session,
        encoded.string(PK)?.to_owned(),
        encoded.string(SK)?.to_owned(),
        receipt.created_at,
    )
}

/// One bounded strong directory page.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SessionReceiptDirectoryPage {
    /// Exact decoded locators.
    pub entries: Vec<SessionReceiptDirectoryEntry>,
    /// Sort key to use as the next exclusive start, when more rows remain.
    pub next: Option<String>,
}

/// Strong, bounded deletion-support reads.
#[derive(Clone, Debug)]
pub struct SessionDeletionStore {
    dynamodb: aws_sdk_dynamodb::Client,
    table: String,
}

impl SessionDeletionStore {
    /// Binds the exact regional session table.
    #[must_use]
    pub fn new(dynamodb: aws_sdk_dynamodb::Client, table: impl Into<String>) -> Self {
        Self {
            dynamodb,
            table: table.into(),
        }
    }

    /// Strongly reads the payload-free deletion coordination head.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError`] when the strong read is unavailable or the stored
    /// deletion head is malformed or belongs to another authority scope.
    pub async fn load_head(
        &self,
        workspace: WorkspaceId,
        session: SessionId,
    ) -> Result<Option<aex_session_domain::SessionDeletionHead>, StoreError> {
        let target = keys::head(session);
        let item = self
            .dynamodb
            .get_item()
            .table_name(&self.table)
            .key(PK, s(target.pk))
            .key(SK, s(target.sk))
            .consistent_read(true)
            .send()
            .await
            .map_err(|error| classify(&error, Idempotence::Read))?
            .item;
        item.as_ref()
            .map(|item| {
                crate::authority_codec::decode_session_deletion_head(item, workspace, session)
                    .map_err(StoreError::from)
            })
            .transpose()
    }

    /// Strongly reads the progress root.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError`] for an unavailable read or malformed authority.
    pub async fn load_progress(
        &self,
        workspace: WorkspaceId,
        session: SessionId,
    ) -> Result<Option<SessionDeletionProgress>, StoreError> {
        let target = keys::deletion_progress(session);
        let item = self
            .dynamodb
            .get_item()
            .table_name(&self.table)
            .key(PK, s(target.pk))
            .key(SK, s(target.sk))
            .consistent_read(true)
            .send()
            .await
            .map_err(|error| classify(&error, Idempotence::Read))?
            .item;
        item.as_ref()
            .map(|item| decode_progress(item, workspace, session).map_err(StoreError::from))
            .transpose()
    }

    /// Atomically reads progress and all eight owner receipts.
    ///
    /// A series of strong `GetItem`s is not sufficient: a concurrent owner
    /// write could otherwise produce a mixed snapshot and a false completion
    /// or corruption result.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError`] when the transactional read cannot be built or
    /// completed, or when its progress/evidence rows are malformed or mutually
    /// inconsistent.
    pub async fn load_snapshot(
        &self,
        workspace: WorkspaceId,
        session: SessionId,
    ) -> Result<Option<SessionDeletionSnapshot>, StoreError> {
        let reads = snapshot_gets(&self.table, session)?;
        let output = self
            .dynamodb
            .transact_get_items()
            .set_transact_items(Some(reads))
            .send()
            .await
            .map_err(|error| classify(&error, Idempotence::Read))?;
        let responses = output.responses.unwrap_or_default();
        if responses.len() != 1 + DeletionOwner::ALL.len() {
            return Err(StoreError::Invalid {
                detail: "the deletion snapshot returned an unexpected response count".to_owned(),
            });
        }
        let mut responses = responses.into_iter();
        let progress_item = responses.next().and_then(|response| response.item);
        let evidence_items = responses
            .map(|response| response.item)
            .collect::<Vec<Option<Item>>>();
        let Some(progress_item) = progress_item else {
            if evidence_items.iter().any(Option::is_some) {
                return Err(StoreError::Invalid {
                    detail: "deletion owner evidence exists without its progress root".to_owned(),
                });
            }
            return Ok(None);
        };
        let progress = decode_progress(&progress_item, workspace, session)?;
        let mut evidence = BTreeMap::new();
        for (expected, item) in DeletionOwner::ALL.into_iter().zip(evidence_items) {
            let Some(item) = item else {
                continue;
            };
            let decoded = decode_evidence(&item, workspace, session)?;
            if decoded.owner != expected || !decoded.belongs_to(progress) {
                return Err(StoreError::Invalid {
                    detail: "deletion evidence disagrees with its atomic progress snapshot"
                        .to_owned(),
                });
            }
            evidence.insert(decoded.owner, decoded);
        }
        Ok(Some(SessionDeletionSnapshot { progress, evidence }))
    }

    /// Strongly queries one bounded page of receipt locators.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::Invalid`] for a zero/oversized page or malformed
    /// cursor, and [`StoreError`] for an unavailable/corrupt authority read.
    pub async fn list_receipts(
        &self,
        workspace: WorkspaceId,
        session: SessionId,
        after: Option<&str>,
        limit: u8,
    ) -> Result<SessionReceiptDirectoryPage, StoreError> {
        if limit == 0 || limit > RECEIPT_DIRECTORY_PAGE_MAX {
            return Err(StoreError::Invalid {
                detail: format!(
                    "a receipt-directory page must contain 1..={RECEIPT_DIRECTORY_PAGE_MAX} rows"
                ),
            });
        }
        if after.is_some_and(|cursor| !cursor.starts_with(keys::RECEIPT_DIRECTORY_PREFIX)) {
            return Err(StoreError::Invalid {
                detail: "a receipt-directory cursor is outside the directory prefix".to_owned(),
            });
        }
        let partition = keys::session_partition(session);
        let mut query = self
            .dynamodb
            .query()
            .table_name(&self.table)
            .consistent_read(true)
            .key_condition_expression("pk = :pk AND begins_with(sk, :prefix)")
            .expression_attribute_values(":pk", s(partition.clone()))
            .expression_attribute_values(":prefix", s(keys::RECEIPT_DIRECTORY_PREFIX))
            .limit(i32::from(limit));
        if let Some(cursor) = after {
            query = query
                .exclusive_start_key(PK, s(partition))
                .exclusive_start_key(SK, s(cursor));
        }
        let output = query
            .send()
            .await
            .map_err(|error| classify(&error, Idempotence::Read))?;
        let entries = output
            .items
            .unwrap_or_default()
            .iter()
            .map(|item| decode_receipt_directory(item, workspace, session))
            .collect::<Result<Vec<_>, _>>()?;
        let next = output
            .last_evaluated_key
            .as_ref()
            .map(|cursor| {
                cursor
                    .get(SK)
                    .and_then(|value| value.as_s().ok())
                    .filter(|value| value.starts_with(keys::RECEIPT_DIRECTORY_PREFIX))
                    .cloned()
                    .ok_or_else(|| StoreError::Invalid {
                        detail: "the receipt-directory continuation key is malformed".to_owned(),
                    })
            })
            .transpose()?;
        Ok(SessionReceiptDirectoryPage { entries, next })
    }
}

/// Encodes the immutable progress root.
///
/// # Errors
///
/// Returns [`StoreError::Invalid`] when the progress identity has a zero epoch
/// or otherwise fails its domain validation.
pub fn encode_progress(progress: SessionDeletionProgress) -> Result<Item, StoreError> {
    let progress = progress.validate()?;
    let target = keys::deletion_progress(progress.session);
    Ok(ItemBuilder::new(codec::SESSION_DELETION_PROGRESS)
        .set(PK, s(target.pk))
        .set(SK, s(target.sk))
        .set("workspaceId", s(progress.workspace.to_string()))
        .set("sessionId", s(progress.session.to_string()))
        .set("operationId", s(progress.operation.to_string()))
        .set("deletionEpoch", n(progress.epoch.0))
        .set("state", s(PROGRESS_STATE))
        .set("startedAt", stamp(progress.started_at))
        .build())
}

/// Strictly decodes one progress root under an asserted tenant/session.
///
/// # Errors
///
/// Returns [`CodecError`] when the row is malformed, belongs to another
/// workspace or session, or does not match its canonical physical key.
pub fn decode_progress(
    item: &Item,
    workspace: WorkspaceId,
    session: SessionId,
) -> Result<SessionDeletionProgress, CodecError> {
    let row = Row::bind(item, codec::SESSION_DELETION_PROGRESS)?;
    row.owned_by("workspaceId", &workspace.to_string())?;
    let progress = SessionDeletionProgress {
        workspace,
        session: row.id("sessionId")?,
        operation: row.id("operationId")?,
        epoch: DeletionEpoch(row.u64("deletionEpoch")?),
        started_at: row.timestamp("startedAt")?,
    };
    let target = keys::deletion_progress(session);
    if progress.session != session
        || progress.epoch.0 == 0
        || row.enumerated("state", &[PROGRESS_STATE])? != PROGRESS_STATE
        || row.string(PK)? != target.pk
        || row.string(SK)? != target.sk
    {
        return Err(malformed(
            codec::SESSION_DELETION_PROGRESS,
            "sessionId",
            "the progress identity does not match its exact key",
        ));
    }
    Ok(progress)
}

/// Encodes one immutable owner receipt.
///
/// # Errors
///
/// Returns [`StoreError::Invalid`] when the evidence carries a zero deletion
/// epoch.
pub fn encode_evidence(evidence: SessionDeletionEvidence) -> Result<Item, StoreError> {
    if evidence.epoch.0 == 0 {
        return Err(StoreError::Invalid {
            detail: "session deletion evidence requires a non-zero epoch".to_owned(),
        });
    }
    let target = keys::deletion_evidence(evidence.session, evidence.owner.as_str());
    Ok(ItemBuilder::new(codec::SESSION_DELETION_EVIDENCE)
        .set(PK, s(target.pk))
        .set(SK, s(target.sk))
        .set("workspaceId", s(evidence.workspace.to_string()))
        .set("sessionId", s(evidence.session.to_string()))
        .set("operationId", s(evidence.operation.to_string()))
        .set("deletionEpoch", n(evidence.epoch.0))
        .set("owner", s(evidence.owner.as_str()))
        .set("proofSha256", s(evidence.proof.as_hex()))
        .set("completedAt", stamp(evidence.completed_at))
        .build())
}

/// Strictly decodes one owner receipt.
///
/// # Errors
///
/// Returns [`CodecError`] when the row is malformed, names an unknown owner,
/// belongs to another authority scope, or does not match its canonical key.
pub fn decode_evidence(
    item: &Item,
    workspace: WorkspaceId,
    session: SessionId,
) -> Result<SessionDeletionEvidence, CodecError> {
    let row = Row::bind(item, codec::SESSION_DELETION_EVIDENCE)?;
    row.owned_by("workspaceId", &workspace.to_string())?;
    let owner = DeletionOwner::parse(row.string("owner")?).ok_or_else(|| {
        malformed(
            codec::SESSION_DELETION_EVIDENCE,
            "owner",
            "the evidence owner is outside the closed vocabulary",
        )
    })?;
    let proof = EvidenceDigest::parse(row.string("proofSha256")?).ok_or_else(|| {
        malformed(
            codec::SESSION_DELETION_EVIDENCE,
            "proofSha256",
            "the evidence digest is not an exact SHA-256",
        )
    })?;
    let evidence = SessionDeletionEvidence {
        workspace,
        session: row.id("sessionId")?,
        operation: row.id("operationId")?,
        epoch: DeletionEpoch(row.u64("deletionEpoch")?),
        owner,
        proof,
        completed_at: row.timestamp("completedAt")?,
    };
    let target = keys::deletion_evidence(session, owner.as_str());
    if evidence.session != session
        || evidence.epoch.0 == 0
        || row.string(PK)? != target.pk
        || row.string(SK)? != target.sk
    {
        return Err(malformed(
            codec::SESSION_DELETION_EVIDENCE,
            "sessionId",
            "the evidence identity does not match its exact key",
        ));
    }
    Ok(evidence)
}

/// Encodes one receipt-directory locator.
///
/// # Errors
///
/// Returns [`StoreError::Invalid`] when the locator's receipt key, scope, or
/// target digest is internally inconsistent.
pub fn encode_receipt_directory(entry: &SessionReceiptDirectoryEntry) -> Result<Item, StoreError> {
    entry.validate()?;
    let target = entry.key();
    Ok(ItemBuilder::new(codec::SESSION_RECEIPT_DIRECTORY)
        .set(PK, s(target.pk))
        .set(SK, s(target.sk))
        .set("workspaceId", s(entry.workspace.to_string()))
        .set("sessionId", s(entry.session.to_string()))
        .set("receiptPk", s(entry.receipt_pk.clone()))
        .set("receiptSk", s(entry.receipt_sk.clone()))
        .set("scope", s(entry.scope.clone()))
        .set("targetSha256", s(entry.target.as_hex()))
        .set("createdAt", stamp(entry.created_at))
        .build())
}

/// Strictly decodes one receipt-directory locator.
///
/// # Errors
///
/// Returns [`CodecError`] when the row is malformed, belongs to another
/// authority scope, or disagrees with its receipt target or canonical key.
pub fn decode_receipt_directory(
    item: &Item,
    workspace: WorkspaceId,
    session: SessionId,
) -> Result<SessionReceiptDirectoryEntry, CodecError> {
    let row = Row::bind(item, codec::SESSION_RECEIPT_DIRECTORY)?;
    row.owned_by("workspaceId", &workspace.to_string())?;
    let stored_session = row.id("sessionId")?;
    let receipt_partition_key = row.string("receiptPk")?.to_owned();
    let receipt_sort_key = row.string("receiptSk")?.to_owned();
    let scope = row.string("scope")?.to_owned();
    let target = EvidenceDigest::parse(row.string("targetSha256")?).ok_or_else(|| {
        malformed(
            codec::SESSION_RECEIPT_DIRECTORY,
            "targetSha256",
            "the receipt target digest is not an exact SHA-256",
        )
    })?;
    let entry = SessionReceiptDirectoryEntry {
        workspace,
        session: stored_session,
        receipt_pk: receipt_partition_key,
        receipt_sk: receipt_sort_key,
        scope,
        target,
        created_at: row.timestamp("createdAt")?,
    };
    let expected_target = receipt_target(&entry.receipt_pk, &entry.receipt_sk);
    let directory_key = keys::receipt_directory(session, &target.as_hex());
    let expected_scope = receipt_scope(workspace, session, &entry.receipt_pk);
    if entry.session != session
        || entry.receipt_sk != RECEIPT_SK
        || expected_scope != Some(entry.scope.as_str())
        || entry.target != expected_target
        || row.string(PK)? != directory_key.pk
        || row.string(SK)? != directory_key.sk
    {
        return Err(malformed(
            codec::SESSION_RECEIPT_DIRECTORY,
            "receiptPk",
            "the receipt locator does not match its tenant, target or exact key",
        ));
    }
    Ok(entry)
}

/// Compiles initialization after the application has atomically elected the
/// session head's operation and epoch.
///
/// This two-item transaction cannot initialize progress against a live or
/// differently owned head.
///
/// # Errors
///
/// Returns [`StoreError`] when the progress identity is invalid or either the
/// head guard or immutable progress write cannot be compiled into the plan.
pub fn compile_initialize_progress(
    table: &str,
    progress: SessionDeletionProgress,
) -> Result<TransactionPlan, StoreError> {
    let progress = progress.validate()?;
    let head = keys::head(progress.session);
    let mut plan = TransactionPlan::new(format!("delete-init:{}", progress.operation));
    plan.condition_check(
        SESSION_HEAD_GUARD,
        ConditionCheckBuilder::default()
            .table_name(table)
            .set_key(Some(key(&head.pk, &head.sk)))
            .condition_expression(
                "itemType = :head AND workspaceId = :workspace AND sessionId = :session AND \
                 lifecycle = :deleting AND deletionEpoch = :epoch AND \
                 mutationGuardOperationId = :operation",
            )
            .expression_attribute_values(":head", s(codec::SESSION_DELETION_HEAD))
            .expression_attribute_values(":workspace", s(progress.workspace.to_string()))
            .expression_attribute_values(":session", s(progress.session.to_string()))
            .expression_attribute_values(":deleting", s(PROGRESS_STATE))
            .expression_attribute_values(":epoch", n(progress.epoch.0))
            .expression_attribute_values(":operation", s(progress.operation.to_string())),
    )?;
    plan.put(
        DELETION_PROGRESS,
        PutBuilder::default()
            .table_name(table)
            .set_item(Some(encode_progress(progress)?))
            .condition_expression(IMMUTABLE),
    )?;
    Ok(plan)
}

/// Compiles one immutable owner receipt behind the exact progress identity.
///
/// # Errors
///
/// Returns [`StoreError`] when progress is invalid, the evidence does not
/// belong to it, or the guarded transaction cannot be compiled.
pub fn compile_record_evidence(
    table: &str,
    progress: SessionDeletionProgress,
    evidence: SessionDeletionEvidence,
) -> Result<TransactionPlan, StoreError> {
    let progress = progress.validate()?;
    if !evidence.belongs_to(progress) {
        return Err(StoreError::Invalid {
            detail: "deletion evidence is not bound to the asserted operation and epoch".to_owned(),
        });
    }
    let mut plan = TransactionPlan::new(format!(
        "delete-proof:{}:{}",
        progress.operation,
        evidence.owner.as_str()
    ));
    append_progress_guard(table, progress, &mut plan)?;
    plan.put(
        DELETION_EVIDENCE,
        PutBuilder::default()
            .table_name(table)
            .set_item(Some(encode_evidence(evidence)?))
            .condition_expression(IMMUTABLE),
    )?;
    Ok(plan)
}

/// Appends exact conditional deletion of every progress/evidence row to the
/// final operation/work/tombstone transaction.
///
/// The deletes are the guards: each one asserts the exact operation, epoch,
/// owner and proof before removing the now-spent content-free scaffold. This
/// leaves the session partition with the minimal tombstone rather than a
/// second permanent deletion-state model.
///
/// # Errors
///
/// Returns [`StoreError::Invalid`] unless all exact owner receipts are present
/// in the supplied atomic snapshot.
pub fn append_completion_cleanup(
    table: &str,
    snapshot: &SessionDeletionSnapshot,
    plan: &mut TransactionPlan,
) -> Result<(), StoreError> {
    if !snapshot.is_complete() {
        return Err(StoreError::Invalid {
            detail: "the deletion completion cleanup is missing owner evidence".to_owned(),
        });
    }
    for owner in DeletionOwner::ALL {
        let evidence = snapshot
            .evidence
            .get(&owner)
            .ok_or_else(|| StoreError::Invalid {
                detail: format!(
                    "the deletion completion cleanup is missing `{}`",
                    owner.as_str()
                ),
            })?;
        if !evidence.belongs_to(snapshot.progress) {
            return Err(StoreError::Invalid {
                detail: "the deletion completion cleanup contains cross-operation evidence"
                    .to_owned(),
            });
        }
        let target = keys::deletion_evidence(snapshot.progress.session, owner.as_str());
        plan.delete(
            DELETION_EVIDENCE,
            DeleteBuilder::default()
                .table_name(table)
                .set_key(Some(key(&target.pk, &target.sk)))
                .condition_expression(
                    "itemType = :item AND workspaceId = :workspace AND sessionId = :session AND \
                     operationId = :operation AND deletionEpoch = :epoch AND owner = :owner AND \
                     proofSha256 = :proof",
                )
                .expression_attribute_values(":item", s(codec::SESSION_DELETION_EVIDENCE))
                .expression_attribute_values(
                    ":workspace",
                    s(snapshot.progress.workspace.to_string()),
                )
                .expression_attribute_values(":session", s(snapshot.progress.session.to_string()))
                .expression_attribute_values(
                    ":operation",
                    s(snapshot.progress.operation.to_string()),
                )
                .expression_attribute_values(":epoch", n(snapshot.progress.epoch.0))
                .expression_attribute_values(":owner", s(owner.as_str()))
                .expression_attribute_values(":proof", s(evidence.proof.as_hex())),
        )?;
    }
    let progress = keys::deletion_progress(snapshot.progress.session);
    plan.delete(
        DELETION_PROGRESS,
        DeleteBuilder::default()
            .table_name(table)
            .set_key(Some(key(&progress.pk, &progress.sk)))
            .condition_expression(
                "itemType = :item AND workspaceId = :workspace AND sessionId = :session AND \
                 operationId = :operation AND deletionEpoch = :epoch AND #state = :deleting",
            )
            .expression_attribute_names("#state", "state")
            .expression_attribute_values(":item", s(codec::SESSION_DELETION_PROGRESS))
            .expression_attribute_values(":workspace", s(snapshot.progress.workspace.to_string()))
            .expression_attribute_values(":session", s(snapshot.progress.session.to_string()))
            .expression_attribute_values(":operation", s(snapshot.progress.operation.to_string()))
            .expression_attribute_values(":epoch", n(snapshot.progress.epoch.0))
            .expression_attribute_values(":deleting", s(PROGRESS_STATE)),
    )?;
    Ok(())
}

/// Builds the conditional HEAD replacement for the final barrier.
///
/// The caller must append [`append_completion_cleanup`] plus the canonical
/// operation and work transitions to the same transaction. This function only
/// owns the physical head condition and minimal tombstone encoding.
///
/// # Errors
///
/// Returns [`StoreError::Invalid`] when progress is invalid or the tombstone is
/// not bound to its exact workspace, session, operation, and deletion epoch.
pub fn tombstone_put(
    table: &str,
    progress: SessionDeletionProgress,
    tombstone: &SessionTombstone,
    expected_revision: u64,
) -> Result<PutBuilder, StoreError> {
    let progress = progress.validate()?;
    if tombstone.workspace != progress.workspace
        || tombstone.session != progress.session
        || tombstone.deleted_by != progress.operation
        || tombstone.epoch != progress.epoch
    {
        return Err(StoreError::Invalid {
            detail: "the tombstone is not bound to the exact deletion progress identity".to_owned(),
        });
    }
    Ok(PutBuilder::default()
        .table_name(table)
        .set_item(Some(crate::authority_codec::encode_session_tombstone(
            tombstone,
        )))
        .condition_expression(
            "itemType = :head AND workspaceId = :workspace AND sessionId = :session AND \
             revision = :revision AND lifecycle = :deleting AND deletionEpoch = :epoch AND \
             mutationGuardOperationId = :operation",
        )
        .expression_attribute_values(":head", s(codec::SESSION_DELETION_HEAD))
        .expression_attribute_values(":workspace", s(progress.workspace.to_string()))
        .expression_attribute_values(":session", s(progress.session.to_string()))
        .expression_attribute_values(":revision", n(expected_revision))
        .expression_attribute_values(":deleting", s(PROGRESS_STATE))
        .expression_attribute_values(":epoch", n(progress.epoch.0))
        .expression_attribute_values(":operation", s(progress.operation.to_string())))
}

/// Closed compiler for the final tombstone item in a multi-family settlement.
///
/// Operation and work rows remain compiled by their owning adapters. This
/// compiler owns only the HEAD replacement and refuses every other action.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DeletionTombstoneCompiler {
    progress: SessionDeletionProgress,
    expected_revision: u64,
}

impl DeletionTombstoneCompiler {
    /// Binds the exact progress root and scrubbed-head revision observed by the
    /// final atomic snapshot.
    #[must_use]
    pub const fn new(progress: SessionDeletionProgress, expected_revision: u64) -> Self {
        Self {
            progress,
            expected_revision,
        }
    }
}

impl crate::application_plan::ExternalActionCompiler for DeletionTombstoneCompiler {
    fn compile_action(
        &self,
        tables: &crate::plan::RegionalTables,
        binding: crate::application_plan::AuthorityBinding,
        action: &crate::application_plan::LogicalAction<'_>,
        output: &mut TransactionPlan,
    ) -> Result<(), StoreError> {
        let Some(aex_session_app::plan::Write::PutTombstone(tombstone)) = action.write else {
            return Err(StoreError::Invalid {
                detail: "the deletion tombstone compiler received a non-tombstone action"
                    .to_owned(),
            });
        };
        if binding.workspace != self.progress.workspace
            || binding.session != Some(self.progress.session)
            || action.conditions.len() != 2
            || !action.conditions.iter().any(|(_, condition)| {
                matches!(
                    condition,
                    aex_session_app::plan::Condition::SessionRevision { session, expected }
                        if *session == self.progress.session
                            && expected.0 == self.expected_revision
                )
            })
            || !action.conditions.iter().any(|(_, condition)| {
                matches!(
                    condition,
                    aex_session_app::plan::Condition::MutationGuardHeldBy { session, holder }
                        if *session == self.progress.session
                            && *holder == self.progress.operation
                )
            })
        {
            return Err(StoreError::Invalid {
                detail: "the tombstone action is not bound to the scrubbed deletion head"
                    .to_owned(),
            });
        }
        output.put(
            Participant::SESSION_HEAD,
            tombstone_put(
                &tables.session_authority,
                self.progress,
                tombstone,
                self.expected_revision,
            )?,
        )?;
        Ok(())
    }
}

/// Builds the immutable directory write which must accompany its receipt put
/// in the same application transaction.
///
/// # Errors
///
/// Returns [`StoreError::Invalid`] for a forged or internally inconsistent
/// locator.
pub fn receipt_directory_put(
    table: &str,
    entry: &SessionReceiptDirectoryEntry,
) -> Result<PutBuilder, StoreError> {
    Ok(PutBuilder::default()
        .table_name(table)
        .set_item(Some(encode_receipt_directory(entry)?))
        .condition_expression(IMMUTABLE))
}

/// Compiles conditional deletion of one located receipt and its directory row.
///
/// The progress guard makes a stale or differently owned continuation fail
/// before it can delete anything. A receipt already reclaimed by TTL is an
/// idempotent success; an unrelated row at the addressed key is not.
///
/// # Errors
///
/// Returns [`StoreError`] when progress or the locator is invalid, their scopes
/// differ, or the guarded receipt/directory transaction cannot be compiled.
pub fn compile_delete_receipt(
    table: &str,
    progress: SessionDeletionProgress,
    entry: &SessionReceiptDirectoryEntry,
) -> Result<TransactionPlan, StoreError> {
    let progress = progress.validate()?;
    if entry.workspace != progress.workspace || entry.session != progress.session {
        return Err(StoreError::Invalid {
            detail: "the receipt directory entry is outside the deletion progress scope".to_owned(),
        });
    }
    entry.validate()?;
    let mut plan = TransactionPlan::new(format!(
        "delete-receipt:{}:{}",
        progress.operation,
        entry.target.as_hex()
    ));
    append_progress_guard(table, progress, &mut plan)?;
    plan.delete(
        RECEIPT_PAYLOAD,
        DeleteBuilder::default()
            .table_name(table)
            .set_key(Some(key(&entry.receipt_pk, &entry.receipt_sk)))
            .condition_expression(
                "attribute_not_exists(pk) OR (itemType = :receipt AND #scope = :scope)",
            )
            .expression_attribute_names("#scope", "scope")
            .expression_attribute_values(":receipt", s(RECEIPT_ITEM_TYPE))
            .expression_attribute_values(":scope", s(entry.scope.clone())),
    )?;
    let directory = entry.key();
    plan.delete(
        RECEIPT_DIRECTORY,
        DeleteBuilder::default()
            .table_name(table)
            .set_key(Some(key(&directory.pk, &directory.sk)))
            .condition_expression(
                "itemType = :directory AND workspaceId = :workspace AND sessionId = :session AND \
                 receiptPk = :receiptPk AND receiptSk = :receiptSk AND #scope = :scope AND \
                 targetSha256 = :target",
            )
            .expression_attribute_names("#scope", "scope")
            .expression_attribute_values(":directory", s(codec::SESSION_RECEIPT_DIRECTORY))
            .expression_attribute_values(":workspace", s(progress.workspace.to_string()))
            .expression_attribute_values(":session", s(progress.session.to_string()))
            .expression_attribute_values(":receiptPk", s(entry.receipt_pk.clone()))
            .expression_attribute_values(":receiptSk", s(entry.receipt_sk.clone()))
            .expression_attribute_values(":scope", s(entry.scope.clone()))
            .expression_attribute_values(":target", s(entry.target.as_hex())),
    )?;
    Ok(plan)
}

/// Compiles one bounded page of exact payload-row deletes behind the durable
/// progress identity.
///
/// The continuation must first validate each row's workspace/session binding
/// through its owning codec or query projection. This compiler then prevents a
/// stale continuation from deleting after another operation takes authority,
/// and prevents a key reused for another item family from being removed.
///
/// # Errors
///
/// Returns [`StoreError`] when progress is invalid, the page is empty or above
/// its bound, a target addresses protected deletion authority, or a guarded
/// delete cannot be compiled.
pub fn compile_guarded_deletes(
    session_table: &str,
    progress: SessionDeletionProgress,
    targets: &[DeletionTarget],
) -> Result<TransactionPlan, StoreError> {
    let progress = progress.validate()?;
    if targets.is_empty() || targets.len() > DELETION_PAGE_MAX {
        return Err(StoreError::Invalid {
            detail: format!("a guarded deletion page must contain 1..={DELETION_PAGE_MAX} rows"),
        });
    }
    let head = keys::head(progress.session);
    let deletion_progress = keys::deletion_progress(progress.session);
    let delete_operation = keys::operation(progress.operation);
    let mut plan = TransactionPlan::new(format!(
        "delete-page:{}:{}",
        progress.operation,
        targets.len()
    ));
    append_progress_guard(session_table, progress, &mut plan)?;
    for target in targets {
        if target.table.is_empty()
            || target.pk.is_empty()
            || target.sk.is_empty()
            || target.item_type.is_empty()
            || (target.pk == head.pk && target.sk == head.sk)
            || (target.pk == deletion_progress.pk && target.sk == deletion_progress.sk)
            || (target.pk == delete_operation.pk && target.sk == delete_operation.sk)
            || (target.pk == keys::session_partition(progress.session)
                && (target.sk.starts_with(keys::DELETION_EVIDENCE_PREFIX)
                    || target.sk.starts_with(keys::RECEIPT_DIRECTORY_PREFIX)))
        {
            return Err(StoreError::Invalid {
                detail: "a guarded deletion page addressed protected deletion authority".to_owned(),
            });
        }
        plan.delete(
            OWNED_PAYLOAD,
            DeleteBuilder::default()
                .table_name(&target.table)
                .set_key(Some(key(&target.pk, &target.sk)))
                .condition_expression("itemType = :item")
                .expression_attribute_values(":item", s(target.item_type.clone())),
        )?;
    }
    Ok(plan)
}

fn append_progress_guard(
    table: &str,
    progress: SessionDeletionProgress,
    plan: &mut TransactionPlan,
) -> Result<(), StoreError> {
    let target = keys::deletion_progress(progress.session);
    plan.condition_check(
        DELETION_PROGRESS,
        ConditionCheckBuilder::default()
            .table_name(table)
            .set_key(Some(key(&target.pk, &target.sk)))
            .condition_expression(
                "itemType = :item AND workspaceId = :workspace AND sessionId = :session AND \
                 operationId = :operation AND deletionEpoch = :epoch AND #state = :deleting",
            )
            .expression_attribute_names("#state", "state")
            .expression_attribute_values(":item", s(codec::SESSION_DELETION_PROGRESS))
            .expression_attribute_values(":workspace", s(progress.workspace.to_string()))
            .expression_attribute_values(":session", s(progress.session.to_string()))
            .expression_attribute_values(":operation", s(progress.operation.to_string()))
            .expression_attribute_values(":epoch", n(progress.epoch.0))
            .expression_attribute_values(":deleting", s(PROGRESS_STATE)),
    )?;
    Ok(())
}

fn snapshot_gets(table: &str, session: SessionId) -> Result<Vec<TransactGetItem>, StoreError> {
    let mut targets = Vec::with_capacity(1 + DeletionOwner::ALL.len());
    targets.push(keys::deletion_progress(session));
    targets.extend(
        DeletionOwner::ALL
            .into_iter()
            .map(|owner| keys::deletion_evidence(session, owner.as_str())),
    );
    targets
        .into_iter()
        .map(|target| {
            let get = Get::builder()
                .table_name(table)
                .set_key(Some(key(&target.pk, &target.sk)))
                .build()
                .map_err(|error| StoreError::Invalid {
                    detail: format!("a deletion snapshot read could not be built: {error}"),
                })?;
            Ok(TransactGetItem::builder().get(get).build())
        })
        .collect()
}

fn receipt_target(pk: &str, sk: &str) -> EvidenceDigest {
    let mut hasher = Sha256::new();
    hasher.update(pk.as_bytes());
    hasher.update([0]);
    hasher.update(sk.as_bytes());
    EvidenceDigest(hasher.finalize().into())
}

fn receipt_scope(workspace: WorkspaceId, session: SessionId, pk: &str) -> Option<&str> {
    let rest = pk.strip_prefix(&format!("IDEM#{workspace}#"))?;
    let (scope, digest) = rest.rsplit_once('#')?;
    EvidenceDigest::parse(digest)?;
    let exact_session = scope.rsplit_once(':').is_some_and(|(base, subject)| {
        base.starts_with("session.")
            && crate::replay::IdempotencyScope::BASES.contains(&base)
            && subject == session.to_string()
    });
    (scope == "session.create" || exact_session).then_some(scope)
}

fn malformed(
    item_type: &'static str,
    attribute: &'static str,
    reason: impl Into<String>,
) -> CodecError {
    CodecError::Malformed {
        item_type,
        attribute,
        reason: reason.into(),
    }
}

#[cfg(test)]
mod tests {
    use aex_wire::ids::{OperationId, SessionId, Uuid7, WorkspaceId};

    use super::*;

    fn id<T: aex_wire::ids::PrefixedId>(tag: u8) -> T {
        T::from_uuid7(Uuid7::compose(1, [tag; 10]))
    }

    fn at(millis: i64) -> Timestamp {
        Timestamp::from_unix_millis(millis).expect("fixture timestamp")
    }

    fn progress() -> SessionDeletionProgress {
        SessionDeletionProgress {
            workspace: id::<WorkspaceId>(1),
            session: id::<SessionId>(2),
            operation: id::<OperationId>(3),
            epoch: DeletionEpoch(4),
            started_at: at(5),
        }
    }

    fn evidence(owner: DeletionOwner) -> SessionDeletionEvidence {
        let progress = progress();
        SessionDeletionEvidence {
            workspace: progress.workspace,
            session: progress.session,
            operation: progress.operation,
            epoch: progress.epoch,
            owner,
            proof: EvidenceDigest::from_proof(owner.as_str().as_bytes()),
            completed_at: at(6),
        }
    }

    #[test]
    fn progress_and_owner_evidence_round_trip_exact_identity() {
        let progress = progress();
        let progress_item = encode_progress(progress).expect("progress encodes");
        assert_eq!(
            decode_progress(&progress_item, progress.workspace, progress.session),
            Ok(progress)
        );
        for owner in DeletionOwner::ALL {
            let evidence = evidence(owner);
            let item = encode_evidence(evidence).expect("evidence encodes");
            assert_eq!(
                decode_evidence(&item, evidence.workspace, evidence.session),
                Ok(evidence)
            );
        }
    }

    #[test]
    fn completion_is_derived_only_from_atomic_owner_rows() {
        let progress = progress();
        let mut snapshot = SessionDeletionSnapshot {
            progress,
            evidence: DeletionOwner::ALL
                .into_iter()
                .map(|owner| (owner, evidence(owner)))
                .collect(),
        };
        assert!(snapshot.is_complete());
        snapshot.evidence.remove(&DeletionOwner::AuditFact);
        assert!(!snapshot.is_complete());
        assert!(!snapshot.domain_evidence().audit_fact_retained);
    }

    #[test]
    fn completion_cleanup_conditionally_removes_all_spent_scaffold() {
        let progress = progress();
        let snapshot = SessionDeletionSnapshot {
            progress,
            evidence: DeletionOwner::ALL
                .into_iter()
                .map(|owner| (owner, evidence(owner)))
                .collect(),
        };
        let mut plan = TransactionPlan::new("delete-complete:test");
        append_completion_cleanup("session-authority", &snapshot, &mut plan)
            .expect("complete cleanup");
        assert_eq!(plan.len(), 1 + DeletionOwner::ALL.len());
        assert!(plan.actions().iter().all(|action| {
            action
                .delete()
                .and_then(|delete| delete.condition_expression())
                .is_some_and(|condition| !condition.is_empty())
        }));
    }

    #[test]
    fn evidence_compiler_guards_progress_before_immutable_proof() {
        let plan = compile_record_evidence(
            "dev-eu-west-1-session-authority",
            progress(),
            evidence(DeletionOwner::Messages),
        )
        .expect("conditional plan");
        assert_eq!(plan.len(), 2);
        assert_eq!(plan.participants(), &[DELETION_PROGRESS, DELETION_EVIDENCE]);
        assert!(plan.actions()[0].condition_check().is_some());
        assert_eq!(
            plan.actions()[1]
                .put()
                .and_then(|put| put.condition_expression()),
            Some(IMMUTABLE)
        );
    }

    #[test]
    fn receipt_directory_is_exact_enumerable_and_cross_tenant_safe() {
        let progress = progress();
        let key_digest = EvidenceDigest::from_proof(b"key").as_hex();
        let pk = format!(
            "IDEM#{}#session.message:{}#{key_digest}",
            progress.workspace, progress.session
        );
        let entry = SessionReceiptDirectoryEntry::new(
            progress.workspace,
            progress.session,
            pk,
            RECEIPT_SK,
            at(7),
        )
        .expect("valid receipt locator");
        let item = encode_receipt_directory(&entry).expect("directory encodes");
        assert_eq!(
            decode_receipt_directory(&item, progress.workspace, progress.session),
            Ok(entry.clone())
        );
        assert!(decode_receipt_directory(&item, id::<WorkspaceId>(9), progress.session).is_err());
        assert!(
            SessionReceiptDirectoryEntry::new(
                progress.workspace,
                progress.session,
                format!(
                    "IDEM#another-workspace#session.message:{}#{key_digest}",
                    progress.session
                ),
                RECEIPT_SK,
                at(7),
            )
            .is_err()
        );
    }

    #[test]
    fn receipt_cleanup_is_progress_guarded_and_never_unconditional() {
        let progress = progress();
        let entry = SessionReceiptDirectoryEntry::new(
            progress.workspace,
            progress.session,
            format!(
                "IDEM#{}#session.message:{}#{}",
                progress.workspace,
                progress.session,
                EvidenceDigest::from_proof(b"key").as_hex()
            ),
            RECEIPT_SK,
            at(7),
        )
        .expect("valid receipt locator");
        let plan = compile_delete_receipt("dev-eu-west-1-session-authority", progress, &entry)
            .expect("conditional cleanup");
        assert_eq!(plan.len(), 3);
        assert_eq!(
            plan.participants(),
            &[DELETION_PROGRESS, RECEIPT_PAYLOAD, RECEIPT_DIRECTORY]
        );
        for action in &plan.actions()[1..] {
            assert!(
                action
                    .delete()
                    .and_then(|delete| delete.condition_expression())
                    .is_some_and(|condition| !condition.is_empty())
            );
        }
    }

    #[test]
    fn atomic_snapshot_addresses_progress_then_every_owner() {
        let progress = progress();
        let gets = snapshot_gets("session-authority", progress.session).expect("snapshot reads");
        assert_eq!(gets.len(), 1 + DeletionOwner::ALL.len());
        let first = gets[0].get().expect("progress get");
        assert_eq!(
            first
                .key()
                .get(SK)
                .and_then(|value| value.as_s().ok())
                .map(String::as_str),
            Some(keys::DELETION_PROGRESS_SK)
        );
    }

    #[test]
    fn tombstone_put_is_exact_head_replacement_not_a_free_standing_write() {
        let progress = progress();
        let tombstone = SessionTombstone {
            session: progress.session,
            workspace: progress.workspace,
            deleted_by: progress.operation,
            epoch: progress.epoch,
            deleted_at: at(8),
        };
        let put = tombstone_put("session-authority", progress, &tombstone, 9)
            .expect("conditional tombstone")
            .build()
            .expect("complete put");
        assert_eq!(put.item().len(), 8);
        assert!(
            put.condition_expression()
                .is_some_and(|condition| condition.contains("mutationGuardOperationId"))
        );
    }
}
