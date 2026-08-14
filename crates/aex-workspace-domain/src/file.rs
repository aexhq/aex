//! Latest-only workspace-file state and storage persistence.
//!
//! Public identity is `(workspace, name)`. The counters in this module are
//! private publication fences: they make asynchronous URL/import and upload
//! completion safe, but are never file versions and never cross the customer
//! boundary. Immutable content identities remain available to sessions that
//! captured them before a later overwrite.

use std::collections::{BTreeMap, BTreeSet};

use aex_content_domain::{ContentDigest, NormalizedPath};
use aex_wire::ids::{ResourceName, SessionId, ToolCallId, WorkspaceId};
use aex_wire::types::Timestamp;

/// Private monotone overwrite intent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct FileIntent(u64);

impl FileIntent {
    /// Constructs a private durable authority counter.
    ///
    /// The authority must never reuse a value for the same `(workspace,name)`,
    /// including after delete and recreate. It is deliberately absent from all
    /// public file schemas.
    ///
    /// # Panics
    ///
    /// If `value` is zero: the authority counter starts at one.
    #[must_use]
    pub const fn from_private_counter(value: u64) -> Self {
        assert!(value != 0, "file intent counter starts at one");
        Self(value)
    }
}

/// How the current overwrite was admitted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileSource {
    /// Request-carried bytes.
    Inline,
    /// Bounded server-side HTTPS import.
    Url,
    /// Direct single or multipart upload.
    Upload,
    /// The trusted `storage.persist` tool.
    StorageTool,
}

/// Verified immutable content behind one ready current value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReadyFile {
    /// Exact plaintext digest.
    pub digest: ContentDigest,
    /// Exact plaintext length.
    pub size_bytes: u64,
    /// Declared media type.
    pub media_type: String,
    /// Private encrypted object-store key.
    pub object_key: String,
}

/// The public lifecycle of the current value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FileState {
    /// Accepted asynchronous import/upload whose bytes are not yet publishable.
    Pending,
    /// Verified bytes are available.
    Ready(ReadyFile),
    /// The current asynchronous overwrite failed with a stable bounded code.
    Failed {
        /// Stable bounded failure code; no fetched body or secret-bearing detail.
        code: String,
    },
}

/// Exactly one current workspace-file record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceFile {
    /// Owning workspace.
    pub workspace: WorkspaceId,
    /// Stable public logical name.
    pub name: ResourceName,
    /// How the current overwrite entered the authority.
    pub source: FileSource,
    /// Current lifecycle state.
    pub state: FileState,
    /// When this current overwrite was admitted.
    pub updated_at: Timestamp,
    /// Private stale-completion fence.
    intent: FileIntent,
}

impl WorkspaceFile {
    /// Private intent passed only to the asynchronous worker/commit path.
    #[must_use]
    pub const fn intent(&self) -> FileIntent {
        self.intent
    }

    /// Ready bytes, when this current overwrite has published them.
    #[must_use]
    pub const fn ready(&self) -> Option<&ReadyFile> {
        match &self.state {
            FileState::Ready(ready) => Some(ready),
            FileState::Pending | FileState::Failed { .. } => None,
        }
    }
}

/// A new overwrite admitted before asynchronous work begins.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileAdmission {
    /// Current row written immediately.
    pub current: WorkspaceFile,
    /// Prior immutable object, potentially collectible after references drain.
    pub displaced: Option<ReadyFile>,
}

/// Admits an inline value atomically as ready.
#[must_use]
pub fn admit_inline(
    current: Option<&WorkspaceFile>,
    workspace: WorkspaceId,
    name: ResourceName,
    ready: ReadyFile,
    now: Timestamp,
    intent: FileIntent,
) -> FileAdmission {
    admit(
        current,
        workspace,
        name,
        FileSource::Inline,
        FileState::Ready(ready),
        now,
        intent,
    )
}

/// Admits URL or direct-upload work immediately as pending.
///
/// The old current value is displaced at admission, not completion. New reads
/// and new sessions therefore cannot fall back to stale bytes while work runs.
///
/// # Panics
///
/// If `source` is [`FileSource::Inline`]: only asynchronous sources may enter
/// pending.
#[must_use]
pub fn admit_pending(
    current: Option<&WorkspaceFile>,
    workspace: WorkspaceId,
    name: ResourceName,
    source: FileSource,
    now: Timestamp,
    intent: FileIntent,
) -> FileAdmission {
    assert!(
        matches!(
            source,
            FileSource::Url | FileSource::Upload | FileSource::StorageTool
        ),
        "only asynchronous sources may enter pending"
    );
    admit(
        current,
        workspace,
        name,
        source,
        FileState::Pending,
        now,
        intent,
    )
}

fn admit(
    current: Option<&WorkspaceFile>,
    workspace: WorkspaceId,
    name: ResourceName,
    source: FileSource,
    state: FileState,
    now: Timestamp,
    intent: FileIntent,
) -> FileAdmission {
    assert!(
        current.is_none_or(|file| file.intent != intent),
        "a new overwrite must allocate a fresh private intent"
    );
    let displaced = current.and_then(WorkspaceFile::ready).cloned();
    FileAdmission {
        current: WorkspaceFile {
            workspace,
            name,
            source,
            state,
            updated_at: now,
            intent,
        },
        displaced,
    }
}

/// Result of an exact-intent asynchronous completion.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FilePublish {
    /// This intent was still current and published its terminal state.
    Published(WorkspaceFile),
    /// A newer overwrite won; the completion must not mutate the current row.
    Stale {
        /// Verified object produced by stale work and eligible for reference-aware cleanup.
        orphan: Option<ReadyFile>,
    },
}

/// Publishes verified bytes only when `intent` is still the current overwrite.
#[must_use]
pub fn publish_ready(
    current: &WorkspaceFile,
    intent: FileIntent,
    ready: ReadyFile,
    now: Timestamp,
) -> FilePublish {
    if current.intent != intent || current.state != FileState::Pending {
        return FilePublish::Stale {
            orphan: Some(ready),
        };
    }
    let mut published = current.clone();
    published.state = FileState::Ready(ready);
    published.updated_at = now;
    FilePublish::Published(published)
}

/// Publishes a bounded failure only when `intent` is still current.
#[must_use]
pub fn publish_failed(
    current: &WorkspaceFile,
    intent: FileIntent,
    code: impl Into<String>,
    now: Timestamp,
) -> FilePublish {
    if current.intent != intent || current.state != FileState::Pending {
        return FilePublish::Stale { orphan: None };
    }
    let mut published = current.clone();
    published.state = FileState::Failed { code: code.into() };
    published.updated_at = now;
    FilePublish::Published(published)
}

/// One name-to-path selection at session admission.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileMount {
    /// Current logical file name.
    pub name: ResourceName,
    /// Normalized path relative to `/workspace`.
    pub path: NormalizedPath,
}

/// Private immutable manifest row captured for one admitted session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FrozenFile {
    /// Logical name used at admission.
    pub name: ResourceName,
    /// Exact destination path.
    pub path: NormalizedPath,
    /// Exact object/hash identity, unaffected by later file overwrites.
    pub content: ReadyFile,
}

/// Why session file selection was refused.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum FreezeError {
    /// The requested name had no current row.
    #[error("workspace file `{0}` was not found")]
    NotFound(String),
    /// The requested current row was not ready.
    #[error("workspace file `{0}` is not ready")]
    NotReady(String),
    /// Two selections map to the same path, case-insensitively.
    #[error("workspace mount path collides with `{0}`")]
    PathCollision(String),
}

/// Resolves latest names once and freezes exact immutable content identities.
///
/// Later overwrites cannot change the returned manifest. This is the internal
/// pin required for session correctness, not a public file-version selector.
///
/// # Errors
///
/// [`FreezeError::PathCollision`] when two mounts fold to the same path,
/// [`FreezeError::NotFound`] when a name has no current row, and
/// [`FreezeError::NotReady`] when the current row has not published bytes.
pub fn freeze_manifest(
    current: &BTreeMap<ResourceName, WorkspaceFile>,
    mounts: &[FileMount],
) -> Result<Vec<FrozenFile>, FreezeError> {
    let mut paths = BTreeSet::new();
    let mut frozen = Vec::with_capacity(mounts.len());
    for mount in mounts {
        let folded = mount.path.as_str().to_ascii_lowercase();
        if !paths.insert(folded) {
            return Err(FreezeError::PathCollision(mount.path.to_string()));
        }
        let file = current
            .get(&mount.name)
            .ok_or_else(|| FreezeError::NotFound(mount.name.as_str().to_owned()))?;
        let ready = file
            .ready()
            .ok_or_else(|| FreezeError::NotReady(mount.name.as_str().to_owned()))?;
        frozen.push(FrozenFile {
            name: mount.name.clone(),
            path: mount.path.clone(),
            content: ready.clone(),
        });
    }
    Ok(frozen)
}

/// Tracks immutable object reachability from current names and live sessions.
#[derive(Debug, Default)]
pub struct FileReferences {
    current: BTreeMap<ContentDigest, usize>,
    sessions: BTreeMap<SessionId, BTreeSet<ContentDigest>>,
}

impl FileReferences {
    /// Records a current-name pointer replacement.
    pub fn replace_current(&mut self, old: Option<&ReadyFile>, new: Option<&ReadyFile>) {
        if let Some(old) = old {
            decrement(&mut self.current, old.digest);
        }
        if let Some(new) = new {
            *self.current.entry(new.digest).or_default() += 1;
        }
    }

    /// Retains every object in a frozen session manifest.
    pub fn retain_session(&mut self, session: SessionId, manifest: &[FrozenFile]) {
        self.sessions.insert(
            session,
            manifest.iter().map(|file| file.content.digest).collect(),
        );
    }

    /// Releases a session's exact manifest after terminal cleanup evidence.
    pub fn release_session(&mut self, session: SessionId) {
        self.sessions.remove(&session);
    }

    /// Whether an immutable object is still reachable.
    #[must_use]
    pub fn is_referenced(&self, digest: ContentDigest) -> bool {
        self.current.get(&digest).is_some_and(|count| *count > 0)
            || self.sessions.values().any(|set| set.contains(&digest))
    }

    /// Displaced/orphan objects now safe for the collector to delete.
    #[must_use]
    pub fn collectible<'a>(
        &self,
        candidates: impl IntoIterator<Item = &'a ReadyFile>,
    ) -> Vec<ContentDigest> {
        candidates
            .into_iter()
            .map(|file| file.digest)
            .filter(|digest| !self.is_referenced(*digest))
            .collect()
    }
}

fn decrement(counts: &mut BTreeMap<ContentDigest, usize>, digest: ContentDigest) {
    let Some(count) = counts.get_mut(&digest) else {
        return;
    };
    *count = count.saturating_sub(1);
    if *count == 0 {
        counts.remove(&digest);
    }
}

/// Stable identity/input of one trusted `storage.persist` invocation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoragePersistRequest {
    /// Durable tool-call identity; retries reuse it.
    pub call: ToolCallId,
    /// Exact normalized guest path selected by the model.
    pub path: NormalizedPath,
    /// Latest-only workspace-file name to overwrite.
    pub name: ResourceName,
    /// Declared media type.
    pub media_type: String,
}

/// Verified result returned to the model and telemetry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoragePersistResult {
    /// Latest-only logical name.
    pub name: ResourceName,
    /// Exact persisted digest.
    pub digest: ContentDigest,
    /// Exact persisted byte length.
    pub size_bytes: u64,
    /// Declared media type.
    pub media_type: String,
    /// Bounded model/live preview.
    pub preview: Vec<u8>,
    /// Whether the full bytes exceeded the preview bound.
    pub truncated: bool,
    /// Stable guest path containing the full bytes for the call.
    pub full_result_path: NormalizedPath,
}

/// Private replay record for `storage.persist`.
#[derive(Debug, Default)]
pub struct StoragePersistReceipts {
    receipts: BTreeMap<ToolCallId, (StoragePersistRequest, StoragePersistResult)>,
}

/// Storage-persist admission/replay outcome.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StoragePersistOutcome {
    /// First execution for this exact tool call.
    Execute,
    /// Identical retry returns the committed answer without another upload.
    Replay(StoragePersistResult),
}

/// Why a storage-persist retry was refused.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum StoragePersistError {
    /// One durable call identity was reused with different arguments.
    #[error("storage.persist tool call identity was reused with different arguments")]
    IdempotencyConflict,
}

impl StoragePersistReceipts {
    /// Checks whether this invocation must execute or replay.
    ///
    /// # Errors
    ///
    /// [`StoragePersistError::IdempotencyConflict`] when the durable tool-call
    /// identity was already committed with different arguments.
    pub fn admit(
        &self,
        request: &StoragePersistRequest,
    ) -> Result<StoragePersistOutcome, StoragePersistError> {
        let Some((stored_request, stored_result)) = self.receipts.get(&request.call) else {
            return Ok(StoragePersistOutcome::Execute);
        };
        if stored_request != request {
            return Err(StoragePersistError::IdempotencyConflict);
        }
        Ok(StoragePersistOutcome::Replay(stored_result.clone()))
    }

    /// Records the verified trusted commit.
    pub fn commit(&mut self, request: StoragePersistRequest, result: StoragePersistResult) {
        self.receipts
            .entry(request.call)
            .or_insert((request, result));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aex_wire::ids::{PrefixedId as _, Uuid7};

    fn workspace() -> WorkspaceId {
        WorkspaceId::from_uuid7(Uuid7::compose(1, [1; 10]))
    }

    fn session(seed: u8) -> SessionId {
        SessionId::from_uuid7(Uuid7::compose(1, [seed; 10]))
    }

    fn call(seed: u8) -> ToolCallId {
        ToolCallId::from_uuid7(Uuid7::compose(1, [seed; 10]))
    }

    fn name(value: &str) -> ResourceName {
        ResourceName::parse(value).expect("name")
    }

    fn time(value: i64) -> Timestamp {
        Timestamp::from_unix_millis(value).expect("time")
    }

    fn ready(bytes: &[u8], key: &str) -> ReadyFile {
        ReadyFile {
            digest: ContentDigest::of(bytes),
            size_bytes: bytes.len() as u64,
            media_type: "application/octet-stream".to_owned(),
            object_key: key.to_owned(),
        }
    }

    const fn intent(value: u64) -> FileIntent {
        FileIntent::from_private_counter(value)
    }

    #[test]
    fn pending_overwrite_hides_prior_bytes_and_stale_completion_cannot_publish() {
        let first = admit_inline(
            None,
            workspace(),
            name("asset"),
            ready(b"one", "o/1"),
            time(1),
            intent(1),
        );
        let url = admit_pending(
            Some(&first.current),
            workspace(),
            name("asset"),
            FileSource::Url,
            time(2),
            intent(2),
        );
        assert!(url.current.ready().is_none());
        let upload = admit_pending(
            Some(&url.current),
            workspace(),
            name("asset"),
            FileSource::Upload,
            time(3),
            intent(3),
        );
        assert!(matches!(
            publish_ready(
                &upload.current,
                url.current.intent(),
                ready(b"url", "o/url"),
                time(4)
            ),
            FilePublish::Stale { orphan: Some(_) }
        ));
        let FilePublish::Published(final_value) = publish_ready(
            &upload.current,
            upload.current.intent(),
            ready(b"upload", "o/upload"),
            time(5),
        ) else {
            panic!("current upload publishes")
        };
        assert_eq!(
            final_value.ready().expect("ready").digest,
            ContentDigest::of(b"upload")
        );
    }

    #[test]
    fn delete_and_recreate_cannot_reuse_an_async_publication_fence() {
        let deleted = admit_pending(
            None,
            workspace(),
            name("asset"),
            FileSource::Url,
            time(1),
            intent(41),
        );
        // Dropping the current row models delete. The authority's private
        // counter survives and the recreated name receives a fresh intent.
        let recreated = admit_pending(
            None,
            workspace(),
            name("asset"),
            FileSource::Upload,
            time(2),
            intent(42),
        );
        assert!(matches!(
            publish_ready(
                &recreated.current,
                deleted.current.intent(),
                ready(b"late-url", "o/late"),
                time(3),
            ),
            FilePublish::Stale { orphan: Some(_) }
        ));
    }

    #[test]
    fn frozen_manifest_survives_latest_overwrite_and_holds_gc_reference() {
        let current = admit_inline(
            None,
            workspace(),
            name("asset"),
            ready(b"one", "o/1"),
            time(1),
            intent(1),
        );
        let mut files = BTreeMap::new();
        files.insert(name("asset"), current.current.clone());
        let manifest = freeze_manifest(
            &files,
            &[FileMount {
                name: name("asset"),
                path: NormalizedPath::parse("inputs/asset.bin").expect("path"),
            }],
        )
        .expect("freeze");
        let replacement = admit_inline(
            Some(&current.current),
            workspace(),
            name("asset"),
            ready(b"two", "o/2"),
            time(2),
            intent(2),
        );
        assert_eq!(manifest[0].content.digest, ContentDigest::of(b"one"));

        let mut refs = FileReferences::default();
        refs.replace_current(None, current.current.ready());
        refs.retain_session(session(2), &manifest);
        refs.replace_current(current.current.ready(), replacement.current.ready());
        let displaced = replacement.displaced.as_ref().expect("displaced");
        assert!(refs.collectible([displaced]).is_empty());
        refs.release_session(session(2));
        assert_eq!(
            refs.collectible([displaced]),
            vec![ContentDigest::of(b"one")]
        );
    }

    #[test]
    fn case_colliding_mount_paths_are_refused() {
        let one = admit_inline(
            None,
            workspace(),
            name("one"),
            ready(b"1", "o/1"),
            time(1),
            intent(1),
        );
        let two = admit_inline(
            None,
            workspace(),
            name("two"),
            ready(b"2", "o/2"),
            time(1),
            intent(1),
        );
        let files = BTreeMap::from([(name("one"), one.current), (name("two"), two.current)]);
        assert!(matches!(
            freeze_manifest(
                &files,
                &[
                    FileMount {
                        name: name("one"),
                        path: NormalizedPath::parse("A/x").expect("path")
                    },
                    FileMount {
                        name: name("two"),
                        path: NormalizedPath::parse("a/X").expect("path")
                    },
                ],
            ),
            Err(FreezeError::PathCollision(_))
        ));
    }

    #[test]
    fn storage_persist_replays_exact_call_and_refuses_changed_arguments() {
        let request = StoragePersistRequest {
            call: call(3),
            path: NormalizedPath::parse("outputs/movie.mp4").expect("path"),
            name: name("movie"),
            media_type: "video/mp4".to_owned(),
        };
        let result = StoragePersistResult {
            name: request.name.clone(),
            digest: ContentDigest::of(&[0, 159, 146, 150]),
            size_bytes: 4,
            media_type: request.media_type.clone(),
            preview: vec![0, 159],
            truncated: true,
            full_result_path: request.path.clone(),
        };
        let mut receipts = StoragePersistReceipts::default();
        assert_eq!(receipts.admit(&request), Ok(StoragePersistOutcome::Execute));
        receipts.commit(request.clone(), result.clone());
        assert_eq!(
            receipts.admit(&request),
            Ok(StoragePersistOutcome::Replay(result))
        );

        let mut changed = request;
        changed.name = name("other");
        assert_eq!(
            receipts.admit(&changed),
            Err(StoragePersistError::IdempotencyConflict)
        );
    }
}
