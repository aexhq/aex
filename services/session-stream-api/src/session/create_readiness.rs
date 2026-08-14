//! Exact selected-content transfer performed by sandbox preparation.
//!
//! The create route resolves and elects registry metadata before provider
//! launch. This module then reads only the elected content digest and streams
//! it through the private exact-generation Hands file RPC. A response is not
//! publishable until every file reaches its final atomic guest path.

use std::future::Future;

use aex_content_aws::ContentObjectStore;
use aex_hands_protocol::files::{
    FILE_FRAME_BYTES, FILE_TRANSFER_PART_BYTES, FileFailureCode, FilePartReceipt, FileRequest,
    FileResponse, FileUploadId, FileUploadState,
};
use aex_hands_protocol::operation::{FileMode, GuestPath, GuestRoot};
use aex_hands_protocol::rpc::HandsOperationId;
use aex_session_dynamodb::create_preparation::{CreatePreparation, PreparedFile};
use aex_wire::ids::{ContentHash, GenerationId, PrefixedId as _, ResourceName, SessionId, Uuid7};
use sha2::{Digest as _, Sha256};

/// One verified body read from the content authority.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StartupBody {
    /// Exact bytes whose length and digest were reverified.
    pub bytes: Vec<u8>,
}

/// Minimal content port used by asynchronous sandbox preparation.
#[async_trait::async_trait]
pub trait StartupContent: Send + Sync {
    /// Reads exactly one elected body, bounded by its persisted size.
    async fn read(&self, file: &PreparedFile) -> Result<StartupBody, StartupFileError>;
}

/// Production content adapter over the existing immutable object store.
pub struct ContentObjects<'a> {
    store: &'a dyn ContentObjectStore,
    workspace: aex_wire::ids::WorkspaceId,
}

impl<'a> ContentObjects<'a> {
    /// Binds one create's workspace to the shared content authority.
    #[must_use]
    pub const fn new(
        store: &'a dyn ContentObjectStore,
        workspace: aex_wire::ids::WorkspaceId,
    ) -> Self {
        Self { store, workspace }
    }
}

#[async_trait::async_trait]
impl StartupContent for ContentObjects<'_> {
    async fn read(&self, file: &PreparedFile) -> Result<StartupBody, StartupFileError> {
        let object = self
            .store
            .read_bounded(self.workspace, &file.content, file.size_bytes)
            .await
            .map_err(|error| StartupFileError::Content(error.to_string()))?;
        let received = u64::try_from(object.body.len()).unwrap_or(u64::MAX);
        if object.declared_bytes != file.size_bytes
            || received != file.size_bytes
            || ContentHash::of(&object.body) != file.content
        {
            return Err(StartupFileError::ContentIntegrity);
        }
        Ok(StartupBody { bytes: object.body })
    }
}

/// Minimal exact-generation guest port used by asynchronous sandbox preparation.
pub trait StartupGuest: Send + Sync {
    /// Sends one bounded request batch under one deterministic activity id.
    fn call(
        &self,
        session: SessionId,
        generation: GenerationId,
        activity: HandsOperationId,
        requests: Vec<FileRequest>,
    ) -> impl Future<Output = Result<Vec<FileResponse>, StartupGuestError>> + Send;
}

/// A file whose final guest rename was verified.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MaterializedFile {
    /// Registered public name.
    pub name: ResourceName,
    /// Exact selected registry revision.
    pub revision: u64,
}

/// Closed guest failure vocabulary for elected sandbox transfer.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum StartupGuestError {
    /// Guest or provider transport was unavailable.
    #[error("startup guest transport is unavailable")]
    Unavailable,
    /// The exact-generation guest rejected a request.
    #[error("startup guest rejected the request as {0:?}")]
    Rejected(FileFailureCode),
    /// The guest response did not match the elected request.
    #[error("startup guest returned mismatched state")]
    Mismatched,
}

/// Why elected startup bytes cannot become sandbox preparation readiness.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum StartupFileError {
    /// The existing content adapter refused the exact bounded read.
    #[error("startup content read failed: {0}")]
    Content(String),
    /// The returned body differed from persisted digest/size metadata.
    #[error("startup content failed exact digest/size verification")]
    ContentIntegrity,
    /// The persisted file path could not map inside `/workspace`.
    #[error("startup mount path is not a valid guest workspace path")]
    InvalidPath,
    /// A selected startup file cannot exist without a sandbox generation.
    #[error("startup files require an exact sandbox generation")]
    MissingGeneration,
    /// The guest failed or returned state for another upload.
    #[error(transparent)]
    Guest(#[from] StartupGuestError),
}

/// Materializes every elected file in exact request order.
///
/// # Errors
///
/// Returns [`StartupFileError`] on the first content or exact-generation guest
/// mismatch. Eager callers leave the durable checkpoint requested after an
/// error; detached first-tool recovery reloads the same elected manifest and
/// idempotently resumes it.
pub async fn materialize_selected<C: StartupContent, G: StartupGuest>(
    content: &C,
    guest: &G,
    prepared: &CreatePreparation,
) -> Result<Vec<MaterializedFile>, StartupFileError> {
    let generation = prepared
        .generation
        .ok_or(StartupFileError::MissingGeneration)?;
    let mut materialized = Vec::with_capacity(prepared.files.len());
    for file in &prepared.files {
        let body = content.read(file).await?;
        let upload = upload_id(generation, file);
        let batches = upload_batches(upload, file, &body.bytes)?;
        let expected = expected_state(upload, file, &body.bytes)?;
        execute_upload(
            guest,
            prepared.session,
            generation,
            upload,
            file_mode(file.mode),
            batches,
            &expected,
        )
        .await?;
        materialized.push(MaterializedFile {
            name: file.name.clone(),
            revision: file.revision,
        });
    }
    Ok(materialized)
}

async fn execute_upload<G: StartupGuest>(
    guest: &G,
    session: SessionId,
    generation: GenerationId,
    upload: FileUploadId,
    mode: FileMode,
    batches: Vec<Vec<FileRequest>>,
    expected: &FileUploadState,
) -> Result<(), StartupFileError> {
    for (ordinal, batch) in batches.into_iter().enumerate() {
        if ordinal == 0 {
            verify_open_request(expected, mode, &batch)?;
        }
        let request_count = batch.len();
        let responses = guest
            .call(
                session,
                generation,
                activity_id(upload, ordinal),
                batch,
            )
            .await?;
        if verify_batch(expected, ordinal, request_count, &responses)?
            == UploadBatchProgress::Complete
        {
            break;
        }
    }
    Ok(())
}

fn upload_batches(
    upload: FileUploadId,
    file: &PreparedFile,
    bytes: &[u8],
) -> Result<Vec<Vec<FileRequest>>, StartupFileError> {
    if u64::try_from(bytes.len()).ok() != Some(file.size_bytes)
        || ContentHash::of(bytes) != file.content
    {
        return Err(StartupFileError::ContentIntegrity);
    }
    let path = guest_path(&file.mount_path)?;
    let mut batches = vec![vec![FileRequest::UploadOpen {
        upload,
        path,
        size_bytes: file.size_bytes,
        sha256: file.content,
        mode: file_mode(file.mode),
    }]];
    for (part_index, part) in bytes.chunks(FILE_TRANSFER_PART_BYTES as usize).enumerate() {
        let part_number = u32::try_from(part_index + 1).unwrap_or(u32::MAX);
        let offset = u64::try_from(part_index)
            .unwrap_or(u64::MAX)
            .saturating_mul(u64::from(FILE_TRANSFER_PART_BYTES));
        let part_digest = ContentHash::of(part);
        let mut requests = Vec::with_capacity(part.len().div_ceil(FILE_FRAME_BYTES as usize) + 2);
        requests.push(FileRequest::UploadPartOpen {
            upload,
            part_number,
            offset,
            size_bytes: u32::try_from(part.len()).unwrap_or(u32::MAX),
            sha256: part_digest,
        });
        for (chunk_index, chunk) in part.chunks(FILE_FRAME_BYTES as usize).enumerate() {
            requests.push(FileRequest::UploadPartChunk {
                upload,
                part_number,
                chunk_offset: u32::try_from(chunk_index.saturating_mul(FILE_FRAME_BYTES as usize))
                    .unwrap_or(u32::MAX),
                sha256: ContentHash::of(chunk),
                bytes: chunk.to_vec(),
            });
        }
        requests.push(FileRequest::UploadPartComplete {
            upload,
            part_number,
        });
        batches.push(requests);
    }
    batches.push(vec![FileRequest::UploadComplete { upload }]);
    Ok(batches)
}

fn expected_state(
    upload: FileUploadId,
    file: &PreparedFile,
    bytes: &[u8],
) -> Result<FileUploadState, StartupFileError> {
    if u64::try_from(bytes.len()).ok() != Some(file.size_bytes)
        || ContentHash::of(bytes) != file.content
    {
        return Err(StartupFileError::ContentIntegrity);
    }
    Ok(FileUploadState {
        upload,
        path: guest_path(&file.mount_path)?,
        size_bytes: file.size_bytes,
        sha256: file.content,
        parts: bytes
            .chunks(FILE_TRANSFER_PART_BYTES as usize)
            .enumerate()
            .map(|(index, part)| FilePartReceipt {
                part_number: u32::try_from(index + 1).unwrap_or(u32::MAX),
                offset: u64::try_from(index)
                    .unwrap_or(u64::MAX)
                    .saturating_mul(u64::from(FILE_TRANSFER_PART_BYTES)),
                size_bytes: u32::try_from(part.len()).unwrap_or(u32::MAX),
                sha256: ContentHash::of(part),
            })
            .collect(),
        complete: false,
    })
}

fn verify_open_request(
    expected: &FileUploadState,
    mode: FileMode,
    requests: &[FileRequest],
) -> Result<(), StartupFileError> {
    match requests {
        [FileRequest::UploadOpen {
            upload,
            path,
            size_bytes,
            sha256,
            mode: requested_mode,
        }] if *upload == expected.upload
            && *path == expected.path
            && *size_bytes == expected.size_bytes
            && *sha256 == expected.sha256
            && *requested_mode == mode =>
        {
            Ok(())
        }
        _ => Err(StartupGuestError::Mismatched.into()),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum UploadBatchProgress {
    Pending,
    Complete,
}

fn verify_batch(
    expected: &FileUploadState,
    ordinal: usize,
    request_count: usize,
    responses: &[FileResponse],
) -> Result<UploadBatchProgress, StartupFileError> {
    if responses.len() != request_count {
        return Err(StartupGuestError::Mismatched.into());
    }
    for response in responses {
        if let FileResponse::Rejected { code } = response {
            return Err(StartupGuestError::Rejected(*code).into());
        }
    }
    let final_batch = ordinal == part_count(expected.size_bytes) + 1;
    let state = match responses.last() {
        Some(FileResponse::Upload { state }) if !final_batch => state,
        Some(FileResponse::UploadComplete { state }) if final_batch => state,
        _ => return Err(StartupGuestError::Mismatched.into()),
    };

    // A successful UploadOpen is also the guest's evidence that its persisted
    // mode matches the exact mode in the request: the protocol requires a
    // conflicting reopen to be rejected. A completed reopen is therefore safe
    // to short-circuit only when every response-visible immutable field and the
    // full ordered part manifest also match.
    if ordinal == 0 && state.complete {
        if state.upload != expected.upload
            || state.path != expected.path
            || state.size_bytes != expected.size_bytes
            || state.sha256 != expected.sha256
            || state.parts != expected.parts
        {
            return Err(StartupGuestError::Mismatched.into());
        }
        return Ok(UploadBatchProgress::Complete);
    }
    if state.upload != expected.upload
        || state.path != expected.path
        || state.size_bytes != expected.size_bytes
        || state.sha256 != expected.sha256
        || state.complete != final_batch
        || state.parts != expected.parts[..ordinal.min(expected.parts.len())]
    {
        return Err(StartupGuestError::Mismatched.into());
    }
    let expected_part_count = ordinal.min(expected.parts.len());
    if state.parts.len() != expected_part_count
        || (final_batch && state.parts.len() != expected.parts.len())
    {
        return Err(StartupGuestError::Mismatched.into());
    }
    Ok(if final_batch {
        UploadBatchProgress::Complete
    } else {
        UploadBatchProgress::Pending
    })
}

fn part_count(size: u64) -> usize {
    usize::try_from(size.div_ceil(u64::from(FILE_TRANSFER_PART_BYTES))).unwrap_or(usize::MAX)
}

fn guest_path(path: &aex_wire::ids::FilePath) -> Result<GuestPath, StartupFileError> {
    let root = GuestRoot::workspace();
    let absolute = if path.as_str() == "/" {
        root.0.clone()
    } else {
        format!("{}{}", root.0, path.as_str())
    };
    GuestPath::parse(&root, &absolute).map_err(|_| StartupFileError::InvalidPath)
}

const fn file_mode(mode: aex_wire::models::RegisteredFileMode) -> FileMode {
    match mode {
        aex_wire::models::RegisteredFileMode::V0644 => FileMode::ReadWrite,
        aex_wire::models::RegisteredFileMode::V0755 => FileMode::Executable,
    }
}

fn upload_id(generation: GenerationId, file: &PreparedFile) -> FileUploadId {
    let mut hasher = Sha256::new();
    hasher.update(b"aex.session-create.startup-file.v1\0");
    hasher.update(generation.uuid7().as_bytes());
    hasher.update(file.name.as_str().as_bytes());
    hasher.update(file.revision.to_be_bytes());
    hasher.update(file.content.as_bytes());
    hasher.update(file.mount_path.as_str().as_bytes());
    let digest: [u8; 32] = hasher.finalize().into();
    let mut entropy = [0_u8; 10];
    entropy.copy_from_slice(&digest[..10]);
    FileUploadId(Uuid7::compose(generation.uuid7().unix_millis(), entropy))
}

fn activity_id(upload: FileUploadId, ordinal: usize) -> HandsOperationId {
    let mut hasher = Sha256::new();
    hasher.update(b"aex.session-create.startup-rpc.v1\0");
    hasher.update(upload.0.as_bytes());
    hasher.update(ordinal.to_be_bytes());
    let digest: [u8; 32] = hasher.finalize().into();
    let mut entropy = [0_u8; 10];
    entropy.copy_from_slice(&digest[..10]);
    HandsOperationId(Uuid7::compose(upload.0.unix_millis(), entropy))
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;
    use aex_wire::ids::ResourceName;
    use aex_wire::types::ETag;

    struct CompleteOnOpenGuest {
        state: FileUploadState,
        calls: AtomicUsize,
    }

    impl StartupGuest for CompleteOnOpenGuest {
        fn call(
            &self,
            _session: SessionId,
            _generation: GenerationId,
            _activity: HandsOperationId,
            _requests: Vec<FileRequest>,
        ) -> impl Future<Output = Result<Vec<FileResponse>, StartupGuestError>> + Send {
            let result = if self.calls.fetch_add(1, Ordering::SeqCst) == 0 {
                Ok(vec![FileResponse::Upload {
                    state: self.state.clone(),
                }])
            } else {
                Err(StartupGuestError::Mismatched)
            };
            std::future::ready(result)
        }
    }

    fn file(bytes: &[u8]) -> PreparedFile {
        PreparedFile {
            name: ResourceName::parse("startup").expect("name"),
            revision: 7,
            etag: ETag::parse("etag-7").expect("etag"),
            content: ContentHash::of(bytes),
            size_bytes: bytes.len() as u64,
            mount_path: aex_wire::ids::FilePath::parse("/src/startup.bin").expect("path"),
            media_type: "application/octet-stream".to_owned(),
            mode: aex_wire::models::RegisteredFileMode::V0644,
        }
    }

    #[test]
    fn four_mibibyte_parts_are_decomposed_into_bounded_guest_frames() {
        let bytes = vec![7; FILE_TRANSFER_PART_BYTES as usize + 1];
        let selected = file(&bytes);
        let upload = FileUploadId(Uuid7::compose(1, [1; 10]));
        let batches = upload_batches(upload, &selected, &bytes).expect("plan");
        assert_eq!(batches.len(), 4, "open + two parts + complete");
        let chunks = batches
            .iter()
            .flatten()
            .filter_map(|request| match request {
                FileRequest::UploadPartChunk { bytes, .. } => Some(bytes.len()),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(chunks.len(), 7, "six frames for 4 MiB and one final frame");
        assert!(chunks.iter().all(|size| *size <= FILE_FRAME_BYTES as usize));
    }

    #[test]
    fn empty_files_still_open_and_atomically_complete_without_a_part() {
        let selected = file(&[]);
        let upload = FileUploadId(Uuid7::compose(1, [1; 10]));
        let batches = upload_batches(upload, &selected, &[]).expect("plan");
        assert_eq!(batches.len(), 2);
        assert!(matches!(batches[0][0], FileRequest::UploadOpen { .. }));
        assert!(matches!(batches[1][0], FileRequest::UploadComplete { .. }));
    }

    #[test]
    fn replacement_metadata_mints_a_different_generation_local_upload() {
        let bytes = b"body";
        let original = file(bytes);
        let generation = GenerationId::from_uuid7(Uuid7::compose(1, [2; 10]));
        let first = upload_id(generation, &original);
        let mut replacement = original;
        replacement.revision += 1;
        assert_ne!(upload_id(generation, &replacement), first);
    }

    #[tokio::test]
    async fn completed_open_skips_every_remaining_upload_batch() {
        let bytes = b"already materialized";
        let selected = file(bytes);
        let generation = GenerationId::from_uuid7(Uuid7::compose(1, [3; 10]));
        let session = SessionId::from_uuid7(Uuid7::compose(1, [4; 10]));
        let upload = upload_id(generation, &selected);
        let batches = upload_batches(upload, &selected, bytes).expect("plan");
        let mut completed = expected_state(upload, &selected, bytes).expect("state");
        completed.complete = true;
        let guest = CompleteOnOpenGuest {
            state: completed,
            calls: AtomicUsize::new(0),
        };

        execute_upload(
            &guest,
            session,
            generation,
            upload,
            FileMode::ReadWrite,
            batches,
            &expected_state(upload, &selected, bytes).expect("expected"),
        )
        .await
        .expect("completed replay");

        assert_eq!(guest.calls.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn completed_open_requires_exact_metadata_and_ordered_part_manifest() {
        let bytes = vec![9; FILE_TRANSFER_PART_BYTES as usize + 1];
        let selected = file(&bytes);
        let upload = FileUploadId(Uuid7::compose(1, [5; 10]));
        let expected = expected_state(upload, &selected, &bytes).expect("state");
        let mut completed = expected.clone();
        completed.complete = true;

        let accepted = verify_batch(
            &expected,
            0,
            1,
            &[FileResponse::Upload {
                state: completed.clone(),
            }],
        );
        assert_eq!(accepted, Ok(UploadBatchProgress::Complete));

        let assert_mismatched = |state| {
            assert!(matches!(
                verify_batch(
                    &expected,
                    0,
                    1,
                    &[FileResponse::Upload { state }],
                ),
                Err(StartupFileError::Guest(StartupGuestError::Mismatched))
            ));
        };

        let mut wrong_path = completed.clone();
        wrong_path.path = GuestPath::parse(&GuestRoot::workspace(), "/workspace/other")
            .expect("guest path");
        assert_mismatched(wrong_path);

        let mut wrong_size = completed.clone();
        wrong_size.size_bytes += 1;
        assert_mismatched(wrong_size);

        let mut wrong_hash = completed.clone();
        wrong_hash.sha256 = ContentHash::of(b"other");
        assert_mismatched(wrong_hash);

        let mut wrong_order = completed;
        wrong_order.parts.swap(0, 1);
        assert_mismatched(wrong_order);
    }

    #[test]
    fn completed_open_requires_the_elected_mode_in_the_accepted_request() {
        let bytes = b"body";
        let selected = file(bytes);
        let upload = FileUploadId(Uuid7::compose(1, [6; 10]));
        let expected = expected_state(upload, &selected, bytes).expect("state");
        let mut batches = upload_batches(upload, &selected, bytes).expect("plan");
        let FileRequest::UploadOpen { mode, .. } = &mut batches[0][0] else {
            panic!("open request");
        };
        *mode = FileMode::Executable;

        assert!(matches!(
            verify_open_request(&expected, FileMode::ReadWrite, &batches[0]),
            Err(StartupFileError::Guest(StartupGuestError::Mismatched))
        ));
    }
}
