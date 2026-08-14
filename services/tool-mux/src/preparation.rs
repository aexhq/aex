//! Durable, exact-generation sandbox preparation used by first-call recovery.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use aex_content_aws::ContentObjectStore;
use aex_hands_protocol::files::{
    FILE_FRAME_BYTES, FILE_TRANSFER_PART_BYTES, FilePartReceipt, FileRequest, FileResponse,
    FileUploadId, FileUploadState,
};
use aex_hands_protocol::operation::{FileMode, GuestPath, GuestRoot};
use aex_hands_protocol::rpc::HandsOperationId;
use aex_runtime_control::HandId;
use aex_session_dynamodb::create_preparation::{CreatePreparationStore, PreparedFile};
use aex_session_dynamodb::sandbox_preparation::{
    SANDBOX_PREPARATION_HEARTBEAT_MILLIS, SandboxPreparationAuthority, SandboxPreparationClaim,
    SandboxPreparationLease,
};
use aex_tool_mux::{
    PreparationProgress, ReadyHand, TelemetryEvent, TelemetryKind, TelemetryProducer,
    ToolCallIdentity, ToolMuxFuture,
};
use aex_wire::ids::{ContentHash, GenerationId, PrefixedId as _, SessionId, Uuid7, WorkspaceId};
use sha2::{Digest as _, Sha256};

const PREPARATION_POLL_MILLIS: u64 = 250;

struct LeaseHeartbeat {
    checkpoint: SandboxPreparationAuthority,
    lease: Arc<tokio::sync::Mutex<SandboxPreparationLease>>,
    stopped: Arc<AtomicBool>,
    wake: Arc<tokio::sync::Notify>,
    failure: Arc<Mutex<Option<String>>>,
    task: tokio::task::JoinHandle<()>,
}

impl LeaseHeartbeat {
    fn start(checkpoint: SandboxPreparationAuthority, lease: SandboxPreparationLease) -> Self {
        let lease = Arc::new(tokio::sync::Mutex::new(lease));
        let stopped = Arc::new(AtomicBool::new(false));
        let wake = Arc::new(tokio::sync::Notify::new());
        let failure = Arc::new(Mutex::new(None));
        let task_checkpoint = checkpoint.clone();
        let task_lease = Arc::clone(&lease);
        let task_stopped = Arc::clone(&stopped);
        let task_wake = Arc::clone(&wake);
        let task_failure = Arc::clone(&failure);
        let task = tokio::spawn(async move {
            loop {
                if task_stopped.load(Ordering::Acquire) {
                    return;
                }
                tokio::select! {
                    () = task_wake.notified() => {
                        if task_stopped.load(Ordering::Acquire) {
                            return;
                        }
                    }
                    () = tokio::time::sleep(std::time::Duration::from_millis(
                        SANDBOX_PREPARATION_HEARTBEAT_MILLIS,
                    )) => {}
                }
                let mut lease = task_lease.lock().await;
                let session = lease.session;
                if task_checkpoint
                    .renew(session, &mut lease, now())
                    .await
                    .is_err()
                {
                    *task_failure
                        .lock()
                        .unwrap_or_else(|error| error.into_inner()) =
                        Some("sandbox materialization heartbeat lost its lease".to_owned());
                    return;
                }
            }
        });
        Self {
            checkpoint,
            lease,
            stopped,
            wake,
            failure,
            task,
        }
    }

    async fn pulse(&self) -> Result<(), String> {
        if let Some(error) = self
            .failure
            .lock()
            .unwrap_or_else(|failure| failure.into_inner())
            .clone()
        {
            return Err(error);
        }
        let mut lease = self.lease.lock().await;
        let session = lease.session;
        self.checkpoint
            .renew(session, &mut lease, now())
            .await
            .map_err(|_| "sandbox materialization lease was lost".to_owned())
    }

    async fn release(self) -> Result<(), String> {
        self.stopped.store(true, Ordering::Release);
        self.wake.notify_one();
        let _ = self.task.await;
        let lease = self.lease.lock().await.clone();
        self.checkpoint
            .release(lease.session, &lease)
            .await
            .map_err(|_| "sandbox materialization lease release failed".to_owned())
    }
}

/// Exact setup boundary used by the detached waiter registry.
pub trait SandboxPreparationPort: Send + Sync + 'static {
    /// Loads the elected create, admits a durable waiter, and materializes all exact files.
    fn prepare_for_tool<'a>(
        &'a self,
        workspace: WorkspaceId,
        session: SessionId,
        generation: GenerationId,
        call: &'a ToolCallIdentity,
    ) -> ToolMuxFuture<'a, Result<ReadyHand, String>>;

    /// Settles a previously prepared tool waiter.
    fn settle_tool<'a>(
        &'a self,
        ready: ReadyHand,
        call: &'a ToolCallIdentity,
    ) -> ToolMuxFuture<'a, Result<(), String>>;

    /// Emits terminal preparation failure for one detached first-call recovery.
    fn preparation_failed(&self, session: SessionId, generation: GenerationId, call: &str);
}

/// Production preparation over the create election, immutable content, and Hands generation.
pub struct ProductionSandboxPreparation {
    preparations: CreatePreparationStore,
    checkpoint: SandboxPreparationAuthority,
    content: Arc<dyn ContentObjectStore>,
    live: Arc<dyn aex_brain_hands::LiveFileBackend>,
    telemetry: TelemetryProducer,
}

impl ProductionSandboxPreparation {
    /// Binds the durable manifest and exact-byte authorities.
    #[must_use]
    pub const fn new(
        preparations: CreatePreparationStore,
        checkpoint: SandboxPreparationAuthority,
        content: Arc<dyn ContentObjectStore>,
        live: Arc<dyn aex_brain_hands::LiveFileBackend>,
        telemetry: TelemetryProducer,
    ) -> Self {
        Self {
            preparations,
            checkpoint,
            content,
            live,
            telemetry,
        }
    }

    fn progress(
        &self,
        session: SessionId,
        generation: GenerationId,
        call: Option<String>,
        progress: PreparationProgress,
    ) {
        self.telemetry.emit(TelemetryEvent {
            session,
            hand: Some(HandId::for_session(session)),
            generation: Some(generation),
            call,
            kind: TelemetryKind::SandboxProgress { progress },
        });
    }

    async fn load(
        &self,
        workspace: WorkspaceId,
        session: SessionId,
        generation: GenerationId,
    ) -> Result<aex_session_dynamodb::create_preparation::CreatePreparation, String> {
        let prepared = self
            .preparations
            .load_for_session(workspace, session, Some(generation))
            .await
            .map_err(|_| "sandbox create preparation is unavailable".to_owned())?
            .ok_or_else(|| "sandbox create preparation is absent".to_owned())?;
        if prepared.workspace != workspace
            || prepared.session != session
            || prepared.generation != Some(generation)
        {
            return Err("sandbox create preparation crossed its elected boundary".to_owned());
        }
        Ok(prepared)
    }

    async fn materialize(
        &self,
        prepared: &aex_session_dynamodb::create_preparation::CreatePreparation,
        expected_fence: Option<u64>,
        lease: &LeaseHeartbeat,
    ) -> Result<(), String> {
        let generation = prepared
            .generation
            .ok_or_else(|| "sandbox preparation has no generation".to_owned())?;
        for file in &prepared.files {
            lease.pulse().await?;
            let object = self
                .content
                .read_bounded(prepared.workspace, &file.content, file.size_bytes)
                .await
                .map_err(|_| "registered startup content is unavailable".to_owned())?;
            if object.declared_bytes != file.size_bytes
                || u64::try_from(object.body.len()).ok() != Some(file.size_bytes)
                || ContentHash::of(&object.body) != file.content
            {
                return Err("registered startup content failed exact verification".to_owned());
            }
            let upload = upload_id(generation, file);
            let batches = upload_batches(upload, file, &object.body)?;
            let expected = expected_state(upload, file, &object.body)?;
            let open = batches
                .first()
                .ok_or_else(|| "startup upload has no open batch".to_owned())?;
            lease.pulse().await?;
            let opened = self
                .live
                .call(prepared.session, generation, activity_id(upload, 0), open)
                .await
                .map_err(|_| "registered startup content transfer failed".to_owned())?;
            verify_generation(&opened, generation, expected_fence)?;
            let progress = verify_open(&expected, &opened.responses)?;
            if progress.complete {
                continue;
            }
            // UploadOpen also verifies the stored mode inside Hands. Its
            // returned exact manifest lets recovery skip every completed part.
            let first_pending = progress.completed_parts.saturating_add(1);
            for (ordinal, batch) in batches.iter().enumerate().skip(first_pending) {
                lease.pulse().await?;
                let request_count = batch.len();
                let reply = self
                    .live
                    .call(
                        prepared.session,
                        generation,
                        activity_id(upload, ordinal),
                        batch,
                    )
                    .await
                    .map_err(|_| "registered startup content transfer failed".to_owned())?;
                verify_generation(&reply, generation, expected_fence)?;
                verify_batch(&expected, ordinal, request_count, &reply.responses)?;
            }
        }
        Ok(())
    }

    async fn acquire_materializer(
        &self,
        prepared: &aex_session_dynamodb::create_preparation::CreatePreparation,
        generation: GenerationId,
    ) -> Result<Option<SandboxPreparationLease>, String> {
        let owner = uuid::Uuid::now_v7().to_string();
        loop {
            let status = self
                .checkpoint
                .observe(
                    prepared.workspace,
                    prepared.organization,
                    prepared.session,
                    generation,
                )
                .await
                .map_err(|_| "sandbox preparation checkpoint is unavailable".to_owned())?;
            if !should_materialize(status) {
                return Ok(None);
            }
            let observed = now();
            match self
                .checkpoint
                .claim(
                    prepared.workspace,
                    prepared.organization,
                    prepared.session,
                    generation,
                    &owner,
                    observed,
                )
                .await
                .map_err(|_| "sandbox materialization lease is unavailable".to_owned())?
            {
                SandboxPreparationClaim::Acquired(lease) => {
                    // A prior owner may have settled after its lease expired.
                    // Re-observe before the first guest write so completion is
                    // a hard no-rematerialization barrier.
                    let status = self
                        .checkpoint
                        .observe(
                            prepared.workspace,
                            prepared.organization,
                            prepared.session,
                            generation,
                        )
                        .await
                        .map_err(|_| "sandbox preparation checkpoint is unavailable".to_owned())?;
                    if should_materialize(status) {
                        return Ok(Some(lease));
                    }
                    let _ = self.checkpoint.release(prepared.session, &lease).await;
                    return Ok(None);
                }
                SandboxPreparationClaim::Contended { .. } => {
                    tokio::time::sleep(std::time::Duration::from_millis(PREPARATION_POLL_MILLIS))
                        .await;
                }
            }
        }
    }
}

impl SandboxPreparationPort for ProductionSandboxPreparation {
    fn prepare_for_tool<'a>(
        &'a self,
        workspace: WorkspaceId,
        session: SessionId,
        generation: GenerationId,
        call: &'a ToolCallIdentity,
    ) -> ToolMuxFuture<'a, Result<ReadyHand, String>> {
        Box::pin(async move {
            if call.workspace != workspace || call.session != session {
                return Err("tool waiter crossed its tenant boundary".to_owned());
            }
            // Load and verify the exact elected manifest before any provider effect.
            let prepared = self.load(workspace, session, generation).await?;
            let operation = operation_id(call);
            let correlation = Some(call.call.clone());
            let initial_status = self
                .checkpoint
                .observe(
                    prepared.workspace,
                    prepared.organization,
                    prepared.session,
                    generation,
                )
                .await
                .map_err(|_| "sandbox preparation checkpoint is unavailable".to_owned())?;
            if should_materialize(initial_status) {
                self.progress(
                    session,
                    generation,
                    correlation.clone(),
                    PreparationProgress::Provisioning,
                );
                self.progress(
                    session,
                    generation,
                    correlation.clone(),
                    PreparationProgress::Booting,
                );
            } else {
                self.progress(
                    session,
                    generation,
                    correlation.clone(),
                    PreparationProgress::Resuming,
                );
            }
            let fence = self
                .live
                .hold_tool_waiter(session, generation, operation)
                .await
                .map_err(|_| "exact-generation tool waiter admission failed".to_owned())?;
            let lease = match self.acquire_materializer(&prepared, generation).await {
                Ok(lease) => lease,
                Err(error) => {
                    let _ = self.live.settle_tool_waiter(generation, operation).await;
                    return Err(error);
                }
            };
            let mut lease =
                lease.map(|lease| LeaseHeartbeat::start(self.checkpoint.clone(), lease));
            if let Some(heartbeat) = lease.as_ref() {
                self.progress(
                    session,
                    generation,
                    correlation.clone(),
                    PreparationProgress::MaterializingWorkspace,
                );
                if let Err(error) = self.materialize(&prepared, Some(fence.0), heartbeat).await {
                    // Release stops the heartbeat before making the claim
                    // available to another recovery worker.
                    if let Some(lease) = lease.take() {
                        let _ = lease.release().await;
                    }
                    let _ = self.live.settle_tool_waiter(generation, operation).await;
                    return Err(error);
                }
            }
            let ready = match self.live.ensure_ready(session, generation).await {
                Ok(ready) => ready,
                Err(_) => {
                    if let Some(lease) = lease.take() {
                        let _ = lease.release().await;
                    }
                    let _ = self.live.settle_tool_waiter(generation, operation).await;
                    return Err("sandbox final readiness failed".to_owned());
                }
            };
            if ready.generation != generation || ready.lifecycle_fence != fence.0 {
                if let Some(lease) = lease.take() {
                    let _ = lease.release().await;
                }
                let _ = self.live.settle_tool_waiter(generation, operation).await;
                return Err("sandbox preparation crossed its generation fence".to_owned());
            }
            if let Some(heartbeat) = lease.as_ref()
                && heartbeat.pulse().await.is_err()
            {
                if let Some(lease) = lease.take() {
                    let _ = lease.release().await;
                }
                let _ = self.live.settle_tool_waiter(generation, operation).await;
                return Err("sandbox materialization lease was lost before checkpoint".to_owned());
            }
            if let Some(lease) = lease.take() {
                if self
                    .checkpoint
                    .settle(
                        prepared.workspace,
                        prepared.organization,
                        prepared.session,
                        generation,
                        false,
                        now(),
                    )
                    .await
                    .is_err()
                {
                    let _ = lease.release().await;
                    let _ = self.live.settle_tool_waiter(generation, operation).await;
                    return Err("sandbox ready checkpoint failed".to_owned());
                }
                lease.release().await?;
            }
            self.progress(session, generation, correlation, PreparationProgress::Ready);
            Ok(ReadyHand {
                hand: HandId::for_session(session),
                generation,
                fence,
            })
        })
    }

    fn settle_tool<'a>(
        &'a self,
        ready: ReadyHand,
        call: &'a ToolCallIdentity,
    ) -> ToolMuxFuture<'a, Result<(), String>> {
        Box::pin(async move {
            if ready.hand != HandId::for_session(call.session) {
                return Err("tool waiter settlement names a foreign Hand".to_owned());
            }
            self.live
                .settle_tool_waiter(ready.generation, operation_id(call))
                .await
                .map_err(|_| "exact-generation tool waiter settlement failed".to_owned())
        })
    }

    fn preparation_failed(&self, session: SessionId, generation: GenerationId, call: &str) {
        self.progress(
            session,
            generation,
            Some(call.to_owned()),
            PreparationProgress::Failed,
        );
    }
}

fn upload_batches(
    upload: FileUploadId,
    file: &PreparedFile,
    bytes: &[u8],
) -> Result<Vec<Vec<FileRequest>>, String> {
    let root = GuestRoot::workspace();
    let path = if file.mount_path.as_str() == "/" {
        root.0.clone()
    } else {
        format!("{}{}", root.0, file.mount_path.as_str())
    };
    let path = GuestPath::parse(&root, &path)
        .map_err(|_| "registered startup mount path is invalid".to_owned())?;
    let mode = match file.mode {
        aex_wire::models::RegisteredFileMode::V0644 => FileMode::ReadWrite,
        aex_wire::models::RegisteredFileMode::V0755 => FileMode::Executable,
    };
    let mut batches = vec![vec![FileRequest::UploadOpen {
        upload,
        path,
        size_bytes: file.size_bytes,
        sha256: file.content,
        mode,
    }]];
    for (part_index, part) in bytes.chunks(FILE_TRANSFER_PART_BYTES as usize).enumerate() {
        let part_number = u32::try_from(part_index + 1).unwrap_or(u32::MAX);
        let offset = u64::try_from(part_index)
            .unwrap_or(u64::MAX)
            .saturating_mul(u64::from(FILE_TRANSFER_PART_BYTES));
        let mut batch = vec![FileRequest::UploadPartOpen {
            upload,
            part_number,
            offset,
            size_bytes: u32::try_from(part.len()).unwrap_or(u32::MAX),
            sha256: ContentHash::of(part),
        }];
        for (chunk_index, chunk) in part.chunks(FILE_FRAME_BYTES as usize).enumerate() {
            batch.push(FileRequest::UploadPartChunk {
                upload,
                part_number,
                chunk_offset: u32::try_from(chunk_index.saturating_mul(FILE_FRAME_BYTES as usize))
                    .unwrap_or(u32::MAX),
                sha256: ContentHash::of(chunk),
                bytes: chunk.to_vec(),
            });
        }
        batch.push(FileRequest::UploadPartComplete {
            upload,
            part_number,
        });
        batches.push(batch);
    }
    batches.push(vec![FileRequest::UploadComplete { upload }]);
    Ok(batches)
}

fn expected_state(
    upload: FileUploadId,
    file: &PreparedFile,
    bytes: &[u8],
) -> Result<FileUploadState, String> {
    let root = GuestRoot::workspace();
    let path = if file.mount_path.as_str() == "/" {
        root.0.clone()
    } else {
        format!("{}{}", root.0, file.mount_path.as_str())
    };
    let path = GuestPath::parse(&root, &path)
        .map_err(|_| "registered startup mount path is invalid".to_owned())?;
    Ok(FileUploadState {
        upload,
        path,
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

fn verify_batch(
    expected: &FileUploadState,
    ordinal: usize,
    request_count: usize,
    responses: &[FileResponse],
) -> Result<(), String> {
    if responses.len() != request_count {
        return Err("startup guest returned an incomplete response batch".to_owned());
    }
    for response in responses {
        if matches!(response, FileResponse::Rejected { .. }) {
            return Err("startup guest rejected an exact file transfer".to_owned());
        }
    }
    let final_batch = ordinal == part_count(expected.size_bytes) + 1;
    let state = match responses.last() {
        Some(FileResponse::Upload { state }) if !final_batch => state,
        Some(FileResponse::UploadComplete { state }) if final_batch => state,
        _ => return Err("startup guest returned mismatched file transfer state".to_owned()),
    };
    if state.upload != expected.upload
        || state.path != expected.path
        || state.size_bytes != expected.size_bytes
        || state.sha256 != expected.sha256
        || state.complete != final_batch
        || state.parts != expected.parts[..ordinal.min(expected.parts.len())]
    {
        return Err("startup guest returned mismatched file transfer state".to_owned());
    }
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct UploadProgress {
    completed_parts: usize,
    complete: bool,
}

fn verify_open(
    expected: &FileUploadState,
    responses: &[FileResponse],
) -> Result<UploadProgress, String> {
    let [FileResponse::Upload { state }] = responses else {
        return Err("startup guest returned mismatched upload-open state".to_owned());
    };
    if state.upload != expected.upload
        || state.path != expected.path
        || state.size_bytes != expected.size_bytes
        || state.sha256 != expected.sha256
        || state.parts.len() > expected.parts.len()
        || state.parts != expected.parts[..state.parts.len()]
        || (state.complete && state.parts != expected.parts)
    {
        return Err("startup guest returned mismatched upload-open state".to_owned());
    }
    Ok(UploadProgress {
        completed_parts: state.parts.len(),
        complete: state.complete,
    })
}

fn verify_generation(
    reply: &aex_brain_hands::LiveFileReply,
    generation: GenerationId,
    expected_fence: Option<u64>,
) -> Result<(), String> {
    if reply.generation != generation
        || expected_fence.is_some_and(|fence| reply.lifecycle_fence != fence)
    {
        return Err("registered startup content reached a foreign generation".to_owned());
    }
    Ok(())
}

fn part_count(size: u64) -> usize {
    usize::try_from(size.div_ceil(u64::from(FILE_TRANSFER_PART_BYTES))).unwrap_or(usize::MAX)
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

pub(crate) fn operation_id(call: &ToolCallIdentity) -> HandsOperationId {
    let digest = Sha256::digest(serde_json::to_vec(call).unwrap_or_default());
    let mut entropy = [0_u8; 10];
    entropy.copy_from_slice(&digest[..10]);
    HandsOperationId(Uuid7::compose(call.session.uuid7().unix_millis(), entropy))
}

fn now() -> aex_wire::types::Timestamp {
    let millis = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .and_then(|duration| i64::try_from(duration.as_millis()).ok())
        .unwrap_or(0);
    aex_wire::types::Timestamp::from_unix_millis(millis).unwrap_or_else(|_| {
        aex_wire::types::Timestamp::from_unix_millis(0)
            .unwrap_or_else(|_| unreachable!("the unix epoch is a valid timestamp"))
    })
}

const fn should_materialize(status: aex_session_domain::SandboxPreparationStatus) -> bool {
    matches!(
        status,
        aex_session_domain::SandboxPreparationStatus::Requested
    )
}

#[cfg(test)]
mod tests {
    use super::{should_materialize, verify_open};
    use aex_hands_protocol::files::{FilePartReceipt, FileResponse, FileUploadId, FileUploadState};
    use aex_hands_protocol::operation::{GuestPath, GuestRoot};
    use aex_session_domain::SandboxPreparationStatus;
    use aex_wire::ids::{ContentHash, Uuid7};

    fn expected_upload() -> FileUploadState {
        let bytes = b"already materialized";
        FileUploadState {
            upload: FileUploadId(Uuid7::compose(1, [1; 10])),
            path: GuestPath::parse(&GuestRoot::workspace(), "/workspace/input.txt").expect("path"),
            size_bytes: bytes.len() as u64,
            sha256: ContentHash::of(bytes),
            parts: vec![FilePartReceipt {
                part_number: 1,
                offset: 0,
                size_bytes: bytes.len() as u32,
                sha256: ContentHash::of(bytes),
            }],
            complete: false,
        }
    }

    #[test]
    fn completed_preparation_never_rematerializes_startup_files() {
        assert!(should_materialize(SandboxPreparationStatus::Requested));
        assert!(!should_materialize(SandboxPreparationStatus::Ready));
        assert!(!should_materialize(SandboxPreparationStatus::Suspended));
    }

    #[test]
    fn an_exact_completed_upload_is_verified_and_skipped() {
        let expected = expected_upload();
        let mut completed = expected.clone();
        completed.complete = true;
        let progress = verify_open(&expected, &[FileResponse::Upload { state: completed }])
            .expect("exact completion is recoverable");
        assert!(progress.complete);
        assert_eq!(progress.completed_parts, expected.parts.len());
    }

    #[test]
    fn a_completed_upload_with_a_foreign_manifest_is_refused() {
        let expected = expected_upload();
        let mut foreign = expected.clone();
        foreign.complete = true;
        foreign.sha256 = ContentHash::of(b"foreign");
        assert!(verify_open(&expected, &[FileResponse::Upload { state: foreign }]).is_err());
    }
}
