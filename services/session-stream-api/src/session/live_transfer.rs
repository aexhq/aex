//! Durable election and ownership authority for ephemeral live-file transfers.
//!
//! File bytes never enter this table. The rows elect one public transfer id
//! before a guest effect, bind it to one workspace/session/generation, and keep
//! the exact replay response. The `MicroVM` remains the only byte store.

use std::sync::Arc;

use aex_hands_protocol::files::FileDownloadState;
use aex_session_dynamodb::attr::{Item, ItemBuilder, PK, Row, SK, b, n, s, stamp};
use aex_session_dynamodb::error::{
    Idempotence, Resolution, StoreError, classify, decode_cancellation_with_resolution,
};
use aex_session_dynamodb::plan::{Participant, TransactionPlan};
use aex_wire::ids::{
    ContentHash, FileDownloadId, FilePath, FileUploadId, GenerationId, SessionId, WorkspaceId,
};
use aex_wire::models::RegisteredFileMode;
use aex_wire::types::Timestamp;
use async_trait::async_trait;
use aws_sdk_dynamodb::Client;
use serde::{Deserialize, Serialize};

const TRANSFER_ITEM: &str = "live_file_transfer";
const ELECTION_ITEM: &str = "live_file_election";
const TRANSFER_PARTICIPANT: Participant = Participant::new("session.live_file_transfer");
const ELECTION_PARTICIPANT: Participant = Participant::new("session.live_file_election");

/// One generation-local transfer identity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "id", rename_all = "snake_case")]
pub enum TransferKey {
    /// A resumable upload.
    Upload(FileUploadId),
    /// An exact-version download.
    Download(FileDownloadId),
}

impl TransferKey {
    fn sort_key(self) -> String {
        match self {
            Self::Upload(id) => format!("LIVEFILE#UPLOAD#{id}"),
            Self::Download(id) => format!("LIVEFILE#DOWNLOAD#{id}"),
        }
    }
}

/// Durable transfer lifecycle. TTL cleanup is never interpreted as the fence.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TransferState {
    /// Elected before the guest open effect.
    Opening,
    /// Guest upload/download state is available.
    Open,
    /// The upload was atomically published.
    Complete,
    /// The download was re-hashed and verified.
    Verified,
    /// Temporary guest state was explicitly removed.
    Aborted,
}

/// Upload intent and generation ownership. It never contains file bytes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct UploadTransfer {
    /// Owning workspace.
    pub workspace: WorkspaceId,
    /// Owning session.
    pub session: SessionId,
    /// Exact retained generation.
    pub generation: GenerationId,
    /// Public transfer identity.
    pub id: FileUploadId,
    /// Final workspace path.
    pub path: FilePath,
    /// Complete file length.
    pub size_bytes: u64,
    /// Complete file digest.
    pub sha256: ContentHash,
    /// Final file mode.
    pub mode: RegisteredFileMode,
    /// Provider hard stop.
    pub expires_at: Timestamp,
    /// Current durable phase.
    pub state: TransferState,
    /// Optimistic revision.
    pub revision: u64,
}

/// Download intent plus its immutable opened descriptor.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct DownloadTransfer {
    /// Owning workspace.
    pub workspace: WorkspaceId,
    /// Owning session.
    pub session: SessionId,
    /// Exact retained generation.
    pub generation: GenerationId,
    /// Public transfer identity.
    pub id: FileDownloadId,
    /// Source workspace path.
    pub path: FilePath,
    /// Provider hard stop.
    pub expires_at: Timestamp,
    /// Descriptor pinned by the guest open effect.
    pub descriptor: Option<FileDownloadState>,
    /// Current durable phase.
    pub state: TransferState,
    /// Optimistic revision.
    pub revision: u64,
}

/// Either transfer family.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "transfer", content = "record", rename_all = "snake_case")]
pub enum TransferRecord {
    /// Upload record.
    Upload(UploadTransfer),
    /// Download record.
    Download(DownloadTransfer),
}

impl TransferRecord {
    /// Physical identity.
    #[must_use]
    pub const fn key(&self) -> TransferKey {
        match self {
            Self::Upload(record) => TransferKey::Upload(record.id),
            Self::Download(record) => TransferKey::Download(record.id),
        }
    }

    /// Owning workspace.
    #[must_use]
    pub const fn workspace(&self) -> WorkspaceId {
        match self {
            Self::Upload(record) => record.workspace,
            Self::Download(record) => record.workspace,
        }
    }

    /// Owning session.
    #[must_use]
    pub const fn session(&self) -> SessionId {
        match self {
            Self::Upload(record) => record.session,
            Self::Download(record) => record.session,
        }
    }

    /// Exact retained generation.
    #[must_use]
    pub const fn generation(&self) -> GenerationId {
        match self {
            Self::Upload(record) => record.generation,
            Self::Download(record) => record.generation,
        }
    }

    /// Correctness expiry fence.
    #[must_use]
    pub const fn expires_at(&self) -> Timestamp {
        match self {
            Self::Upload(record) => record.expires_at,
            Self::Download(record) => record.expires_at,
        }
    }

    /// Optimistic revision.
    #[must_use]
    pub const fn revision(&self) -> u64 {
        match self {
            Self::Upload(record) => record.revision,
            Self::Download(record) => record.revision,
        }
    }

    /// Advances the revision after a state transition.
    pub fn advance_revision(&mut self) {
        match self {
            Self::Upload(record) => record.revision = record.revision.saturating_add(1),
            Self::Download(record) => record.revision = record.revision.saturating_add(1),
        }
    }
}

/// Durable replay election. Only digests of credential/replay identity persist.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct TransferElection {
    /// Physical election partition.
    pub key: String,
    /// Caller intent digest.
    pub intent: [u8; 32],
    /// Elected transfer.
    pub transfer: TransferKey,
    /// Exact canonical response after the guest effect settles.
    pub response: Option<Vec<u8>>,
    /// Correctness expiry fence.
    pub expires_at: Timestamp,
    /// Optimistic revision.
    pub revision: u64,
}

/// Input to one pre-effect election.
pub struct ElectRequest {
    /// Workspace boundary.
    pub workspace: WorkspaceId,
    /// Session boundary.
    pub session: SessionId,
    /// Stable route spelling.
    pub route: &'static str,
    /// Digest of the edge's route/principal scope.
    pub scope: [u8; 32],
    /// Digest of the caller replay key.
    pub key_sha256: String,
    /// Canonical request intent.
    pub intent: [u8; 32],
    /// Elected transfer id.
    pub transfer: TransferKey,
    /// Optional create record, committed atomically with the election.
    pub create: Option<TransferRecord>,
    /// Correctness expiry fence.
    pub expires_at: Timestamp,
}

/// Result of election or exact replay.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ElectedTransfer {
    /// Winning election.
    pub election: TransferElection,
    /// Transfer record, when the caller elected a create.
    pub transfer: Option<TransferRecord>,
}

/// Live-transfer store failures.
#[derive(Debug, thiserror::Error)]
pub enum LiveTransferError {
    /// No owned transfer exists.
    #[error("live transfer not found")]
    NotFound,
    /// Replay key was reused with another intent.
    #[error("live transfer replay identity was reused with another intent")]
    IdempotencyConflict,
    /// Durable store failure.
    #[error(transparent)]
    Store(#[from] StoreError),
}

/// Narrow durable capability consumed by the live-file handlers.
#[async_trait]
pub trait LiveTransferStore: Send + Sync + 'static {
    /// Elects one identity before any guest effect.
    async fn elect(&self, request: ElectRequest) -> Result<ElectedTransfer, LiveTransferError>;
    /// Loads one strongly consistent, ownership-checked record.
    async fn load(
        &self,
        workspace: WorkspaceId,
        session: SessionId,
        key: TransferKey,
    ) -> Result<Option<TransferRecord>, LiveTransferError>;
    /// Persists a non-idempotent state transition after an idempotent guest effect.
    async fn save(
        &self,
        expected_revision: u64,
        record: &TransferRecord,
    ) -> Result<(), LiveTransferError>;
    /// Atomically stores a state transition and the exact replay response.
    async fn settle(
        &self,
        expected_revision: u64,
        record: &TransferRecord,
        election: &TransferElection,
        response: &[u8],
    ) -> Result<(), LiveTransferError>;
}

/// `DynamoDB` implementation over the existing session-authority table.
#[derive(Debug, Clone)]
pub struct LiveTransferDynamoStore {
    client: Client,
    table: Arc<str>,
}

impl LiveTransferDynamoStore {
    /// Binds the authority to the session table.
    #[must_use]
    pub fn new(client: Client, table: impl Into<Arc<str>>) -> Self {
        Self {
            client,
            table: table.into(),
        }
    }

    async fn get(&self, pk: &str, sk: &str) -> Result<Option<Item>, StoreError> {
        let output = self
            .client
            .get_item()
            .table_name(self.table.as_ref())
            .key(PK, s(pk))
            .key(SK, s(sk))
            .consistent_read(true)
            .send()
            .await
            .map_err(|error| classify(&error, Idempotence::Read))?;
        Ok(output.item)
    }

    async fn read_election(&self, key: &str) -> Result<Option<TransferElection>, StoreError> {
        self.get(key, "ELECTION")
            .await?
            .map(|item| decode_election(&item, key))
            .transpose()
    }

    async fn send(&self, plan: &TransactionPlan, resolution: Resolution) -> Result<(), StoreError> {
        match plan.compile(&self.client)?.send().await {
            Ok(_) => Ok(()),
            Err(error) => {
                if let Some(service) = error.as_service_error() {
                    return Err(decode_cancellation_with_resolution(
                        service,
                        plan.participants(),
                        resolution,
                    ));
                }
                Err(classify(&error, Idempotence::Write(resolution)))
            }
        }
    }
}

#[async_trait]
impl LiveTransferStore for LiveTransferDynamoStore {
    async fn elect(&self, request: ElectRequest) -> Result<ElectedTransfer, LiveTransferError> {
        let election_key = election_key(&request);
        if let Some(existing) = self.read_election(&election_key).await? {
            return resolve_election(self, &request, existing).await;
        }
        let election = TransferElection {
            key: election_key,
            intent: request.intent,
            transfer: request.transfer,
            response: None,
            expires_at: request.expires_at,
            revision: 0,
        };
        let mut plan =
            TransactionPlan::new(format!("live-file-elect-{}", request.transfer.sort_key()));
        plan.put(
            ELECTION_PARTICIPANT,
            immutable_put(self.table.as_ref(), encode_election(&election)?),
        )?;
        if let Some(record) = &request.create {
            if record.key() != request.transfer
                || record.workspace() != request.workspace
                || record.session() != request.session
                || record.revision() != 0
            {
                return Err(StoreError::Invalid {
                    detail: "live transfer election carries an incoherent create record".to_owned(),
                }
                .into());
            }
            plan.put(
                TRANSFER_PARTICIPANT,
                immutable_put(self.table.as_ref(), encode_transfer(record)?),
            )?;
        }
        match self.send(&plan, Resolution::IdempotencyReceipt).await {
            Ok(()) => Ok(ElectedTransfer {
                election,
                transfer: request.create,
            }),
            Err(StoreError::PreconditionFailed { .. } | StoreError::CommitAmbiguous { .. }) => {
                let existing = self.read_election(&election.key).await?.ok_or(
                    StoreError::CommitAmbiguous {
                        resolve_by: Resolution::IdempotencyReceipt,
                    },
                )?;
                resolve_election(self, &request, existing).await
            }
            Err(error) => Err(error.into()),
        }
    }

    async fn load(
        &self,
        workspace: WorkspaceId,
        session: SessionId,
        key: TransferKey,
    ) -> Result<Option<TransferRecord>, LiveTransferError> {
        let pk = session_partition(session);
        let sk = key.sort_key();
        let Some(item) = self.get(&pk, &sk).await? else {
            return Ok(None);
        };
        let record = decode_transfer(&item, &pk, &sk)?;
        if record.workspace() != workspace || record.session() != session || record.key() != key {
            return Err(StoreError::Invalid {
                detail: "live transfer row ownership or requested identity disagrees".to_owned(),
            }
            .into());
        }
        Ok(Some(record))
    }

    async fn save(
        &self,
        expected_revision: u64,
        record: &TransferRecord,
    ) -> Result<(), LiveTransferError> {
        let mut plan = TransactionPlan::new(format!("live-file-save-{}", record.key().sort_key()));
        plan.put(
            TRANSFER_PARTICIPANT,
            revision_put(
                self.table.as_ref(),
                encode_transfer(record)?,
                TRANSFER_ITEM,
                expected_revision,
            ),
        )?;
        match self.send(&plan, Resolution::TargetItem).await {
            Ok(()) => Ok(()),
            Err(StoreError::PreconditionFailed { .. } | StoreError::CommitAmbiguous { .. }) => {
                let existing = self
                    .load(record.workspace(), record.session(), record.key())
                    .await?;
                if existing.as_ref() == Some(record) {
                    Ok(())
                } else {
                    Err(StoreError::CommitAmbiguous {
                        resolve_by: Resolution::TargetItem,
                    }
                    .into())
                }
            }
            Err(error) => Err(error.into()),
        }
    }

    async fn settle(
        &self,
        expected_revision: u64,
        record: &TransferRecord,
        election: &TransferElection,
        response: &[u8],
    ) -> Result<(), LiveTransferError> {
        if election.transfer != record.key() || election.response.is_some() {
            return Err(StoreError::Invalid {
                detail: "live transfer settlement does not match its open election".to_owned(),
            }
            .into());
        }
        let mut settled = election.clone();
        settled.response = Some(response.to_vec());
        settled.revision = settled.revision.saturating_add(1);
        let mut plan =
            TransactionPlan::new(format!("live-file-settle-{}", record.key().sort_key()));
        plan.put(
            TRANSFER_PARTICIPANT,
            revision_put(
                self.table.as_ref(),
                encode_transfer(record)?,
                TRANSFER_ITEM,
                expected_revision,
            ),
        )?;
        plan.put(
            ELECTION_PARTICIPANT,
            revision_put(
                self.table.as_ref(),
                encode_election(&settled)?,
                ELECTION_ITEM,
                election.revision,
            ),
        )?;
        match self.send(&plan, Resolution::IdempotencyReceipt).await {
            Ok(()) => Ok(()),
            Err(StoreError::PreconditionFailed { .. } | StoreError::CommitAmbiguous { .. }) => {
                let existing = self.read_election(&election.key).await?.ok_or(
                    StoreError::CommitAmbiguous {
                        resolve_by: Resolution::IdempotencyReceipt,
                    },
                )?;
                if existing.intent == election.intent
                    && existing.transfer == election.transfer
                    && existing.response.as_deref() == Some(response)
                {
                    Ok(())
                } else {
                    Err(StoreError::CommitAmbiguous {
                        resolve_by: Resolution::IdempotencyReceipt,
                    }
                    .into())
                }
            }
            Err(error) => Err(error.into()),
        }
    }
}

async fn resolve_election(
    store: &LiveTransferDynamoStore,
    request: &ElectRequest,
    existing: TransferElection,
) -> Result<ElectedTransfer, LiveTransferError> {
    if existing.intent != request.intent
        || (request.create.is_none() && existing.transfer != request.transfer)
    {
        return Err(LiveTransferError::IdempotencyConflict);
    }
    let transfer = if request.create.is_some() {
        store
            .load(request.workspace, request.session, existing.transfer)
            .await?
    } else {
        None
    };
    if request.create.is_some() && transfer.is_none() {
        return Err(StoreError::Invalid {
            detail: "a live-file create election exists without its atomic transfer row".to_owned(),
        }
        .into());
    }
    Ok(ElectedTransfer {
        election: existing,
        transfer,
    })
}

fn session_partition(session: SessionId) -> String {
    format!("SESSION#{session}")
}

fn election_key(request: &ElectRequest) -> String {
    format!(
        "LIVEIDEM#{}#{}#{}#{}#{}",
        request.workspace,
        request.session,
        request.route,
        hex::encode(request.scope),
        request.key_sha256
    )
}

fn encode_transfer(record: &TransferRecord) -> Result<Item, StoreError> {
    let document = serde_json::to_vec(record).map_err(|error| StoreError::Invalid {
        detail: format!("live transfer could not be encoded: {error}"),
    })?;
    Ok(ItemBuilder::new(TRANSFER_ITEM)
        .set(PK, s(session_partition(record.session())))
        .set(SK, s(record.key().sort_key()))
        .set("workspaceId", s(record.workspace().to_string()))
        .set("sessionId", s(record.session().to_string()))
        .set("generationId", s(record.generation().to_string()))
        .set("revision", n(record.revision()))
        .set("expiresAt", stamp(record.expires_at()))
        .set(
            "expiresAtEpochSeconds",
            n(expiry_epoch(record.expires_at())?),
        )
        .set("document", b(document))
        .build())
}

fn decode_transfer(
    item: &Item,
    expected_partition_key: &str,
    expected_sort_key: &str,
) -> Result<TransferRecord, StoreError> {
    let row = Row::bind(item, TRANSFER_ITEM)?;
    if row.string(PK)? != expected_partition_key || row.string(SK)? != expected_sort_key {
        return Err(StoreError::Invalid {
            detail: "live transfer row key disagrees with the requested key".to_owned(),
        });
    }
    let record: TransferRecord =
        serde_json::from_slice(row.bytes("document")?).map_err(|error| StoreError::Invalid {
            detail: format!("live transfer document is malformed: {error}"),
        })?;
    if row.string("workspaceId")? != record.workspace().to_string()
        || row.string("sessionId")? != record.session().to_string()
        || row.string("generationId")? != record.generation().to_string()
        || row.u64("revision")? != record.revision()
        || row.timestamp("expiresAt")? != record.expires_at()
        || row.u64("expiresAtEpochSeconds")? != expiry_epoch(record.expires_at())?
        || session_partition(record.session()) != expected_partition_key
        || record.key().sort_key() != expected_sort_key
    {
        return Err(StoreError::Invalid {
            detail: "live transfer indexed attributes disagree with its document".to_owned(),
        });
    }
    Ok(record)
}

fn encode_election(election: &TransferElection) -> Result<Item, StoreError> {
    let document = serde_json::to_vec(election).map_err(|error| StoreError::Invalid {
        detail: format!("live transfer election could not be encoded: {error}"),
    })?;
    Ok(ItemBuilder::new(ELECTION_ITEM)
        .set(PK, s(election.key.clone()))
        .set(SK, s("ELECTION"))
        .set("revision", n(election.revision))
        .set("expiresAt", stamp(election.expires_at))
        .set(
            "expiresAtEpochSeconds",
            n(expiry_epoch(election.expires_at)?),
        )
        .set("document", b(document))
        .build())
}

fn decode_election(item: &Item, expected_key: &str) -> Result<TransferElection, StoreError> {
    let row = Row::bind(item, ELECTION_ITEM)?;
    let election: TransferElection =
        serde_json::from_slice(row.bytes("document")?).map_err(|error| StoreError::Invalid {
            detail: format!("live transfer election is malformed: {error}"),
        })?;
    if row.string(PK)? != expected_key
        || row.string(SK)? != "ELECTION"
        || election.key != expected_key
        || row.u64("revision")? != election.revision
        || row.timestamp("expiresAt")? != election.expires_at
        || row.u64("expiresAtEpochSeconds")? != expiry_epoch(election.expires_at)?
    {
        return Err(StoreError::Invalid {
            detail: "live transfer election indexed attributes disagree with its document"
                .to_owned(),
        });
    }
    Ok(election)
}

fn expiry_epoch(expires_at: Timestamp) -> Result<u64, StoreError> {
    u64::try_from(expires_at.unix_millis().div_euclid(1_000)).map_err(|_| StoreError::Invalid {
        detail: "live transfer expiry must be at or after the Unix epoch".to_owned(),
    })
}

fn immutable_put(table: &str, item: Item) -> aws_sdk_dynamodb::types::builders::PutBuilder {
    aws_sdk_dynamodb::types::Put::builder()
        .table_name(table)
        .set_item(Some(item))
        .condition_expression("attribute_not_exists(#pk)")
        .expression_attribute_names("#pk", PK)
}

fn revision_put(
    table: &str,
    item: Item,
    item_type: &'static str,
    expected_revision: u64,
) -> aws_sdk_dynamodb::types::builders::PutBuilder {
    aws_sdk_dynamodb::types::Put::builder()
        .table_name(table)
        .set_item(Some(item))
        .condition_expression("#item_type = :item_type AND #revision = :revision")
        .expression_attribute_names("#item_type", "itemType")
        .expression_attribute_names("#revision", "revision")
        .expression_attribute_values(":item_type", s(item_type))
        .expression_attribute_values(":revision", n(expected_revision))
}

#[cfg(test)]
mod tests {
    use super::*;
    use aex_wire::ids::Uuid7;

    fn id<T: aex_wire::ids::PrefixedId>(time: u64, byte: u8) -> T {
        T::from_uuid7(Uuid7::compose(time, [byte; 10]))
    }

    fn upload() -> TransferRecord {
        TransferRecord::Upload(UploadTransfer {
            workspace: id(1, 1),
            session: id(2, 2),
            generation: id(3, 3),
            id: id(4, 4),
            path: FilePath::parse("/large.bin").expect("path"),
            size_bytes: 4_194_304,
            sha256: ContentHash::from_bytes([5; 32]),
            mode: RegisteredFileMode::V0644,
            expires_at: Timestamp::from_unix_millis(30_000).expect("time"),
            state: TransferState::Opening,
            revision: 0,
        })
    }

    #[test]
    fn transfer_rows_bind_the_requested_key_and_indexed_authority() {
        let record = upload();
        let pk = session_partition(record.session());
        let sk = record.key().sort_key();
        let item = encode_transfer(&record).expect("encode");
        assert_eq!(decode_transfer(&item, &pk, &sk).expect("decode"), record);

        let mut corrupt = item;
        corrupt.insert(
            "generationId".to_owned(),
            s(id::<GenerationId>(9, 9).to_string()),
        );
        assert!(decode_transfer(&corrupt, &pk, &sk).is_err());

        let mut corrupt_ttl = encode_transfer(&record).expect("encode");
        corrupt_ttl.insert("expiresAtEpochSeconds".to_owned(), n(29));
        assert!(decode_transfer(&corrupt_ttl, &pk, &sk).is_err());
    }

    #[test]
    fn election_rows_store_only_digests_identity_and_exact_response() {
        let record = upload();
        let election = TransferElection {
            key: "LIVEIDEM#bounded".to_owned(),
            intent: [7; 32],
            transfer: record.key(),
            response: Some(br#"{\"state\":\"staging\"}"#.to_vec()),
            expires_at: record.expires_at(),
            revision: 1,
        };
        let item = encode_election(&election).expect("encode");
        assert_eq!(
            decode_election(&item, &election.key).expect("decode"),
            election
        );
        assert!(!format!("{item:?}").contains("customer-replay-key"));
    }

    #[test]
    fn transfer_documents_never_contain_file_bytes() {
        let record = upload();
        let encoded = serde_json::to_vec(&record).expect("document");
        assert!(encoded.len() < 2_048);
        assert!(!String::from_utf8(encoded).expect("json").contains("bytes"));
    }
}
