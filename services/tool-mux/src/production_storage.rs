//! Production `storage.persist` ports: exact-generation guest download,
//! immutable content publication, and latest-current registry replacement.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use aex_brain_hands::LiveFileBackend;
use aex_content_aws::{PutImmutablePath, S3ContentObjects};
use aex_content_domain::{RegisteredName, RegistryKind};
use aex_content_dynamodb::codec::{ContentDescriptor, ContentPin, ObjectLocation};
use aex_content_dynamodb::store::{ContentMetadataStore, ContentStore};
use aex_content_dynamodb::wire_pending::PinOwner;
use aex_hands_protocol::files::{
    FILE_FRAME_BYTES, FileDownloadId, FileRequest, FileResponse, MAX_FILE_BYTES,
};
use aex_hands_protocol::operation::GuestPath;
use aex_hands_protocol::rpc::HandsOperationId;
use aex_registry_dynamodb::store::{
    RegistryDynamoStore, RegistryStore, SetCommit, SetCommitted, SetReceipt,
};
use aex_session_dynamodb::error::StoreError;
use aex_session_dynamodb::plan::Participant;
use aex_session_dynamodb::replay::{
    DecodeReceipt, IdempotencyScope, RECEIPT_RETENTION, Receipt, ReceiptStore, key_digest,
};
use aex_tool_mux::{ReadyHand, ToolCallIdentity, ToolMuxFuture};
use aex_wire::CanonicalJson;
use aex_wire::idempotency::{IdempotencyKey, IntentDigest};
use aex_wire::ids::{ContentHash, Uuid7};
use aex_wire::models::{ContentRef, RegisteredFileMode, RegisteredFileRead};
use aex_wire::types::{DecimalU128, Timestamp};
use aex_workspace_domain::registry::{
    ProposedValue, RegisteredValueRef, RegistryState, ValueDocument, set,
};
use sha2::{Digest as _, Sha256};
use tokio::io::AsyncWriteExt as _;

use crate::storage::{
    GuestFileStreamPort, LatestFileAuthorityPort, PersistAuthorityError, PersistIntent,
    PersistLatest, PersistedLatest, VerifiedGuestFile,
};

const PREVIEW_BYTES: usize = 4 * 1024;
const REGISTRY_ENTRY_CAP: u64 = 1_000;

/// Exact-generation guest-to-local staging adapter.
pub struct ProductionGuestFileStream {
    guest: Arc<dyn LiveFileBackend>,
    staging_root: PathBuf,
}

impl ProductionGuestFileStream {
    /// Binds the authenticated guest and a task-private staging directory.
    #[must_use]
    pub fn new(guest: Arc<dyn LiveFileBackend>, staging_root: PathBuf) -> Self {
        Self {
            guest,
            staging_root,
        }
    }

    async fn call_one(
        &self,
        ready: ReadyHand,
        call: &ToolCallIdentity,
        ordinal: u32,
        request: FileRequest,
    ) -> Result<FileResponse, String> {
        let operation = operation(call, ordinal);
        let reply = self
            .guest
            .call(
                ready.hand.session(),
                ready.generation,
                operation,
                &[request],
            )
            .await
            .map_err(|_| "storage.persist guest transfer failed".to_owned())?;
        if reply.generation != ready.generation || reply.lifecycle_fence != ready.fence.0 {
            return Err("storage.persist guest answered from a stale generation".to_owned());
        }
        let [response] = reply.responses.as_slice() else {
            return Err("storage.persist guest returned a malformed transfer answer".to_owned());
        };
        Ok(response.clone())
    }
}

impl GuestFileStreamPort for ProductionGuestFileStream {
    fn stream_verified<'a>(
        &'a self,
        ready: ReadyHand,
        source: &'a GuestPath,
        call: &'a ToolCallIdentity,
    ) -> ToolMuxFuture<'a, Result<VerifiedGuestFile, String>> {
        Box::pin(async move {
            tokio::fs::create_dir_all(&self.staging_root)
                .await
                .map_err(|_| "storage.persist could not create private staging".to_owned())?;
            let download = FileDownloadId(operation_uuid(call, 0));
            let opened = self
                .call_one(
                    ready,
                    call,
                    0,
                    FileRequest::DownloadOpen {
                        download,
                        path: source.clone(),
                        range: None,
                    },
                )
                .await?;
            let FileResponse::DownloadOpened { state } = opened else {
                return Err("storage.persist guest refused to open a regular file".to_owned());
            };
            if state.download != download
                || state.start != 0
                || state.length_bytes != state.file_size_bytes
                || state.file_size_bytes > MAX_FILE_BYTES
            {
                return Err("storage.persist guest returned invalid download evidence".to_owned());
            }
            let path = self.staging_root.join(format!(
                "{}.stage",
                hex::encode(operation_uuid(call, 1).as_bytes())
            ));
            let mut file = tokio::fs::OpenOptions::new()
                .create(true)
                .truncate(true)
                .write(true)
                .open(&path)
                .await
                .map_err(|_| "storage.persist could not open private staging".to_owned())?;
            let transfer = async {
                let mut offset = 0_u64;
                let mut ordinal = 1_u32;
                let mut hasher = Sha256::new();
                let mut preview = Vec::with_capacity(PREVIEW_BYTES);
                loop {
                    let response = self
                        .call_one(
                            ready,
                            call,
                            ordinal,
                            FileRequest::DownloadChunk {
                                download,
                                offset,
                                max_bytes: FILE_FRAME_BYTES,
                            },
                        )
                        .await?;
                    ordinal = ordinal.saturating_add(1);
                    let FileResponse::DownloadChunk { chunk } = response else {
                        return Err("storage.persist guest returned a malformed chunk".to_owned());
                    };
                    if chunk.download != download
                        || chunk.offset != offset
                        || chunk.bytes.len() > FILE_FRAME_BYTES as usize
                        || ContentHash::of(&chunk.bytes) != chunk.sha256
                        || (chunk.bytes.is_empty() && !chunk.last)
                    {
                        return Err(
                            "storage.persist guest returned invalid chunk evidence".to_owned()
                        );
                    }
                    let next = offset.saturating_add(chunk.bytes.len() as u64);
                    if next > state.length_bytes {
                        return Err(
                            "storage.persist guest exceeded the opened descriptor".to_owned()
                        );
                    }
                    if preview.len() < PREVIEW_BYTES {
                        let take = (PREVIEW_BYTES - preview.len()).min(chunk.bytes.len());
                        preview.extend_from_slice(&chunk.bytes[..take]);
                    }
                    file.write_all(&chunk.bytes)
                        .await
                        .map_err(|_| "storage.persist private staging write failed".to_owned())?;
                    hasher.update(&chunk.bytes);
                    offset = next;
                    if chunk.last {
                        break;
                    }
                }
                if offset != state.length_bytes {
                    return Err("storage.persist guest ended before the opened length".to_owned());
                }
                file.flush()
                    .await
                    .map_err(|_| "storage.persist private staging flush failed".to_owned())?;
                file.sync_all()
                    .await
                    .map_err(|_| "storage.persist private staging sync failed".to_owned())?;
                let closed = self
                    .call_one(
                        ready,
                        call,
                        ordinal,
                        FileRequest::DownloadClose { download },
                    )
                    .await?;
                let FileResponse::DownloadClosed {
                    download: closed_id,
                    sha256,
                    version,
                } = closed
                else {
                    return Err("storage.persist guest did not close its descriptor".to_owned());
                };
                let computed = ContentHash::from_bytes(hasher.finalize().into());
                if closed_id != download
                    || sha256 != state.sha256
                    || version != state.version
                    || computed != state.sha256
                {
                    return Err("storage.persist whole-file integrity check failed".to_owned());
                }
                Ok(VerifiedGuestFile {
                    ready,
                    source: source.clone(),
                    staging_ref: path.to_string_lossy().into_owned(),
                    hash: computed,
                    bytes: offset,
                    preview,
                })
            }
            .await;
            if transfer.is_err() {
                let _ = self
                    .call_one(
                        ready,
                        call,
                        u32::MAX,
                        FileRequest::DownloadAbort { download },
                    )
                    .await;
                let _ = tokio::fs::remove_file(&path).await;
            }
            transfer
        })
    }
}

/// Latest-current file authority backed by the existing content and registry stores.
pub struct ProductionLatestFileAuthority {
    registry: RegistryDynamoStore,
    content: ContentStore,
    objects: S3ContentObjects,
    content_kms_key_id: String,
    staging_root: PathBuf,
}

impl ProductionLatestFileAuthority {
    /// Binds the three existing regional authorities.
    #[must_use]
    pub fn new(
        registry: RegistryDynamoStore,
        content: ContentStore,
        objects: S3ContentObjects,
        content_kms_key_id: String,
        staging_root: PathBuf,
    ) -> Self {
        Self {
            registry,
            content,
            objects,
            content_kms_key_id,
            staging_root,
        }
    }

    async fn replay(
        &self,
        intent: &PersistIntent,
    ) -> Result<Option<PersistedLatest>, PersistAuthorityError> {
        let key = replay_key(intent)?;
        let scope = scope()?;
        let Some(receipt) = self
            .registry
            .read_receipt(intent.workspace, &scope, &key, now()?)
            .await
            .map_err(store_failed)?
        else {
            return Ok(None);
        };
        if receipt.intent != intent_digest(intent) {
            return Err(PersistAuthorityError::IdempotencyConflict);
        }
        let stored = SetReceipt::decode_receipt(&receipt).map_err(store_failed)?;
        persisted_from_pointer(stored.pointer(intent.workspace), true, Vec::new()).map(Some)
    }
}

impl LatestFileAuthorityPort for ProductionLatestFileAuthority {
    fn load_replay<'a>(
        &'a self,
        intent: &'a PersistIntent,
    ) -> ToolMuxFuture<'a, Result<Option<PersistedLatest>, PersistAuthorityError>> {
        Box::pin(self.replay(intent))
    }

    fn persist_latest<'a>(
        &'a self,
        command: PersistLatest,
    ) -> ToolMuxFuture<'a, Result<PersistedLatest, PersistAuthorityError>> {
        Box::pin(async move {
            if let Some(replay) = self.replay(&command.intent).await? {
                cleanup_staging(&self.staging_root, &command.staging_ref).await;
                return Ok(replay);
            }
            let path = checked_staging_path(&self.staging_root, &command.staging_ref)?;
            let context = format!(
                r#"{{"aex:domain":"content-object","aex:workspace":"{}"}}"#,
                command.intent.workspace
            );
            let object = self
                .objects
                .put_immutable_path(PutImmutablePath {
                    workspace: command.intent.workspace,
                    digest: &command.hash,
                    plaintext_bytes: command.bytes,
                    path: &path,
                    encryption_context: context.as_bytes(),
                })
                .await
                .map_err(|error| PersistAuthorityError::Failed(error.to_string()))?;
            let now = now()?;
            self.content
                .admit_body(
                    &ContentDescriptor {
                        workspace: command.intent.workspace,
                        organization: command.intent.call.organization,
                        digest: command.hash,
                        size_bytes: command.bytes,
                        media_type: command.intent.media_type.clone(),
                        placement: aex_session_dynamodb::measure::Placement::ObjectStore,
                        state: "committed".to_owned(),
                        gc_epoch: 0,
                        created_at: now,
                        verified_at: Some(now),
                        object: Some(ObjectLocation {
                            key: object.key.as_str().to_owned(),
                            etag: object.etag,
                            checksum_sha256: object.checksum_sha256.unwrap_or_default(),
                            checksum_crc64_nvme: object.checksum_crc64_nvme.unwrap_or_default(),
                            part_count: object.part_count,
                            kms_key_id: self.content_kms_key_id.clone(),
                        }),
                    },
                    &ContentPin {
                        workspace: command.intent.workspace,
                        owner: PinOwner::Registry {
                            kind: "file".to_owned(),
                            name: command.intent.name.as_str().to_owned(),
                        },
                        created_at: now,
                    },
                    now,
                )
                .await
                .map_err(store_failed)?;
            let current = self
                .registry
                .load_pointer(
                    command.intent.workspace,
                    RegistryKind::File,
                    command.intent.name.as_str(),
                )
                .await
                .map_err(store_failed)?;
            let value = RegisteredFileRead {
                content: ContentRef {
                    sha256: command.hash,
                    size_bytes: DecimalU128::new(u128::from(command.bytes)),
                },
                media_type: command.intent.media_type.clone(),
                mode: RegisteredFileMode::V0644,
            };
            let canonical = CanonicalJson::from_value(
                &serde_json::to_value(value)
                    .map_err(|error| PersistAuthorityError::Failed(error.to_string()))?,
            )
            .map_err(|error| PersistAuthorityError::Failed(error.to_string()))?;
            let proposal = ProposedValue {
                workspace: command.intent.workspace,
                kind: RegistryKind::File,
                name: RegisteredName::parse(command.intent.name.as_str())
                    .map_err(|error| PersistAuthorityError::Failed(error.to_string()))?,
                value_doc: ValueDocument::new(canonical),
                payload: Some(RegisteredValueRef::Content {
                    digest: command.hash,
                }),
                upload_state: None,
                state: RegistryState::Ready,
                failure_code: None,
            };
            let (outcome, commit) = set(
                current.as_ref(),
                &proposal,
                current.as_ref().map(|pointer| &pointer.row.etag),
                now,
            )
            .map_err(|_| PersistAuthorityError::StaleIntent)?;
            let key = replay_key(&command.intent)?;
            let receipt = Receipt {
                scope: scope()?.render(),
                key_sha256: key_digest(&key),
                intent: intent_digest(&command.intent),
                response_kind: aex_registry_dynamodb::store::SET_RESPONSE_KIND.to_owned(),
                response: SetReceipt::of(outcome, &commit.pointer)
                    .to_body()
                    .map_err(store_failed)?,
                committed_at: now,
                expires_at: Timestamp::from_unix_millis(now.unix_millis().saturating_add(
                    i64::try_from(RECEIPT_RETENTION.as_millis()).unwrap_or(i64::MAX),
                ))
                .map_err(|error| PersistAuthorityError::Failed(error.to_string()))?,
            };
            let committed = self
                .registry
                .commit_set(
                    command.intent.workspace,
                    SetCommit {
                        commit: &commit,
                        from_revision: current.as_ref().map(|pointer| pointer.row.revision),
                        entries_cap: REGISTRY_ENTRY_CAP,
                        creates: current.is_none(),
                        receipt: &receipt,
                    },
                )
                .await;
            let pointer = match committed {
                Ok(SetCommitted::Committed) => commit.pointer,
                Ok(SetCommitted::Replayed(receipt)) => receipt.pointer(command.intent.workspace),
                Err(StoreError::IdempotencyConflict) => {
                    return Err(PersistAuthorityError::IdempotencyConflict);
                }
                Err(StoreError::PreconditionFailed { participant, .. })
                    if participant == Participant::REGISTRY_POINTER =>
                {
                    return Err(PersistAuthorityError::StaleIntent);
                }
                Err(error) => return Err(store_failed(error)),
            };
            cleanup_staging(&self.staging_root, &command.staging_ref).await;
            persisted_from_pointer(pointer, false, command.preview)
        })
    }
}

fn persisted_from_pointer(
    pointer: aex_workspace_domain::registry::RegistryPointer,
    replayed: bool,
    preview: Vec<u8>,
) -> Result<PersistedLatest, PersistAuthorityError> {
    let value: RegisteredFileRead = serde_json::from_str(pointer.value_doc.as_str())
        .map_err(|error| PersistAuthorityError::Failed(error.to_string()))?;
    let bytes = u64::try_from(value.content.size_bytes.get())
        .map_err(|error| PersistAuthorityError::Failed(error.to_string()))?;
    let name = pointer.row.name;
    let download_path = format!("/api/workspace/files/{}/download", name.as_str());
    Ok(PersistedLatest {
        workspace: pointer.row.workspace,
        name,
        hash: value.content.sha256,
        bytes,
        media_type: value.media_type,
        download_path: Some(download_path),
        replayed,
        preview,
    })
}

fn scope() -> Result<IdempotencyScope<'static>, PersistAuthorityError> {
    IdempotencyScope::new("registry.set", Some("file"))
        .map_err(|error| PersistAuthorityError::Failed(error.to_string()))
}

fn replay_key(intent: &PersistIntent) -> Result<IdempotencyKey, PersistAuthorityError> {
    IdempotencyKey::parse(&format!("tool-mux:{}", intent.call.call))
        .map_err(|error| PersistAuthorityError::Failed(error.to_string()))
}

fn intent_digest(intent: &PersistIntent) -> IntentDigest {
    let bytes = serde_json::to_vec(&serde_json::json!({
        "workspace": intent.workspace.to_string(),
        "session": intent.call.session.to_string(),
        "agent": intent.call.agent.to_string(),
        "generation": intent.ready.generation.to_string(),
        "fence": intent.ready.fence.0,
        "source": intent.source.as_str(),
        "name": intent.name.as_str(),
        "mediaType": intent.media_type,
    }))
    .unwrap_or_else(|_| unreachable!("scalar persistence intent is encodable"));
    IntentDigest::from_bytes(Sha256::digest(bytes).into())
}

fn operation(call: &ToolCallIdentity, ordinal: u32) -> HandsOperationId {
    HandsOperationId(operation_uuid(call, ordinal))
}

fn operation_uuid(call: &ToolCallIdentity, ordinal: u32) -> Uuid7 {
    let digest = Sha256::digest(format!("{}:{ordinal}", call.call));
    let mut bytes = [0_u8; 16];
    bytes.copy_from_slice(&digest[..16]);
    bytes[6] = 0x70 | (bytes[6] & 0x0f);
    bytes[8] = 0x80 | (bytes[8] & 0x3f);
    Uuid7::from_bytes(bytes).unwrap_or_else(|_| unreachable!("version bits were normalized"))
}

fn checked_staging_path(root: &Path, value: &str) -> Result<PathBuf, PersistAuthorityError> {
    let path = PathBuf::from(value);
    if path.parent() != Some(root)
        || path.extension().and_then(|value| value.to_str()) != Some("stage")
    {
        return Err(PersistAuthorityError::Failed(
            "storage.persist staging identity escaped its private root".to_owned(),
        ));
    }
    Ok(path)
}

async fn cleanup_staging(root: &Path, value: &str) {
    if let Ok(path) = checked_staging_path(root, value) {
        let _ = tokio::fs::remove_file(path).await;
    }
}

fn now() -> Result<Timestamp, PersistAuthorityError> {
    Timestamp::from_datetime_trunc_ms(time::OffsetDateTime::now_utc())
        .map_err(|error| PersistAuthorityError::Failed(error.to_string()))
}

fn store_failed(error: StoreError) -> PersistAuthorityError {
    PersistAuthorityError::Failed(error.to_string())
}
