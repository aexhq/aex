//! Exact-generation ephemeral session file routes.

use aex_hands_protocol::files::{
    FILE_FRAME_BYTES, FILE_TRANSFER_PART_BYTES, FileDownloadId as GuestDownloadId,
    FileDownloadState, FilePartReceipt, FileRequest, FileResponse, FileUploadId as GuestUploadId,
    FileUploadState, LiveFileEntryKind, MAX_FILE_BYTES,
};
use aex_hands_protocol::operation::{FileMode, GuestPath, GuestRoot};
use aex_hands_protocol::rpc::HandsOperationId;
use aex_regional_http::cursor::{
    CursorBinding, CursorRequestBinding, Order, SnapshotToken, decode_state_resume, encode_state,
};
use aex_regional_http::projection::{ProjectionError, authority_failure};
use aex_runtime_control::store::{GenerationPointer, RuntimeActivityStore as _, RuntimeStoreError};
use aex_session_dynamodb::store::SessionScoped;
use aex_wire::error::{ErrorCode, WireError, WireResult};
use aex_wire::ids::{
    ContentHash, FileDownloadId, FilePath, FileUploadId, GenerationId, PrefixedId as _, SessionId,
    Uuid7,
};
use aex_wire::models;
use aex_wire::routes::RouteId;
use aex_wire::server::{Created, FilesApi, NoContent, RequestContext as WireContext};
use aex_wire::types::{DecimalU128, Timestamp};
use serde::{Deserialize, Serialize};

use super::handlers::Routes;
use super::live_transfer::{
    DownloadTransfer, ElectRequest, LiveTransferError, TransferElection, TransferKey,
    TransferRecord, TransferState, UploadTransfer,
};

const DEFAULT_LIST_LIMIT: u32 = 100;
const TRANSFER_UPPER_LIFETIME_MS: i64 = 8 * 60 * 60 * 1_000;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct LiveListCursor {
    after: String,
    generation_id: GenerationId,
    lifecycle_fence: u64,
}

impl Routes {
    async fn live_generation(
        &self,
        session_id: SessionId,
        required: Option<GenerationId>,
    ) -> WireResult<GenerationPointer> {
        let session = match self
            .shared()
            .sessions
            .load_session(self.context().auth.workspace_id, session_id)
            .await
            .map_err(|error| authority_failure(&error))?
        {
            SessionScoped::Missing => return Err(WireError::new(ErrorCode::NotFound)),
            SessionScoped::Deleted => return Err(WireError::new(ErrorCode::SessionDeleted)),
            SessionScoped::Active(session) => session,
        };
        let generation =
            generation_for_live_status(session.lifecycle.status, session.lifecycle.generation)?;
        if required.is_some_and(|required| required != generation) {
            return Err(WireError::new(ErrorCode::PreconditionFailed));
        }
        let pointer = self
            .shared()
            .runtime_activity
            .load_current_generation(session_id)
            .await
            .map_err(|error| runtime_authority_failure(&error))?
            .ok_or_else(|| WireError::new(ErrorCode::ResourceConflict))?;
        if pointer.session != session_id || pointer.generation != generation {
            return Err(WireError::new(ErrorCode::PreconditionFailed));
        }
        Ok(pointer)
    }

    async fn live_call(
        &self,
        session_id: SessionId,
        generation: GenerationId,
        requests: &[FileRequest],
    ) -> WireResult<aex_brain_hands::LiveFileReply> {
        let activity = HandsOperationId(
            Uuid7::from_bytes(*uuid::Uuid::now_v7().as_bytes())
                .unwrap_or_else(|_| unreachable!("`now_v7` always produces UUIDv7")),
        );
        self.shared()
            .live_files
            .call(session_id, generation, activity, requests)
            .await
            .map_err(|error| {
                eprintln!(
                    "session-stream-api: live-file call for session {session_id} failed: {error}"
                );
                WireError::new(ErrorCode::UpstreamError)
            })
    }

    fn new_uuid7() -> Uuid7 {
        Uuid7::from_bytes(*uuid::Uuid::now_v7().as_bytes())
            .unwrap_or_else(|_| unreachable!("`now_v7` always produces UUIDv7"))
    }

    fn replay(&self) -> WireResult<&aex_regional_http::idempotency::IdempotencyIdentity> {
        self.context().idempotency.as_ref().ok_or_else(|| {
            WireError::new(ErrorCode::InvalidRequest).with_message("idempotency key")
        })
    }

    fn transfer_upper_expiry(&self) -> WireResult<Timestamp> {
        Timestamp::from_unix_millis(
            self.now()?
                .unix_millis()
                .saturating_add(TRANSFER_UPPER_LIFETIME_MS),
        )
        .map_err(|_| WireError::new(ErrorCode::InternalError))
    }

    async fn elect(
        &self,
        session: SessionId,
        route: RouteId,
        transfer: TransferKey,
        create: Option<TransferRecord>,
        expires_at: Timestamp,
    ) -> WireResult<(TransferElection, Option<TransferRecord>)> {
        let replay = self.replay()?;
        let elected = self
            .shared()
            .live_transfers
            .elect(ElectRequest {
                workspace: self.context().auth.workspace_id,
                session,
                route: route.as_str(),
                scope: replay.scope,
                key_sha256: aex_wire::idempotency::key_digest(replay.key.as_str()),
                intent: replay.intent,
                transfer,
                create,
                expires_at,
            })
            .await
            .map_err(transfer_failure)?;
        Ok((elected.election, elected.transfer))
    }

    async fn owned_transfer(
        &self,
        session: SessionId,
        key: TransferKey,
    ) -> WireResult<TransferRecord> {
        let record = self
            .shared()
            .live_transfers
            .load(self.context().auth.workspace_id, session, key)
            .await
            .map_err(transfer_failure)?
            .ok_or_else(|| WireError::new(ErrorCode::NotFound))?;
        if self.now()?.unix_millis() >= record.expires_at().unix_millis() {
            return Err(WireError::new(ErrorCode::NotFound));
        }
        Ok(record)
    }

    fn live_list_query_hash(path: &FilePath, recursive: bool) -> WireResult<[u8; 32]> {
        use sha2::Digest as _;

        #[derive(Serialize)]
        #[serde(rename_all = "camelCase")]
        struct Query<'a> {
            path: &'a str,
            recursive: bool,
        }
        let canonical = aex_wire::canonical::to_jcs_bytes(&Query {
            path: path.as_str(),
            recursive,
        })
        .map_err(|_| WireError::new(ErrorCode::InternalError))?;
        let mut digest = sha2::Sha256::new();
        digest.update(b"aex.session.live-files.list.v1\0");
        digest.update(canonical);
        Ok(digest.finalize().into())
    }

    fn live_list_request_binding(
        &self,
        session_id: SessionId,
        query_hash: [u8; 32],
    ) -> CursorRequestBinding {
        CursorRequestBinding {
            route: RouteId::SessionFilesLiveList,
            principal_scope: self.context().auth.credential_binding,
            region: self.context().auth.placement,
            workspace_id: self.context().auth.workspace_id,
            session_id: Some(session_id),
            query_hash,
            order: Order::Ascending,
        }
    }

    fn live_list_binding(
        &self,
        session_id: SessionId,
        query_hash: [u8; 32],
        generation: GenerationId,
        lifecycle_fence: u64,
    ) -> WireResult<CursorBinding> {
        Ok(CursorBinding {
            route: RouteId::SessionFilesLiveList,
            principal_scope: self.context().auth.credential_binding,
            region: self.context().auth.placement,
            workspace_id: self.context().auth.workspace_id,
            session_id: Some(session_id),
            query_hash,
            order: Order::Ascending,
            snapshot: SnapshotToken::new(format!(
                "session.live-files:{generation}:fence:{lifecycle_fence}"
            ))
            .map_err(|error| WireError::from(ProjectionError::Cursor(error)))?,
        })
    }
}

fn runtime_authority_failure(error: &RuntimeStoreError) -> WireError {
    let code = match error {
        RuntimeStoreError::RevisionConflict { .. }
        | RuntimeStoreError::ReconcileConflict { .. } => ErrorCode::PreconditionFailed,
        RuntimeStoreError::IntentOpen { .. } => ErrorCode::ResourceConflict,
        RuntimeStoreError::NoSuchGeneration { .. } => ErrorCode::NotFound,
        RuntimeStoreError::Unavailable { .. } => ErrorCode::UpstreamError,
        RuntimeStoreError::Malformed { .. } => ErrorCode::InternalError,
    };
    WireError::new(code)
}

fn generation_for_live_status(
    status: aex_session_domain::lifecycle::LifecycleStatus,
    generation: GenerationId,
) -> WireResult<GenerationId> {
    use aex_session_domain::lifecycle::LifecycleStatus;
    match status {
        LifecycleStatus::Terminating | LifecycleStatus::Terminated => {
            Err(WireError::new(ErrorCode::SessionTerminated))
        }
        LifecycleStatus::Deleting => Err(WireError::new(ErrorCode::SessionDeleting)),
        LifecycleStatus::Idle
        | LifecycleStatus::Running
        | LifecycleStatus::Suspending
        | LifecycleStatus::Suspended
        | LifecycleStatus::Resuming => Ok(generation),
    }
}

fn guest_path(path: &FilePath) -> WireResult<GuestPath> {
    let root = GuestRoot::workspace();
    let absolute = if path.as_str() == "/" {
        root.0.clone()
    } else {
        format!("{}{}", root.0, path.as_str())
    };
    GuestPath::parse(&root, &absolute)
        .map_err(|_| WireError::new(ErrorCode::InvalidRequest).with_message("file path"))
}

fn public_entry(entry: aex_hands_protocol::files::LiveFileEntry) -> WireResult<models::FileEntry> {
    let relative = entry
        .path
        .as_str()
        .strip_prefix("/workspace")
        .ok_or_else(|| WireError::new(ErrorCode::UpstreamError))?;
    let path = FilePath::parse(if relative.is_empty() { "/" } else { relative })
        .map_err(|_| WireError::new(ErrorCode::UpstreamError))?;
    let type_ = match entry.kind {
        LiveFileEntryKind::File => models::FileEntryType::File,
        LiveFileEntryKind::Directory => models::FileEntryType::Directory,
        LiveFileEntryKind::Symlink => models::FileEntryType::Symlink,
    };
    let target = if type_ == models::FileEntryType::Symlink {
        Some(
            entry
                .target
                .ok_or_else(|| WireError::new(ErrorCode::UpstreamError))?,
        )
    } else if entry.target.is_none() {
        None
    } else {
        return Err(WireError::new(ErrorCode::UpstreamError));
    };
    Ok(models::FileEntry {
        mode: format!("0{:03o}", entry.mode & 0o777),
        mtime: Timestamp::from_unix_millis(entry.mtime_ms)
            .map_err(|_| WireError::new(ErrorCode::UpstreamError))?,
        path,
        sha256: None,
        size_bytes: DecimalU128::new(u128::from(entry.size_bytes)),
        target,
        type_,
    })
}

fn workspace_access(reply: &aex_brain_hands::LiveFileReply) -> models::WorkspaceAccess {
    models::WorkspaceAccess {
        generation_id: reply.generation,
        resumed: reply.resumed,
    }
}

fn one_response(reply: &aex_brain_hands::LiveFileReply) -> WireResult<&FileResponse> {
    let [response] = reply.responses.as_slice() else {
        return Err(WireError::new(ErrorCode::UpstreamError));
    };
    Ok(response)
}

fn guest_rejection(code: aex_hands_protocol::files::FileFailureCode) -> WireError {
    use aex_hands_protocol::files::FileFailureCode as Guest;
    match code {
        Guest::NotFound => WireError::new(ErrorCode::FileNotFound),
        Guest::InvalidPath | Guest::InvalidRequest => WireError::new(ErrorCode::InvalidRequest),
        Guest::LimitExceeded => WireError::new(ErrorCode::LimitExceeded),
        Guest::Conflict => WireError::new(ErrorCode::ResourceConflict),
        Guest::Unavailable => WireError::new(ErrorCode::UpstreamError),
    }
}

fn transfer_failure(error: LiveTransferError) -> WireError {
    match error {
        LiveTransferError::NotFound => WireError::new(ErrorCode::NotFound),
        LiveTransferError::IdempotencyConflict => WireError::new(ErrorCode::IdempotencyConflict),
        LiveTransferError::Store(aex_session_dynamodb::error::StoreError::CommitAmbiguous {
            ..
        }) => WireError::new(ErrorCode::CommitOutcomeUnknown),
        LiveTransferError::Store(error) => authority_failure(&error),
    }
}

fn upload_mode(mode: models::RegisteredFileMode) -> FileMode {
    match mode {
        models::RegisteredFileMode::V0644 => FileMode::ReadWrite,
        models::RegisteredFileMode::V0755 => FileMode::Executable,
    }
}

fn part_count(size: u64) -> u32 {
    if size == 0 {
        0
    } else {
        u32::try_from(size.div_ceil(u64::from(FILE_TRANSFER_PART_BYTES))).unwrap_or(u32::MAX)
    }
}

fn part_shape(size: u64, number: u32) -> WireResult<(u64, u32)> {
    let count = part_count(size);
    if number == 0 || number > count {
        return Err(WireError::new(ErrorCode::InvalidRequest).with_message("part number"));
    }
    let offset = u64::from(number - 1).saturating_mul(u64::from(FILE_TRANSFER_PART_BYTES));
    let length = size
        .saturating_sub(offset)
        .min(u64::from(FILE_TRANSFER_PART_BYTES));
    Ok((
        offset,
        u32::try_from(length).map_err(|_| WireError::new(ErrorCode::InternalError))?,
    ))
}

fn public_parts(parts: &[FilePartReceipt]) -> Vec<models::LiveFileUploadPart> {
    parts
        .iter()
        .map(|part| models::LiveFileUploadPart {
            offset: DecimalU128::new(u128::from(part.offset)),
            part_number: part.part_number,
            sha256: part.sha256,
            size_bytes: part.size_bytes,
        })
        .collect()
}

fn require_upload_state(record: &UploadTransfer, state: &FileUploadState) -> WireResult<()> {
    if state.upload != GuestUploadId(record.id.uuid7())
        || state.path != guest_path(&record.path)?
        || state.size_bytes != record.size_bytes
        || state.sha256 != record.sha256
        || state.parts.len() > usize::try_from(part_count(record.size_bytes)).unwrap_or(usize::MAX)
    {
        return Err(WireError::new(ErrorCode::UpstreamError));
    }
    let mut previous_number = 0_u32;
    for part in &state.parts {
        let (offset, length) = part_shape(record.size_bytes, part.part_number)?;
        if part.part_number <= previous_number || part.offset != offset || part.size_bytes != length
        {
            return Err(WireError::new(ErrorCode::UpstreamError));
        }
        previous_number = part.part_number;
    }
    Ok(())
}

fn public_upload(
    record: &UploadTransfer,
    state: &FileUploadState,
    access: models::WorkspaceAccess,
) -> WireResult<models::LiveFileUpload> {
    require_upload_state(record, state)?;
    if state.complete != matches!(record.state, TransferState::Complete) {
        return Err(WireError::new(ErrorCode::UpstreamError));
    }
    Ok(models::LiveFileUpload {
        expires_at: record.expires_at,
        generation_id: record.generation,
        id: record.id,
        mode: record.mode,
        part_count: part_count(record.size_bytes),
        part_size_bytes: FILE_TRANSFER_PART_BYTES,
        parts: public_parts(&state.parts),
        path: record.path.clone(),
        session_id: record.session,
        sha256: record.sha256,
        size_bytes: DecimalU128::new(u128::from(record.size_bytes)),
        state: if state.complete {
            models::LiveFileUploadState::Complete
        } else {
            models::LiveFileUploadState::Staging
        },
        workspace_access: access,
    })
}

fn require_download_state(record: &DownloadTransfer, state: &FileDownloadState) -> WireResult<()> {
    if state.download != GuestDownloadId(record.id.uuid7())
        || state.start != 0
        || state.length_bytes != state.file_size_bytes
        || state.file_size_bytes > MAX_FILE_BYTES
        || state.parts.len()
            != usize::try_from(part_count(state.file_size_bytes)).unwrap_or(usize::MAX)
    {
        return Err(WireError::new(ErrorCode::UpstreamError));
    }
    for (index, part) in state.parts.iter().enumerate() {
        let number = u32::try_from(index + 1).unwrap_or(u32::MAX);
        let (offset, length) = part_shape(state.file_size_bytes, number)?;
        if part.part_number != number || part.offset != offset || part.size_bytes != length {
            return Err(WireError::new(ErrorCode::UpstreamError));
        }
    }
    Ok(())
}

fn public_download(
    record: &DownloadTransfer,
    access: models::WorkspaceAccess,
) -> WireResult<models::LiveFileDownload> {
    let state = record
        .descriptor
        .as_ref()
        .ok_or_else(|| WireError::new(ErrorCode::UpstreamError))?;
    require_download_state(record, state)?;
    Ok(models::LiveFileDownload {
        expires_at: record.expires_at,
        generation_id: record.generation,
        id: record.id,
        part_count: part_count(state.file_size_bytes),
        part_size_bytes: FILE_TRANSFER_PART_BYTES,
        parts: state
            .parts
            .iter()
            .map(|part| models::LiveFileDownloadPart {
                offset: DecimalU128::new(u128::from(part.offset)),
                part_number: part.part_number,
                sha256: part.sha256,
                size_bytes: part.size_bytes,
            })
            .collect(),
        path: record.path.clone(),
        session_id: record.session,
        sha256: state.sha256,
        size_bytes: DecimalU128::new(u128::from(state.file_size_bytes)),
        state: if matches!(record.state, TransferState::Verified) {
            models::LiveFileDownloadState::Verified
        } else {
            models::LiveFileDownloadState::Open
        },
        version: state.version,
        workspace_access: access,
    })
}

fn replay_response<T: serde::de::DeserializeOwned>(
    election: &TransferElection,
) -> WireResult<Option<T>> {
    election
        .response
        .as_ref()
        .map(|bytes| {
            serde_json::from_slice(bytes).map_err(|_| WireError::new(ErrorCode::InternalError))
        })
        .transpose()
}

impl FilesApi for Routes {
    async fn session_files_live_list(
        &self,
        _cx: &WireContext,
        session_id: SessionId,
        body: models::LiveFileListRequest,
    ) -> WireResult<models::LiveFileEntryPage> {
        let path = body
            .path
            .unwrap_or(FilePath::parse("/").map_err(|_| WireError::new(ErrorCode::InternalError))?);
        let recursive = body.recursive.unwrap_or(false);
        let query_hash = Self::live_list_query_hash(&path, recursive)?;
        let pointer = self
            .live_generation(session_id, body.if_generation_id)
            .await?;
        let after = if let Some(cursor) = body.cursor.as_ref() {
            let resumed = decode_state_resume::<LiveListCursor>(
                &self.shared().cursor_keys,
                cursor,
                &self.live_list_request_binding(session_id, query_hash),
                self.now()?,
            )
            .map_err(|error| WireError::from(ProjectionError::Cursor(error)))?;
            if resumed.state.generation_id != pointer.generation
                || resumed.state.lifecycle_fence != pointer.fence.0
                || resumed.snapshot.as_str()
                    != self
                        .live_list_binding(
                            session_id,
                            query_hash,
                            pointer.generation,
                            pointer.fence.0,
                        )?
                        .snapshot
                        .as_str()
            {
                return Err(WireError::new(ErrorCode::InvalidCursor));
            }
            Some(
                GuestPath::parse(&GuestRoot::workspace(), &resumed.state.after)
                    .map_err(|_| WireError::new(ErrorCode::InvalidCursor))?,
            )
        } else {
            None
        };
        let reply = self
            .live_call(
                session_id,
                pointer.generation,
                &[FileRequest::List {
                    path: guest_path(&path)?,
                    recursive,
                    limit: body.limit.unwrap_or(DEFAULT_LIST_LIMIT),
                    after,
                }],
            )
            .await?;
        let FileResponse::Listing { listing } = one_response(&reply)? else {
            if let FileResponse::Rejected { code } = one_response(&reply)? {
                return Err(match code {
                    aex_hands_protocol::files::FileFailureCode::NotFound => {
                        WireError::new(ErrorCode::NotFound)
                    }
                    other => guest_rejection(*other),
                });
            }
            return Err(WireError::new(ErrorCode::UpstreamError));
        };
        let items = listing
            .entries
            .clone()
            .into_iter()
            .map(public_entry)
            .collect::<WireResult<Vec<_>>>()?;
        let next_cursor = if let Some(after) = &listing.next_after {
            let binding = self.live_list_binding(
                session_id,
                query_hash,
                reply.generation,
                reply.lifecycle_fence,
            )?;
            Some(
                encode_state(
                    self.shared().cursor_keys.current(),
                    &binding,
                    &LiveListCursor {
                        after: after.as_str().to_owned(),
                        generation_id: reply.generation,
                        lifecycle_fence: reply.lifecycle_fence,
                    },
                    self.now()?,
                )
                .map_err(|error| WireError::from(ProjectionError::Cursor(error)))?,
            )
        } else {
            None
        };
        Ok(models::LiveFileEntryPage {
            items,
            next_cursor,
            workspace_access: workspace_access(&reply),
        })
    }

    async fn session_files_live_stat(
        &self,
        _cx: &WireContext,
        session_id: SessionId,
        body: models::LiveFileStatRequest,
    ) -> WireResult<models::LiveFileEntry> {
        let pointer = self
            .live_generation(session_id, body.if_generation_id)
            .await?;
        let reply = self
            .live_call(
                session_id,
                pointer.generation,
                &[FileRequest::Stat {
                    path: guest_path(&body.path)?,
                }],
            )
            .await?;
        match one_response(&reply)? {
            FileResponse::Entry { entry } => Ok(models::LiveFileEntry {
                entry: public_entry(entry.clone())?,
                workspace_access: workspace_access(&reply),
            }),
            FileResponse::Rejected { code } => Err(guest_rejection(*code)),
            _ => Err(WireError::new(ErrorCode::UpstreamError)),
        }
    }

    async fn session_files_live_download_complete(
        &self,
        _cx: &WireContext,
        session_id: SessionId,
        file_download_id: FileDownloadId,
        body: models::LiveFileDownloadCompleteRequest,
    ) -> WireResult<models::LiveFileDownload> {
        let pointer = self.live_generation(session_id, None).await?;
        let TransferRecord::Download(stored) = self
            .owned_transfer(session_id, TransferKey::Download(file_download_id))
            .await?
        else {
            return Err(WireError::new(ErrorCode::NotFound));
        };
        if stored.generation != pointer.generation {
            return Err(WireError::new(ErrorCode::PreconditionFailed));
        }
        let descriptor = stored
            .descriptor
            .as_ref()
            .ok_or_else(|| WireError::new(ErrorCode::ResourceConflict))?;
        if body.version != descriptor.version || body.sha256 != descriptor.sha256 {
            return Err(WireError::new(ErrorCode::PreconditionFailed));
        }
        let (mut election, _) = self
            .elect(
                session_id,
                RouteId::SessionFilesLiveDownloadComplete,
                TransferKey::Download(file_download_id),
                None,
                stored.expires_at,
            )
            .await?;
        if let Some(response) = replay_response(&election)? {
            return Ok(response);
        }
        let reply = self
            .live_call(
                session_id,
                pointer.generation,
                &[FileRequest::DownloadClose {
                    download: GuestDownloadId(file_download_id.uuid7()),
                }],
            )
            .await?;
        match one_response(&reply)? {
            FileResponse::DownloadClosed {
                download,
                sha256,
                version,
            } if *download == GuestDownloadId(file_download_id.uuid7())
                && *sha256 == body.sha256
                && *version == body.version => {}
            FileResponse::Rejected { code } => {
                return Err(match code {
                    aex_hands_protocol::files::FileFailureCode::Conflict => {
                        WireError::new(ErrorCode::PreconditionFailed)
                    }
                    other => guest_rejection(*other),
                });
            }
            _ => return Err(WireError::new(ErrorCode::UpstreamError)),
        }
        let mut completed = stored;
        let expected_revision = completed.revision;
        completed.state = TransferState::Verified;
        completed.expires_at = reply.expires_at;
        completed.revision = completed.revision.saturating_add(1);
        let response = public_download(&completed, workspace_access(&reply))?;
        let canonical =
            serde_json::to_vec(&response).map_err(|_| WireError::new(ErrorCode::InternalError))?;
        election.expires_at = reply.expires_at;
        self.shared()
            .live_transfers
            .settle(
                expected_revision,
                &TransferRecord::Download(completed),
                &election,
                &canonical,
            )
            .await
            .map_err(transfer_failure)?;
        Ok(response)
    }

    async fn session_files_live_download_create(
        &self,
        _cx: &WireContext,
        session_id: SessionId,
        body: models::LiveFileDownloadRequest,
    ) -> WireResult<Created<models::LiveFileDownload>> {
        let pointer = self
            .live_generation(session_id, body.if_generation_id)
            .await?;
        let id = FileDownloadId::from_uuid7(Self::new_uuid7());
        let upper_expiry = self.transfer_upper_expiry()?;
        let create = TransferRecord::Download(DownloadTransfer {
            workspace: self.context().auth.workspace_id,
            session: session_id,
            generation: pointer.generation,
            id,
            path: body.path.clone(),
            expires_at: upper_expiry,
            descriptor: None,
            state: TransferState::Opening,
            revision: 0,
        });
        let (mut election, elected) = self
            .elect(
                session_id,
                RouteId::SessionFilesLiveDownloadCreate,
                TransferKey::Download(id),
                Some(create),
                upper_expiry,
            )
            .await?;
        if let Some(response) = replay_response(&election)? {
            return Ok(Created(response));
        }
        let Some(TransferRecord::Download(mut record)) = elected else {
            return Err(WireError::new(ErrorCode::InternalError));
        };
        let reply = self
            .live_call(
                session_id,
                record.generation,
                &[FileRequest::DownloadOpen {
                    download: GuestDownloadId(record.id.uuid7()),
                    path: guest_path(&record.path)?,
                    range: None,
                }],
            )
            .await?;
        let state = match one_response(&reply)? {
            FileResponse::DownloadOpened { state } => state.clone(),
            FileResponse::Rejected { code } => return Err(guest_rejection(*code)),
            _ => return Err(WireError::new(ErrorCode::UpstreamError)),
        };
        require_download_state(&record, &state)?;
        let expected_revision = record.revision;
        record.descriptor = Some(state);
        record.state = TransferState::Open;
        record.expires_at = reply.expires_at;
        record.revision = record.revision.saturating_add(1);
        let response = public_download(&record, workspace_access(&reply))?;
        let canonical =
            serde_json::to_vec(&response).map_err(|_| WireError::new(ErrorCode::InternalError))?;
        election.expires_at = reply.expires_at;
        self.shared()
            .live_transfers
            .settle(
                expected_revision,
                &TransferRecord::Download(record),
                &election,
                &canonical,
            )
            .await
            .map_err(transfer_failure)?;
        Ok(Created(response))
    }

    async fn session_files_live_download_delete(
        &self,
        _cx: &WireContext,
        session_id: SessionId,
        file_download_id: FileDownloadId,
    ) -> WireResult<NoContent> {
        let pointer = self.live_generation(session_id, None).await?;
        let TransferRecord::Download(mut record) = self
            .owned_transfer(session_id, TransferKey::Download(file_download_id))
            .await?
        else {
            return Err(WireError::new(ErrorCode::NotFound));
        };
        if record.generation != pointer.generation {
            return Err(WireError::new(ErrorCode::PreconditionFailed));
        }
        if !matches!(record.state, TransferState::Aborted) {
            let reply = self
                .live_call(
                    session_id,
                    pointer.generation,
                    &[FileRequest::DownloadAbort {
                        download: GuestDownloadId(file_download_id.uuid7()),
                    }],
                )
                .await?;
            match one_response(&reply)? {
                FileResponse::DownloadAborted { download }
                    if *download == GuestDownloadId(file_download_id.uuid7()) => {}
                FileResponse::Rejected {
                    code: aex_hands_protocol::files::FileFailureCode::NotFound,
                } => {}
                FileResponse::Rejected { code } => return Err(guest_rejection(*code)),
                _ => return Err(WireError::new(ErrorCode::UpstreamError)),
            }
            let expected = record.revision;
            record.state = TransferState::Aborted;
            record.revision = record.revision.saturating_add(1);
            self.shared()
                .live_transfers
                .save(expected, &TransferRecord::Download(record))
                .await
                .map_err(transfer_failure)?;
        }
        Ok(NoContent)
    }

    async fn session_files_live_download_part_get(
        &self,
        _cx: &WireContext,
        session_id: SessionId,
        file_download_id: FileDownloadId,
        part_number: u32,
        query: models::SessionFilesLiveDownloadPartGetQuery,
    ) -> WireResult<aex_wire::server::BinaryBody> {
        let pointer = self.live_generation(session_id, None).await?;
        let TransferRecord::Download(record) = self
            .owned_transfer(session_id, TransferKey::Download(file_download_id))
            .await?
        else {
            return Err(WireError::new(ErrorCode::NotFound));
        };
        if record.generation != pointer.generation {
            return Err(WireError::new(ErrorCode::PreconditionFailed));
        }
        if !matches!(record.state, TransferState::Open) {
            return Err(WireError::new(ErrorCode::ResourceConflict));
        }
        let descriptor = record
            .descriptor
            .as_ref()
            .ok_or_else(|| WireError::new(ErrorCode::ResourceConflict))?;
        if query.version != descriptor.version {
            return Err(WireError::new(ErrorCode::PreconditionFailed));
        }
        let part = descriptor
            .parts
            .iter()
            .find(|part| part.part_number == part_number)
            .ok_or_else(|| WireError::new(ErrorCode::InvalidRequest))?
            .clone();
        let requests = download_chunk_requests(file_download_id, &part);
        let mut reply = self
            .live_call(session_id, pointer.generation, &requests)
            .await?;
        if reply.responses.first().is_some_and(|response| {
            matches!(
                response,
                FileResponse::Rejected {
                    code: aex_hands_protocol::files::FileFailureCode::NotFound
                }
            )
        }) {
            let reopened = self
                .live_call(
                    session_id,
                    pointer.generation,
                    &[FileRequest::DownloadOpen {
                        download: GuestDownloadId(file_download_id.uuid7()),
                        path: guest_path(&record.path)?,
                        range: None,
                    }],
                )
                .await?;
            match one_response(&reopened)? {
                FileResponse::DownloadOpened { state } if state == descriptor => {}
                FileResponse::DownloadOpened { .. } => {
                    return Err(WireError::new(ErrorCode::PreconditionFailed));
                }
                FileResponse::Rejected { code } => return Err(guest_rejection(*code)),
                _ => return Err(WireError::new(ErrorCode::UpstreamError)),
            }
            reply = self
                .live_call(session_id, pointer.generation, &requests)
                .await?;
        }
        let bytes = assemble_download_part(file_download_id, &part, &reply.responses)?;
        Ok(aex_wire::server::BinaryBody(bytes))
    }

    async fn session_files_live_upload_complete(
        &self,
        _cx: &WireContext,
        session_id: SessionId,
        file_upload_id: FileUploadId,
    ) -> WireResult<models::LiveFileUpload> {
        let pointer = self.live_generation(session_id, None).await?;
        let TransferRecord::Upload(stored) = self
            .owned_transfer(session_id, TransferKey::Upload(file_upload_id))
            .await?
        else {
            return Err(WireError::new(ErrorCode::NotFound));
        };
        if stored.generation != pointer.generation {
            return Err(WireError::new(ErrorCode::PreconditionFailed));
        }
        let (mut election, _) = self
            .elect(
                session_id,
                RouteId::SessionFilesLiveUploadComplete,
                TransferKey::Upload(file_upload_id),
                None,
                stored.expires_at,
            )
            .await?;
        if let Some(response) = replay_response(&election)? {
            return Ok(response);
        }
        let reply = self
            .live_call(
                session_id,
                pointer.generation,
                &[FileRequest::UploadComplete {
                    upload: GuestUploadId(file_upload_id.uuid7()),
                }],
            )
            .await?;
        let state = match one_response(&reply)? {
            FileResponse::UploadComplete { state } => state.clone(),
            FileResponse::Rejected {
                code: aex_hands_protocol::files::FileFailureCode::InvalidRequest,
            } => return Err(WireError::new(ErrorCode::ContentMissing)),
            FileResponse::Rejected { code } => return Err(guest_rejection(*code)),
            _ => return Err(WireError::new(ErrorCode::UpstreamError)),
        };
        let mut completed = stored;
        let expected_revision = completed.revision;
        completed.state = TransferState::Complete;
        completed.expires_at = reply.expires_at;
        completed.revision = completed.revision.saturating_add(1);
        let response = public_upload(&completed, &state, workspace_access(&reply))?;
        let canonical =
            serde_json::to_vec(&response).map_err(|_| WireError::new(ErrorCode::InternalError))?;
        election.expires_at = reply.expires_at;
        self.shared()
            .live_transfers
            .settle(
                expected_revision,
                &TransferRecord::Upload(completed),
                &election,
                &canonical,
            )
            .await
            .map_err(transfer_failure)?;
        Ok(response)
    }

    async fn session_files_live_upload_create(
        &self,
        _cx: &WireContext,
        session_id: SessionId,
        body: models::LiveFileUploadCreateRequest,
    ) -> WireResult<Created<models::LiveFileUpload>> {
        let size_bytes = u64::try_from(body.size_bytes.get())
            .map_err(|_| WireError::new(ErrorCode::LimitExceeded))?;
        if size_bytes > MAX_FILE_BYTES {
            return Err(WireError::new(ErrorCode::LimitExceeded));
        }
        let pointer = self
            .live_generation(session_id, body.if_generation_id)
            .await?;
        let id = FileUploadId::from_uuid7(Self::new_uuid7());
        let mode = body.mode.unwrap_or(models::RegisteredFileMode::V0644);
        let upper_expiry = self.transfer_upper_expiry()?;
        let create = TransferRecord::Upload(UploadTransfer {
            workspace: self.context().auth.workspace_id,
            session: session_id,
            generation: pointer.generation,
            id,
            path: body.path,
            size_bytes,
            sha256: body.sha256,
            mode,
            expires_at: upper_expiry,
            state: TransferState::Opening,
            revision: 0,
        });
        let (mut election, elected) = self
            .elect(
                session_id,
                RouteId::SessionFilesLiveUploadCreate,
                TransferKey::Upload(id),
                Some(create),
                upper_expiry,
            )
            .await?;
        if let Some(response) = replay_response(&election)? {
            return Ok(Created(response));
        }
        let Some(TransferRecord::Upload(mut record)) = elected else {
            return Err(WireError::new(ErrorCode::InternalError));
        };
        let reply = self
            .live_call(
                session_id,
                record.generation,
                &[FileRequest::UploadOpen {
                    upload: GuestUploadId(record.id.uuid7()),
                    path: guest_path(&record.path)?,
                    size_bytes: record.size_bytes,
                    sha256: record.sha256,
                    mode: upload_mode(record.mode),
                }],
            )
            .await?;
        let state = match one_response(&reply)? {
            FileResponse::Upload { state } => state.clone(),
            FileResponse::Rejected { code } => return Err(guest_rejection(*code)),
            _ => return Err(WireError::new(ErrorCode::UpstreamError)),
        };
        require_upload_state(&record, &state)?;
        let expected_revision = record.revision;
        record.state = TransferState::Open;
        record.expires_at = reply.expires_at;
        record.revision = record.revision.saturating_add(1);
        let response = public_upload(&record, &state, workspace_access(&reply))?;
        let canonical =
            serde_json::to_vec(&response).map_err(|_| WireError::new(ErrorCode::InternalError))?;
        election.expires_at = reply.expires_at;
        self.shared()
            .live_transfers
            .settle(
                expected_revision,
                &TransferRecord::Upload(record),
                &election,
                &canonical,
            )
            .await
            .map_err(transfer_failure)?;
        Ok(Created(response))
    }

    async fn session_files_live_upload_delete(
        &self,
        _cx: &WireContext,
        session_id: SessionId,
        file_upload_id: FileUploadId,
    ) -> WireResult<NoContent> {
        let pointer = self.live_generation(session_id, None).await?;
        let TransferRecord::Upload(mut record) = self
            .owned_transfer(session_id, TransferKey::Upload(file_upload_id))
            .await?
        else {
            return Err(WireError::new(ErrorCode::NotFound));
        };
        if record.generation != pointer.generation {
            return Err(WireError::new(ErrorCode::PreconditionFailed));
        }
        if !matches!(record.state, TransferState::Aborted) {
            let reply = self
                .live_call(
                    session_id,
                    pointer.generation,
                    &[FileRequest::UploadAbort {
                        upload: GuestUploadId(file_upload_id.uuid7()),
                    }],
                )
                .await?;
            match one_response(&reply)? {
                FileResponse::UploadAborted { upload }
                    if *upload == GuestUploadId(file_upload_id.uuid7()) => {}
                FileResponse::Rejected {
                    code: aex_hands_protocol::files::FileFailureCode::NotFound,
                } => {}
                FileResponse::Rejected { code } => return Err(guest_rejection(*code)),
                _ => return Err(WireError::new(ErrorCode::UpstreamError)),
            }
            let expected = record.revision;
            record.state = TransferState::Aborted;
            record.revision = record.revision.saturating_add(1);
            self.shared()
                .live_transfers
                .save(expected, &TransferRecord::Upload(record))
                .await
                .map_err(transfer_failure)?;
        }
        Ok(NoContent)
    }

    async fn session_files_live_upload_get(
        &self,
        _cx: &WireContext,
        session_id: SessionId,
        file_upload_id: FileUploadId,
    ) -> WireResult<models::LiveFileUpload> {
        let pointer = self.live_generation(session_id, None).await?;
        let TransferRecord::Upload(record) = self
            .owned_transfer(session_id, TransferKey::Upload(file_upload_id))
            .await?
        else {
            return Err(WireError::new(ErrorCode::NotFound));
        };
        if record.generation != pointer.generation {
            return Err(WireError::new(ErrorCode::PreconditionFailed));
        }
        if matches!(
            record.state,
            TransferState::Opening | TransferState::Aborted
        ) {
            return Err(WireError::new(ErrorCode::ResourceConflict));
        }
        let reply = self
            .live_call(
                session_id,
                pointer.generation,
                &[FileRequest::UploadStatus {
                    upload: GuestUploadId(file_upload_id.uuid7()),
                }],
            )
            .await?;
        match one_response(&reply)? {
            FileResponse::Upload { state } | FileResponse::UploadComplete { state } => {
                public_upload(&record, state, workspace_access(&reply))
            }
            FileResponse::Rejected { code } => Err(guest_rejection(*code)),
            _ => Err(WireError::new(ErrorCode::UpstreamError)),
        }
    }

    async fn session_files_live_upload_part_put(
        &self,
        _cx: &WireContext,
        session_id: SessionId,
        file_upload_id: FileUploadId,
        part_number: u32,
        query: models::SessionFilesLiveUploadPartPutQuery,
        body: &[u8],
    ) -> WireResult<models::LiveFileUpload> {
        let pointer = self.live_generation(session_id, None).await?;
        let TransferRecord::Upload(record) = self
            .owned_transfer(session_id, TransferKey::Upload(file_upload_id))
            .await?
        else {
            return Err(WireError::new(ErrorCode::NotFound));
        };
        if record.generation != pointer.generation {
            return Err(WireError::new(ErrorCode::PreconditionFailed));
        }
        if !matches!(record.state, TransferState::Open) {
            return Err(WireError::new(ErrorCode::ResourceConflict));
        }
        let (offset, size_bytes) = part_shape(record.size_bytes, part_number)?;
        if body.len() != usize::try_from(size_bytes).unwrap_or(usize::MAX) {
            return Err(WireError::new(ErrorCode::InvalidRequest).with_message("part size"));
        }
        if ContentHash::of(body) != query.sha256 {
            return Err(WireError::new(ErrorCode::InvalidRequest).with_message("part checksum"));
        }
        let requests =
            upload_part_requests(file_upload_id, part_number, offset, query.sha256, body);
        let reply = self
            .live_call(session_id, pointer.generation, &requests)
            .await?;
        if let Some(FileResponse::Rejected { code }) = reply
            .responses
            .iter()
            .find(|response| matches!(response, FileResponse::Rejected { .. }))
        {
            return Err(guest_rejection(*code));
        }
        let Some(FileResponse::Upload { state }) = reply.responses.last() else {
            return Err(WireError::new(ErrorCode::UpstreamError));
        };
        public_upload(&record, state, workspace_access(&reply))
    }
}

fn upload_part_requests(
    upload: FileUploadId,
    part_number: u32,
    offset: u64,
    sha256: ContentHash,
    body: &[u8],
) -> Vec<FileRequest> {
    let guest = GuestUploadId(upload.uuid7());
    let mut requests = Vec::with_capacity(8);
    requests.push(FileRequest::UploadPartOpen {
        upload: guest,
        part_number,
        offset,
        size_bytes: u32::try_from(body.len()).unwrap_or(u32::MAX),
        sha256,
    });
    for (index, bytes) in body.chunks(FILE_FRAME_BYTES as usize).enumerate() {
        requests.push(FileRequest::UploadPartChunk {
            upload: guest,
            part_number,
            chunk_offset: u32::try_from(index.saturating_mul(FILE_FRAME_BYTES as usize))
                .unwrap_or(u32::MAX),
            sha256: ContentHash::of(bytes),
            bytes: bytes.to_vec(),
        });
    }
    requests.push(FileRequest::UploadPartComplete {
        upload: guest,
        part_number,
    });
    requests
}

fn download_chunk_requests(download: FileDownloadId, part: &FilePartReceipt) -> Vec<FileRequest> {
    let mut requests = Vec::with_capacity(6);
    let mut offset = part.offset;
    let end = part.offset.saturating_add(u64::from(part.size_bytes));
    while offset < end {
        let length = (end - offset).min(u64::from(FILE_FRAME_BYTES));
        requests.push(FileRequest::DownloadChunk {
            download: GuestDownloadId(download.uuid7()),
            offset,
            max_bytes: u32::try_from(length).unwrap_or(FILE_FRAME_BYTES),
        });
        offset = offset.saturating_add(length);
    }
    requests
}

fn assemble_download_part(
    download: FileDownloadId,
    part: &FilePartReceipt,
    responses: &[FileResponse],
) -> WireResult<Vec<u8>> {
    if responses.len() != download_chunk_requests(download, part).len() {
        return Err(WireError::new(ErrorCode::UpstreamError));
    }
    let mut bytes = Vec::with_capacity(part.size_bytes as usize);
    let mut expected_offset = part.offset;
    for response in responses {
        match response {
            FileResponse::DownloadChunk { chunk }
                if chunk.download == GuestDownloadId(download.uuid7())
                    && chunk.offset == expected_offset
                    && chunk.sha256 == ContentHash::of(&chunk.bytes)
                    && !chunk.bytes.is_empty()
                    && chunk.bytes.len() <= FILE_FRAME_BYTES as usize =>
            {
                expected_offset = expected_offset.saturating_add(chunk.bytes.len() as u64);
                bytes.extend_from_slice(&chunk.bytes);
            }
            FileResponse::Rejected { code } => return Err(guest_rejection(*code)),
            _ => return Err(WireError::new(ErrorCode::UpstreamError)),
        }
    }
    if bytes.len() != part.size_bytes as usize || ContentHash::of(&bytes) != part.sha256 {
        return Err(WireError::new(ErrorCode::PreconditionFailed));
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use aex_session_domain::lifecycle::LifecycleStatus;

    fn generation() -> GenerationId {
        GenerationId::from_uuid7(Uuid7::compose(1, [7; 10]))
    }

    #[test]
    fn terminal_lifecycle_refuses_before_runtime_or_guest_authority() {
        for status in [LifecycleStatus::Terminating, LifecycleStatus::Terminated] {
            let error = generation_for_live_status(status, generation())
                .expect_err("terminal live access must stop at the session head");
            assert_eq!(error.code, ErrorCode::SessionTerminated);
        }
        let deleting = generation_for_live_status(LifecycleStatus::Deleting, generation())
            .expect_err("deleting live access must stop at the session head");
        assert_eq!(deleting.code, ErrorCode::SessionDeleting);
    }

    #[test]
    fn suspended_live_access_preserves_the_exact_generation_for_native_resume() {
        assert_eq!(
            generation_for_live_status(LifecycleStatus::Suspended, generation())
                .expect("suspended sessions use native endpoint resume"),
            generation()
        );
    }

    #[test]
    fn five_gibibytes_is_exactly_1280_public_parts() {
        assert_eq!(part_count(MAX_FILE_BYTES), 1_280);
        assert_eq!(
            part_shape(MAX_FILE_BYTES, 1_280).expect("the final maximum part"),
            (
                MAX_FILE_BYTES - u64::from(FILE_TRANSFER_PART_BYTES),
                FILE_TRANSFER_PART_BYTES
            )
        );
    }
}
