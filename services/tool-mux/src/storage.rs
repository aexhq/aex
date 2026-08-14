//! Trusted `storage.persist` composition over exact-generation guest streaming
//! and the latest-only workspace-file authority.

use std::sync::Arc;

use aex_hands_protocol::operation::GuestPath;
use aex_tool_mux::{
    ExecutorOutput, FullOutput, ReadyHand, StoragePersistPort, ToolCallIdentity, ToolMuxFuture,
};
use aex_wire::ids::{ContentHash, ResourceName, WorkspaceId};
use base64::Engine as _;

const FILE_PREVIEW_BYTES: usize = 4 * 1024;
const DEFAULT_MEDIA_TYPE: &str = "application/octet-stream";

/// Exact evidence returned by the trusted guest-to-authority stream.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifiedGuestFile {
    /// Runtime identity that served every byte.
    pub ready: ReadyHand,
    /// Exact source path in that runtime.
    pub source: GuestPath,
    /// Opaque private staging identity, never a bearer URL.
    pub staging_ref: String,
    /// Verified complete digest.
    pub hash: ContentHash,
    /// Verified complete length.
    pub bytes: u64,
    /// First bounded bytes captured while streaming.
    pub preview: Vec<u8>,
}

/// Exact-generation sandbox file streaming boundary.
pub trait GuestFileStreamPort: Send + Sync + 'static {
    /// Streams through a single-purpose authority grant and verifies length and
    /// hash. The guest never receives AWS credentials or commit authority.
    fn stream_verified<'a>(
        &'a self,
        ready: ReadyHand,
        source: &'a GuestPath,
        call: &'a ToolCallIdentity,
    ) -> ToolMuxFuture<'a, Result<VerifiedGuestFile, String>>;
}

impl<T> GuestFileStreamPort for std::sync::Arc<T>
where
    T: GuestFileStreamPort + ?Sized,
{
    fn stream_verified<'a>(
        &'a self,
        ready: ReadyHand,
        source: &'a GuestPath,
        call: &'a ToolCallIdentity,
    ) -> ToolMuxFuture<'a, Result<VerifiedGuestFile, String>> {
        (**self).stream_verified(ready, source, call)
    }
}

/// Durable argument identity checked before a retry streams bytes again.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PersistIntent {
    /// Authorized workspace.
    pub workspace: WorkspaceId,
    /// Durable replay identity.
    pub call: ToolCallIdentity,
    /// Exact runtime generation and fence.
    pub ready: ReadyHand,
    /// Exact guest source path.
    pub source: GuestPath,
    /// Validated current-value name.
    pub name: ResourceName,
    /// Declared media type.
    pub media_type: String,
}

/// Durable latest-only commit command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PersistLatest {
    /// Arguments fenced before streaming.
    pub intent: PersistIntent,
    /// Opaque verified staging object.
    pub staging_ref: String,
    /// Expected complete hash.
    pub hash: ContentHash,
    /// Expected complete length.
    pub bytes: u64,
    /// Bounded preview retained with the durable receipt.
    pub preview: Vec<u8>,
}

/// Durable authority commit/replay evidence.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PersistedLatest {
    /// Authorized workspace.
    pub workspace: WorkspaceId,
    /// Current logical name overwritten by the call.
    pub name: ResourceName,
    /// Committed hash.
    pub hash: ContentHash,
    /// Committed length.
    pub bytes: u64,
    /// Committed media type.
    pub media_type: String,
    /// Stable authenticated API path for a later download grant.
    pub download_path: Option<String>,
    /// Whether this was an exact replay.
    pub replayed: bool,
    /// Bounded preview retained with the durable replay result.
    pub preview: Vec<u8>,
}

/// Latest-only file-authority refusal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PersistAuthorityError {
    /// A newer intent displaced this commit before publication.
    StaleIntent,
    /// A call identity was reused with different arguments.
    IdempotencyConflict,
    /// Redacted authority/storage failure.
    Failed(String),
}

/// Latest-only workspace-file authority boundary.
pub trait LatestFileAuthorityPort: Send + Sync + 'static {
    /// Returns an exact durable replay before another guest stream begins.
    fn load_replay<'a>(
        &'a self,
        intent: &'a PersistIntent,
    ) -> ToolMuxFuture<'a, Result<Option<PersistedLatest>, PersistAuthorityError>>;

    /// Atomically commits or replays a verified current-value overwrite.
    fn persist_latest<'a>(
        &'a self,
        command: PersistLatest,
    ) -> ToolMuxFuture<'a, Result<PersistedLatest, PersistAuthorityError>>;
}

/// One trusted persistence execution before detached scheduling.
pub trait StorageOperationPort: Send + Sync + 'static {
    /// Streams, verifies and commits one exact latest-only value.
    fn persist<'a>(
        &'a self,
        ready: ReadyHand,
        source: &'a GuestPath,
        logical_name: &'a str,
        media_type: Option<&'a str>,
        call: &'a ToolCallIdentity,
    ) -> ToolMuxFuture<'a, Result<ExecutorOutput, String>>;
}

/// Trusted synchronous operation assembled behind the detached adapter.
pub struct StorageAdapter<G, A> {
    guest: G,
    authority: A,
}

impl<G, A> StorageAdapter<G, A> {
    /// Composes the two trusted narrow ports.
    #[must_use]
    pub const fn new(guest: G, authority: A) -> Self {
        Self { guest, authority }
    }
}

impl<G, A> StorageOperationPort for StorageAdapter<G, A>
where
    G: GuestFileStreamPort,
    A: LatestFileAuthorityPort,
{
    fn persist<'a>(
        &'a self,
        ready: ReadyHand,
        source: &'a GuestPath,
        logical_name: &'a str,
        media_type: Option<&'a str>,
        call: &'a ToolCallIdentity,
    ) -> ToolMuxFuture<'a, Result<ExecutorOutput, String>> {
        Box::pin(async move {
            if ready.hand.session() != call.session {
                return Err("storage.persist refused a foreign session Hand".to_owned());
            }
            let name = ResourceName::parse(logical_name)
                .map_err(|error| format!("storage.persist logical name is invalid: {error}"))?;
            let media_type = validate_media_type(media_type.unwrap_or(DEFAULT_MEDIA_TYPE))?;
            let intent = PersistIntent {
                workspace: call.workspace,
                call: call.clone(),
                ready,
                source: source.clone(),
                name: name.clone(),
                media_type: media_type.clone(),
            };
            if let Some(replay) = self
                .authority
                .load_replay(&intent)
                .await
                .map_err(authority_error)?
            {
                return render_result(replay, call.workspace, &name, &media_type, source);
            }
            let streamed = self.guest.stream_verified(ready, source, call).await?;
            if streamed.ready != ready || streamed.source != *source {
                return Err(
                    "storage.persist refused stale or foreign exact-generation evidence".to_owned(),
                );
            }
            if streamed.preview.len() > FILE_PREVIEW_BYTES {
                return Err("storage.persist bridge exceeded the verified preview bound".to_owned());
            }

            let persisted = self
                .authority
                .persist_latest(PersistLatest {
                    intent,
                    staging_ref: streamed.staging_ref,
                    hash: streamed.hash,
                    bytes: streamed.bytes,
                    preview: streamed.preview,
                })
                .await
                .map_err(authority_error)?;
            if persisted.hash != streamed.hash || persisted.bytes != streamed.bytes {
                return Err(
                    "storage.persist authority returned mismatched commit evidence".to_owned(),
                );
            }
            render_result(persisted, call.workspace, &name, &media_type, source)
        })
    }
}

/// Fast detached facade passed to [`aex_tool_mux::ToolMux`].
pub struct DetachedStorageAdapter {
    operation: Arc<dyn StorageOperationPort>,
    executions: crate::detached::DetachedExecutions,
}

impl DetachedStorageAdapter {
    /// Binds the exact trusted persistence operation.
    #[must_use]
    pub fn new(operation: Arc<dyn StorageOperationPort>) -> Self {
        Self {
            operation,
            executions: crate::detached::DetachedExecutions::default(),
        }
    }
}

impl StoragePersistPort for DetachedStorageAdapter {
    fn start_persist<'a>(
        &'a self,
        ready: ReadyHand,
        source: &'a GuestPath,
        logical_name: &'a str,
        media_type: Option<&'a str>,
        call: &'a ToolCallIdentity,
    ) -> ToolMuxFuture<'a, Result<(), String>> {
        Box::pin(async move {
            let operation = crate::detached::operation_id(call, "storage-persist");
            let executor = Arc::clone(&self.operation);
            let source = source.clone();
            let logical_name = logical_name.to_owned();
            let media_type = media_type.map(str::to_owned);
            let call = call.clone();
            self.executions.start(operation, async move {
                executor
                    .persist(ready, &source, &logical_name, media_type.as_deref(), &call)
                    .await
            });
            Ok(())
        })
    }

    fn read_persist<'a>(
        &'a self,
        ready: ReadyHand,
        call: &'a ToolCallIdentity,
    ) -> ToolMuxFuture<'a, Result<Option<ExecutorOutput>, String>> {
        Box::pin(async move {
            if ready.hand.session() != call.session {
                return Err("storage.persist received a foreign ready Hand".to_owned());
            }
            self.executions
                .read(crate::detached::operation_id(call, "storage-persist"))
        })
    }

    fn cancel_persist<'a>(
        &'a self,
        ready: ReadyHand,
        call: &'a ToolCallIdentity,
    ) -> ToolMuxFuture<'a, Result<(), String>> {
        Box::pin(async move {
            if ready.hand.session() != call.session {
                return Err("storage.persist received a foreign ready Hand".to_owned());
            }
            self.executions
                .cancel(crate::detached::operation_id(call, "storage-persist"));
            Ok(())
        })
    }
}

fn render_result(
    persisted: PersistedLatest,
    workspace: WorkspaceId,
    name: &ResourceName,
    media_type: &str,
    source: &GuestPath,
) -> Result<ExecutorOutput, String> {
    if persisted.workspace != workspace
        || persisted.name != *name
        || persisted.media_type != media_type
        || persisted.preview.len() > FILE_PREVIEW_BYTES
        || persisted
            .download_path
            .as_ref()
            .is_some_and(|path| path.is_empty() || path.len() > 512)
    {
        return Err("storage.persist authority returned mismatched commit evidence".to_owned());
    }
    let (preview, encoding) = encode_preview(&persisted.preview);
    let mut value = serde_json::json!({
        "name": persisted.name.as_str(),
        "sha256": persisted.hash.to_string(),
        "sizeBytes": persisted.bytes.to_string(),
        "mediaType": persisted.media_type,
        "preview": preview,
        "previewEncoding": encoding,
        "previewBytes": persisted.preview.len(),
        "truncated": persisted.bytes > persisted.preview.len() as u64,
        "fullResultPath": source.as_str(),
    });
    if let Some(download_path) = persisted.download_path {
        value["downloadPath"] = serde_json::Value::String(download_path);
    }
    let body = serde_json::to_vec(&value)
        .map_err(|error| format!("storage.persist could not encode its result: {error}"))?;
    Ok(ExecutorOutput {
        preview: body.clone(),
        full: FullOutput::Inline(body),
        is_error: false,
    })
}

fn validate_media_type(value: &str) -> Result<String, String> {
    if value.is_empty() || value.len() > 255 || value.bytes().any(|byte| byte.is_ascii_control()) {
        return Err("storage.persist media type must be 1..255 visible bytes".to_owned());
    }
    Ok(value.to_owned())
}

fn authority_error(error: PersistAuthorityError) -> String {
    match error {
        PersistAuthorityError::StaleIntent => {
            "storage.persist was displaced by a newer overwrite".to_owned()
        }
        PersistAuthorityError::IdempotencyConflict => {
            "storage.persist call identity was reused with different arguments".to_owned()
        }
        PersistAuthorityError::Failed(detail) => {
            format!("storage.persist authority failed: {detail}")
        }
    }
}

fn encode_preview(bytes: &[u8]) -> (String, &'static str) {
    match core::str::from_utf8(bytes) {
        Ok(text) => (text.to_owned(), "utf8"),
        Err(_) => (
            base64::engine::general_purpose::STANDARD.encode(bytes),
            "base64",
        ),
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::sync::Mutex;

    use aex_hands_protocol::operation::GuestRoot;
    use aex_hands_protocol::rpc::Fence;
    use aex_runtime_control::HandId;
    use aex_wire::ids::{AgentId, GenerationId, MessageId, PrefixedId, SessionId, Uuid7};

    use super::*;

    fn id<T: PrefixedId>(seed: u8) -> T {
        T::from_uuid7(Uuid7::compose(u64::from(seed), [seed; 10]))
    }

    fn identity(seed: u8) -> ToolCallIdentity {
        ToolCallIdentity {
            organization: id(0),
            workspace: id(1),
            session: id(2),
            agent: id::<AgentId>(3),
            message: id::<MessageId>(4),
            batch: 5,
            call: format!("call-{seed}"),
            attempt: 1,
        }
    }

    fn ready(generation: u8) -> ReadyHand {
        ReadyHand {
            hand: HandId::for_session(id::<SessionId>(2)),
            generation: id::<GenerationId>(generation),
            fence: Fence(8),
        }
    }

    fn source() -> GuestPath {
        GuestPath::parse(&GuestRoot::workspace(), "/workspace/output.bin").expect("path")
    }

    struct GuestFake {
        bytes: Vec<u8>,
        returned_ready: Mutex<Option<ReadyHand>>,
        streams: Mutex<u32>,
    }

    impl GuestFake {
        fn new(bytes: &[u8]) -> Self {
            Self {
                bytes: bytes.to_vec(),
                returned_ready: Mutex::new(None),
                streams: Mutex::new(0),
            }
        }
    }

    impl GuestFileStreamPort for GuestFake {
        fn stream_verified<'a>(
            &'a self,
            requested: ReadyHand,
            source: &'a GuestPath,
            _call: &'a ToolCallIdentity,
        ) -> ToolMuxFuture<'a, Result<VerifiedGuestFile, String>> {
            *self.streams.lock().expect("streams") += 1;
            let evidence = self
                .returned_ready
                .lock()
                .expect("ready")
                .unwrap_or(requested);
            let bytes = self.bytes.clone();
            let source = source.clone();
            Box::pin(async move {
                Ok(VerifiedGuestFile {
                    ready: evidence,
                    source,
                    staging_ref: "private/staging/one".to_owned(),
                    hash: ContentHash::of(&bytes),
                    bytes: bytes.len() as u64,
                    preview: bytes[..bytes.len().min(FILE_PREVIEW_BYTES)].to_vec(),
                })
            })
        }
    }

    #[derive(Default)]
    struct AuthorityFake {
        receipts: Mutex<BTreeMap<String, (PersistLatest, PersistedLatest)>>,
        latest: Mutex<BTreeMap<String, ContentHash>>,
        commits: Mutex<u32>,
        stale: Mutex<bool>,
        mismatch: Mutex<bool>,
    }

    impl LatestFileAuthorityPort for AuthorityFake {
        fn load_replay<'a>(
            &'a self,
            intent: &'a PersistIntent,
        ) -> ToolMuxFuture<'a, Result<Option<PersistedLatest>, PersistAuthorityError>> {
            Box::pin(async move {
                let key = intent.call.call.clone();
                let receipts = self.receipts.lock().expect("receipts");
                let Some((stored, answer)) = receipts.get(&key) else {
                    return Ok(None);
                };
                if &stored.intent != intent {
                    return Err(PersistAuthorityError::IdempotencyConflict);
                }
                let mut replay = answer.clone();
                replay.replayed = true;
                Ok(Some(replay))
            })
        }

        fn persist_latest<'a>(
            &'a self,
            command: PersistLatest,
        ) -> ToolMuxFuture<'a, Result<PersistedLatest, PersistAuthorityError>> {
            Box::pin(async move {
                if *self.stale.lock().expect("stale") {
                    return Err(PersistAuthorityError::StaleIntent);
                }
                let key = command.intent.call.call.clone();
                if let Some((stored, answer)) = self.receipts.lock().expect("receipts").get(&key) {
                    if stored != &command {
                        return Err(PersistAuthorityError::IdempotencyConflict);
                    }
                    let mut replay = answer.clone();
                    replay.replayed = true;
                    return Ok(replay);
                }
                *self.commits.lock().expect("commits") += 1;
                self.latest
                    .lock()
                    .expect("latest")
                    .insert(command.intent.name.as_str().to_owned(), command.hash);
                let mut answer = PersistedLatest {
                    workspace: command.intent.workspace,
                    name: command.intent.name.clone(),
                    hash: command.hash,
                    bytes: command.bytes,
                    media_type: command.intent.media_type.clone(),
                    download_path: Some(format!(
                        "/api/files/{}/downloads",
                        command.intent.name.as_str()
                    )),
                    replayed: false,
                    preview: command.preview.clone(),
                };
                if *self.mismatch.lock().expect("mismatch") {
                    answer.bytes = answer.bytes.saturating_add(1);
                }
                self.receipts
                    .lock()
                    .expect("receipts")
                    .insert(key, (command, answer.clone()));
                Ok(answer)
            })
        }
    }

    #[tokio::test]
    async fn verified_binary_commit_is_exact_and_idempotent() {
        let bytes = [0, 255, 1, 254];
        let adapter = StorageAdapter::new(GuestFake::new(&bytes), AuthorityFake::default());
        let call = identity(1);
        let first = adapter
            .persist(ready(7), &source(), "asset.bin", None, &call)
            .await
            .expect("persist");
        let second = adapter
            .persist(ready(7), &source(), "asset.bin", None, &call)
            .await
            .expect("replay");
        assert_eq!(first, second);
        assert_eq!(*adapter.authority.commits.lock().expect("commits"), 1);
        assert_eq!(*adapter.guest.streams.lock().expect("streams"), 1);
        assert_eq!(
            adapter.authority.latest.lock().expect("latest")["asset.bin"],
            ContentHash::of(&bytes)
        );
        let FullOutput::Inline(body) = first.full else {
            panic!("small structured result")
        };
        let value: serde_json::Value = serde_json::from_slice(&body).expect("json");
        assert_eq!(value["sizeBytes"], "4");
        assert_eq!(value["previewEncoding"], "base64");
    }

    #[tokio::test]
    async fn later_call_overwrites_only_the_latest_name() {
        let first = StorageAdapter::new(GuestFake::new(b"one"), AuthorityFake::default());
        first
            .persist(
                ready(7),
                &source(),
                "result",
                Some("text/plain"),
                &identity(1),
            )
            .await
            .expect("first");
        let second = StorageAdapter::new(GuestFake::new(b"two"), first.authority);
        second
            .persist(
                ready(7),
                &source(),
                "result",
                Some("text/plain"),
                &identity(2),
            )
            .await
            .expect("second");
        assert_eq!(
            second.authority.latest.lock().expect("latest")["result"],
            ContentHash::of(b"two")
        );
        assert_eq!(second.authority.latest.lock().expect("latest").len(), 1);
    }

    #[tokio::test]
    async fn stale_or_foreign_generation_is_refused() {
        let guest = GuestFake::new(b"body");
        *guest.returned_ready.lock().expect("ready") = Some(ready(99));
        let foreign = StorageAdapter::new(guest, AuthorityFake::default());
        assert!(
            foreign
                .persist(ready(7), &source(), "result", None, &identity(1))
                .await
                .expect_err("foreign generation")
                .contains("stale or foreign")
        );
        assert_eq!(*foreign.authority.commits.lock().expect("commits"), 0);

        let stale = StorageAdapter::new(GuestFake::new(b"body"), AuthorityFake::default());
        *stale.authority.stale.lock().expect("stale") = true;
        assert!(
            stale
                .persist(ready(7), &source(), "result", None, &identity(1))
                .await
                .expect_err("stale intent")
                .contains("newer overwrite")
        );
    }

    #[tokio::test]
    async fn authority_hash_or_length_mismatch_fails_closed() {
        let adapter = StorageAdapter::new(GuestFake::new(b"body"), AuthorityFake::default());
        *adapter.authority.mismatch.lock().expect("mismatch") = true;
        assert!(
            adapter
                .persist(ready(7), &source(), "result", None, &identity(1))
                .await
                .expect_err("mismatch")
                .contains("mismatched commit evidence")
        );
    }
}
