//! The one export this task produces, and the two provider surfaces it needs.
//!
//! The order of operations is the contract:
//!
//! 1. every memory reservation is acquired, before anything is read;
//! 2. the export row is resolved and the lease is taken, before any observation
//!    is read — losing it exits **zero**, because a cancel or a takeover is
//!    authoritative rather than an error;
//! 3. bounded pages are streamed from the pinned snapshot, never the whole
//!    export;
//! 4. each filled part is uploaded and then checkpointed under the fence, so a
//!    replacement task resumes from durable state alone;
//! 5. the manifest is written last, the upload is completed, the object is
//!    re-read and verified, and only then is the export published by one
//!    conditional update.
//!
//! One member is one part. S3 refuses a non-final part under 5 MiB, so the
//! trailing member and the manifest share the **single** final part rather than
//! being uploaded as two undersized ones.
//!
//! Both provider surfaces are traits, so every one of those steps is exercised
//! without AWS. The production implementations are the real SDK clients and live
//! in [`crate::aws`].

use aex_observation_domain::keys::ScopeKey;
use aex_observation_export::checkpoint::verify_parts;
use aex_observation_export::encoder::{encoder_for, sha256_hex};
use aex_observation_export::{
    Completeness, EncodeError, ExportCheckpoint, ExportManifest, ExportMember, Format,
    ManifestError, MemberEncoder, PartRecord, Publication, ResumeError,
};
use aex_otlp_admission::MemoryBudget;
use aex_wire::error::ErrorCode;
use aex_wire::types::Timestamp;

use crate::budget::{CapacityError, MemoryPlan, PAGE_SLOT_BYTES};

/// The largest part number S3 accepts in one multipart upload.
pub const PART_NUMBER_MAX: u32 = 10_000;

/// The prefix every export object is written under.
pub const EXPORT_PREFIX: &str = "exports";

/// Why the export stopped.
#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
pub enum TaskError {
    /// A memory reservation could not be met.
    #[error(transparent)]
    Capacity(#[from] CapacityError),
    /// A resume could not be reconciled with the provider.
    #[error(transparent)]
    Resume(#[from] ResumeError),
    /// A member could not be encoded.
    #[error(transparent)]
    Encode(#[from] EncodeError),
    /// The manifest refused the export.
    #[error(transparent)]
    Manifest(#[from] ManifestError),
    /// The produced artifact does not match what was uploaded.
    #[error("the export failed its integrity check: {what}")]
    Integrity {
        /// What disagreed.
        what: String,
    },
    /// A page carried more than the reservation covers.
    #[error("a page carried {observed} records against a {limit}-record reservation")]
    PageOverrun {
        /// What the provider returned.
        observed: usize,
        /// What the reservation covers.
        limit: usize,
    },
    /// One record exceeded the per-observation ceiling.
    #[error("a record of {observed} bytes exceeds the {limit}-byte per-observation ceiling")]
    RecordTooLarge {
        /// The observed record size.
        observed: usize,
        /// The ceiling.
        limit: usize,
    },
    /// The export needs more parts than one multipart upload allows.
    #[error("the export needs more than {limit} parts; raise the part size")]
    PartLimit {
        /// The provider ceiling.
        limit: u32,
    },
    /// A provider call failed.
    #[error("`{operation}` failed: {reason}")]
    Provider {
        /// Which call.
        operation: &'static str,
        /// What the provider reported, without its own body.
        reason: String,
    },
    /// A stored item did not carry a value this task can read.
    #[error("stored item `{item}` is missing attribute `{attribute}`")]
    Malformed {
        /// Which item family.
        item: &'static str,
        /// Which attribute.
        attribute: &'static str,
    },
}

impl TaskError {
    /// The wire code this failure is published as, when it has one.
    #[must_use]
    pub const fn code(&self) -> Option<ErrorCode> {
        match self {
            Self::Capacity(_) => Some(CapacityError::CODE),
            _ => None,
        }
    }

    /// Builds a provider failure without carrying an upstream body.
    #[must_use]
    pub fn provider(operation: &'static str, reason: impl std::fmt::Display) -> Self {
        Self::Provider {
            operation,
            reason: reason.to_string(),
        }
    }
}

/// The export row, as the lease read it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExportLease {
    /// The fence this task holds.
    pub fence: u64,
    /// The scope the export reads.
    pub scope: ScopeKey,
    /// The pinned snapshot, in epoch milliseconds.
    pub snapshot: u128,
    /// The requested format.
    pub format: Format,
    /// The completeness rule.
    pub completeness: Completeness,
    /// Every gap intersecting the window.
    pub gaps: Vec<String>,
    /// The deletion epoch the export pinned.
    pub pinned_deletion_epoch: u64,
    /// When the published artifact expires.
    pub expires_at: Timestamp,
    /// The pinned partitions the walk visits, in order.
    pub partitions: Vec<String>,
}

/// What the lease attempt did.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum LeaseOutcome {
    /// The lease moved `launching` to `generating` under this fence.
    Taken(Box<ExportLease>),
    /// A cancel, a deletion or another task holds the row.
    Lost {
        /// What won.
        reason: &'static str,
    },
}

/// One bounded page of canonical records.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Page {
    /// The canonical bytes of each observation, in walk order.
    pub records: Vec<Vec<u8>>,
    /// Where to resume, or `None` when the pinned snapshot is exhausted.
    pub next_cursor: Option<Box<str>>,
}

/// A provider part listing, already read to exhaustion.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct PartListing {
    /// Every part the provider reported.
    pub parts: Vec<PartRecord>,
    /// Whether the provider signalled more parts and gave no marker.
    ///
    /// A short list looks exactly like a completed upload, so this can never be
    /// silently folded into an empty tail.
    pub truncated_without_marker: bool,
}

/// The durable resume point plus the member ledger the manifest is built from.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExportProgress {
    /// The crate's checkpoint: fence, upload identity, parts, cursor, member.
    pub checkpoint: ExportCheckpoint,
    /// Every finished member, in ordinal order.
    pub members: Vec<ExportMember>,
}

/// Everything the one publishing update carries.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PublishRequest {
    /// The fence the update is conditioned on.
    pub fence: u64,
    /// The scope whose live deletion epoch the update is conditioned on.
    pub scope: ScopeKey,
    /// The deletion epoch the export pinned.
    pub pinned_deletion_epoch: u64,
    /// The published object.
    pub object_key: Box<str>,
    /// The manifest hash the customer verifies against.
    pub manifest_hash: Box<str>,
    /// The exact published length.
    pub object_bytes: u64,
    /// When the export became ready.
    pub ready_at: Timestamp,
    /// When the artifact expires.
    pub expires_at: Timestamp,
}

/// What the provider reports about the completed object.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ObjectHead {
    /// The exact stored length.
    pub bytes: u64,
    /// The provider checksum of the stored object.
    pub checksum: Box<str>,
}

/// The `DynamoDB` surface one export needs.
#[async_trait::async_trait]
pub trait ExportAuthority: Send + Sync {
    /// Moves `launching` to `generating` and bumps the fence, under the lease.
    ///
    /// # Errors
    ///
    /// Returns [`TaskError::Provider`] when the conditional update could not be
    /// attempted. A **lost** condition is not an error: it is
    /// [`LeaseOutcome::Lost`].
    async fn take_lease(&self, now: Timestamp) -> Result<LeaseOutcome, TaskError>;

    /// Reads the durable resume point written under this fence.
    ///
    /// # Errors
    ///
    /// Returns [`TaskError::Provider`] when the read fails and
    /// [`TaskError::Malformed`] when the stored item cannot be decoded.
    async fn load_progress(&self, fence: u64) -> Result<Option<ExportProgress>, TaskError>;

    /// Persists the resume point under the fence.
    ///
    /// # Errors
    ///
    /// Returns [`TaskError::Provider`] when the fenced write fails.
    async fn record_progress(&self, progress: &ExportProgress) -> Result<(), TaskError>;

    /// Reads one bounded page from the pinned snapshot.
    ///
    /// # Errors
    ///
    /// Returns [`TaskError::Provider`] when the read fails and
    /// [`TaskError::Malformed`] when a stored observation cannot be decoded.
    async fn next_page(
        &self,
        lease: &ExportLease,
        cursor: Option<&str>,
        limit: usize,
    ) -> Result<Page, TaskError>;

    /// Publishes the export with one conditional update.
    ///
    /// # Errors
    ///
    /// Returns [`TaskError::Provider`] when the update could not be attempted. A
    /// lost condition is [`Publication::Superseded`], not an error.
    async fn publish(&self, request: &PublishRequest) -> Result<Publication, TaskError>;
}

/// The `S3` surface one export needs.
#[async_trait::async_trait]
pub trait ExportObjects: Send + Sync {
    /// Starts the multipart upload.
    ///
    /// # Errors
    ///
    /// Returns [`TaskError::Provider`] when the upload could not be created.
    async fn create_upload(&self, key: &str) -> Result<Box<str>, TaskError>;

    /// Uploads one part and returns the provider `ETag`.
    ///
    /// # Errors
    ///
    /// Returns [`TaskError::Provider`] when the part could not be uploaded.
    async fn upload_part(
        &self,
        key: &str,
        upload_id: &str,
        number: u32,
        body: Vec<u8>,
    ) -> Result<Box<str>, TaskError>;

    /// Lists every uploaded part, following the marker **to exhaustion**.
    ///
    /// # Errors
    ///
    /// Returns [`TaskError::Provider`] when the listing could not be read.
    async fn list_parts(&self, key: &str, upload_id: &str) -> Result<PartListing, TaskError>;

    /// Completes the upload and returns the provider checksum.
    ///
    /// # Errors
    ///
    /// Returns [`TaskError::Provider`] when the upload could not be completed.
    async fn complete_upload(
        &self,
        key: &str,
        upload_id: &str,
        parts: &[PartRecord],
    ) -> Result<Box<str>, TaskError>;

    /// Aborts the upload explicitly.
    ///
    /// # Errors
    ///
    /// Returns [`TaskError::Provider`] when the abort could not be issued.
    async fn abort_upload(&self, key: &str, upload_id: &str) -> Result<(), TaskError>;

    /// Re-reads the completed object.
    ///
    /// # Errors
    ///
    /// Returns [`TaskError::Provider`] when the object could not be read.
    async fn head_object(&self, key: &str) -> Result<ObjectHead, TaskError>;
}

/// What one export run did.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ExportOutcome {
    /// The export is `ready` and the artifact is verified.
    Published {
        /// Where the artifact lives.
        object_key: Box<str>,
        /// Its exact length.
        object_bytes: u64,
        /// Its manifest hash.
        manifest_hash: Box<str>,
        /// How many parts it was uploaded in.
        parts: u32,
        /// How many records it carries.
        records: u64,
    },
    /// A cancel, a deletion or a lease takeover won.
    ///
    /// The task aborted its multipart upload, deleted nothing else and exits
    /// **zero**.
    Superseded {
        /// What won.
        reason: &'static str,
    },
}

impl ExportOutcome {
    /// Whether the artifact became `ready`.
    #[must_use]
    pub const fn is_published(&self) -> bool {
        matches!(self, Self::Published { .. })
    }

    /// What won, when the export was superseded.
    #[must_use]
    pub const fn superseded_by(&self) -> Option<&'static str> {
        match self {
            Self::Superseded { reason } => Some(reason),
            Self::Published { .. } => None,
        }
    }
}

/// Everything one export run is bounded by.
#[derive(Clone, Debug)]
pub struct TaskSettings {
    /// The four reservations acquired before the producing loop.
    pub plan: MemoryPlan,
    /// The working budget the reservations are taken from.
    pub budget: MemoryBudget,
    /// How many observations one page may carry.
    pub page_limit: usize,
    /// How large one multipart part is.
    pub part_bytes: usize,
    /// The object key, without the extension the leased format decides.
    pub object_prefix: Box<str>,
}

/// The object prefix one export is written under.
///
/// The extension is not knowable here: the requested format lives on the export
/// row and is only read once the lease is held.
#[must_use]
pub fn object_prefix(
    workspace: aex_wire::ids::WorkspaceId,
    export: aex_wire::ids::ExportId,
) -> String {
    format!("{EXPORT_PREFIX}/{workspace}/{export}")
}

/// The object key the leased format implies.
#[must_use]
pub fn artifact_key(prefix: &str, format: Format) -> String {
    format!("{prefix}.{}", format.extension())
}

/// The name of one member inside the artifact.
#[must_use]
pub fn member_name(ordinal: u16, format: Format) -> String {
    format!("{ordinal:04}.{}", format.extension())
}

/// Where the walk is inside the pinned snapshot.
#[derive(Clone, Debug, Eq, PartialEq)]
enum Position {
    /// Nothing has been read yet. Never persisted: a fresh export has no
    /// checkpoint at all, so an empty stored cursor is unambiguous.
    Start,
    /// Resume strictly after this cursor.
    After(Box<str>),
    /// The pinned snapshot is exhausted.
    Exhausted,
}

impl Position {
    /// The cursor a page read continues from.
    fn as_cursor(&self) -> Option<&str> {
        match self {
            Self::After(cursor) => Some(cursor),
            Self::Start | Self::Exhausted => None,
        }
    }

    /// The stored spelling. `Start` is never stored.
    fn to_stored(&self) -> Box<str> {
        match self {
            Self::After(cursor) => cursor.clone(),
            Self::Start | Self::Exhausted => Box::from(""),
        }
    }

    /// Reads the stored spelling. An empty cursor is an exhausted walk.
    fn from_stored(stored: &str) -> Self {
        if stored.is_empty() {
            Self::Exhausted
        } else {
            Self::After(Box::from(stored))
        }
    }
}

/// What has been staged into the encoder but not yet uploaded.
#[derive(Clone, Copy, Debug, Default)]
struct Staged {
    bytes: usize,
    records: u64,
}

/// The member still open when the walk exhausted.
struct Pending {
    encoder: Box<dyn MemberEncoder>,
    staged: Staged,
}

/// The upload as it stands right now.
#[derive(Clone, Debug)]
struct UploadState {
    key: Box<str>,
    upload_id: Box<str>,
    parts: Vec<PartRecord>,
    members: Vec<ExportMember>,
    position: Position,
    member_ordinal: u16,
    member_checkpoint: u64,
    bytes_written: u64,
}

impl UploadState {
    /// A fresh upload.
    fn fresh(key: Box<str>, upload_id: Box<str>) -> Self {
        Self {
            key,
            upload_id,
            parts: Vec::new(),
            members: Vec::new(),
            position: Position::Start,
            member_ordinal: 0,
            member_checkpoint: 0,
            bytes_written: 0,
        }
    }

    /// The upload a checkpoint describes.
    fn resumed(key: Box<str>, progress: ExportProgress) -> Self {
        let checkpoint = progress.checkpoint;
        Self {
            key,
            upload_id: checkpoint.upload_id,
            parts: checkpoint.parts,
            members: progress.members,
            position: Position::from_stored(&checkpoint.cursor),
            member_ordinal: checkpoint.member_ordinal,
            member_checkpoint: checkpoint.member_checkpoint,
            bytes_written: checkpoint.bytes_written,
        }
    }

    /// The durable resume point this state implies.
    fn progress(&self, fence: u64) -> ExportProgress {
        ExportProgress {
            checkpoint: ExportCheckpoint {
                fence,
                upload_id: self.upload_id.clone(),
                parts: self.parts.clone(),
                cursor: self.position.to_stored(),
                member_ordinal: self.member_ordinal,
                member_checkpoint: self.member_checkpoint,
                bytes_written: self.bytes_written,
            },
            members: self.members.clone(),
        }
    }

    /// Records one finished member.
    fn push_member(&mut self, member: ExportMember) -> Result<(), TaskError> {
        self.member_ordinal =
            member
                .ordinal
                .checked_add(1)
                .ok_or_else(|| TaskError::Integrity {
                    what: "the export needs more members than one artifact can carry".to_owned(),
                })?;
        self.members.push(member);
        Ok(())
    }

    /// Records one uploaded part.
    fn push_part(&mut self, part: PartRecord) {
        self.bytes_written += part.bytes;
        self.parts.push(part);
    }
}

/// What sealing the artifact produced.
struct Sealed {
    manifest_hash: String,
    records: u64,
}

/// One export, composed over the two provider surfaces.
pub struct ExportTask<A, O> {
    authority: A,
    objects: O,
    settings: TaskSettings,
}

impl<A, O> ExportTask<A, O>
where
    A: ExportAuthority,
    O: ExportObjects,
{
    /// Composes one export run.
    #[must_use]
    pub fn new(authority: A, objects: O, settings: TaskSettings) -> Self {
        Self {
            authority,
            objects,
            settings,
        }
    }

    /// Generates exactly one export and stops.
    ///
    /// # Errors
    ///
    /// Returns the typed failure of the first stage that refused. A cancel, a
    /// deletion or a lease takeover is **not** a failure: it is
    /// [`ExportOutcome::Superseded`], and the caller exits zero.
    pub async fn run(&self, now: Timestamp) -> Result<ExportOutcome, TaskError> {
        // Reserved before anything is read, encoded or uploaded. A budget that
        // cannot cover the plan fails here, never as an OOM kill later.
        let reserved = self.settings.plan.acquire(&self.settings.budget)?;
        debug_assert_eq!(reserved.count(), MemoryPlan::RESERVATIONS);

        let lease = match self.authority.take_lease(now).await? {
            LeaseOutcome::Lost { reason } => return Ok(ExportOutcome::Superseded { reason }),
            LeaseOutcome::Taken(lease) => *lease,
        };
        let mut state = self.resume(&lease).await?;
        let outcome = self.generate(&lease, &mut state, now).await;
        self.settle(&state, outcome).await
    }

    /// Aborts an abandoned upload and reports what happened.
    async fn settle(
        &self,
        state: &UploadState,
        outcome: Result<ExportOutcome, TaskError>,
    ) -> Result<ExportOutcome, TaskError> {
        match outcome {
            Ok(ExportOutcome::Published { .. }) => outcome,
            // An abandoned upload is aborted explicitly; it is never left to a
            // bucket lifecycle rule. Aborting an upload that was already
            // completed is a no-op, which keeps this the single cleanup path.
            // Nothing else is deleted: this role holds no `s3:DeleteObject`.
            Ok(ExportOutcome::Superseded { reason }) => {
                self.abort(state).await?;
                Ok(ExportOutcome::Superseded { reason })
            }
            Err(error) => {
                if let Err(abort) = self.abort(state).await {
                    tracing::error!(error = %abort, "the multipart upload could not be aborted");
                }
                Err(error)
            }
        }
    }

    /// Explicitly aborts the multipart upload.
    async fn abort(&self, state: &UploadState) -> Result<(), TaskError> {
        self.objects
            .abort_upload(&state.key, &state.upload_id)
            .await
    }

    /// Resumes from durable state, or starts a new upload.
    async fn resume(&self, lease: &ExportLease) -> Result<UploadState, TaskError> {
        let key: Box<str> = artifact_key(&self.settings.object_prefix, lease.format).into();
        let Some(progress) = self.authority.load_progress(lease.fence).await? else {
            let upload_id = self.objects.create_upload(&key).await?;
            return Ok(UploadState::fresh(key, upload_id));
        };
        // The listing is read to exhaustion by the adapter; a truncation without
        // a marker is a hard integrity failure here, never a silent short list.
        // The upload is deliberately **not** aborted: the checkpoint is still
        // authoritative and a replacement task must be able to re-verify it.
        let listing = self
            .objects
            .list_parts(&key, &progress.checkpoint.upload_id)
            .await?;
        verify_parts(
            &progress.checkpoint,
            &listing.parts,
            listing.truncated_without_marker,
        )?;
        Ok(UploadState::resumed(key, progress))
    }

    /// Streams, encodes, uploads, seals and publishes.
    async fn generate(
        &self,
        lease: &ExportLease,
        state: &mut UploadState,
        now: Timestamp,
    ) -> Result<ExportOutcome, TaskError> {
        let pending = self.produce(lease, state).await?;
        let sealed = self.seal(lease, state, pending, now).await?;
        self.commit(lease, state, &sealed, now).await
    }

    /// The producing loop: one bounded page at a time, one part at a time.
    async fn produce(
        &self,
        lease: &ExportLease,
        state: &mut UploadState,
    ) -> Result<Pending, TaskError> {
        // Built before the loop, so an unsupported format is a typed refusal
        // rather than a half-written artifact.
        let mut encoder = encoder_for(lease.format)?;
        let mut staged = Staged::default();
        while !matches!(state.position, Position::Exhausted) {
            let page = self
                .authority
                .next_page(lease, state.position.as_cursor(), self.settings.page_limit)
                .await?;
            self.check_page(&page)?;
            let next = page
                .next_cursor
                .map_or(Position::Exhausted, Position::After);
            if next == state.position {
                return Err(TaskError::Integrity {
                    what: "a page returned the cursor it was given, so the walk cannot progress"
                        .to_owned(),
                });
            }
            for record in &page.records {
                encoder.feed(record)?;
                staged.bytes += record.len() + 1;
                staged.records += 1;
            }
            state.position = next;
            // Only a full part is flushed here: the trailing member rides with
            // the manifest in the single final part, which is the only one S3
            // allows to sit under the 5 MiB floor.
            if staged.bytes >= self.settings.part_bytes {
                let finished = std::mem::replace(&mut encoder, encoder_for(lease.format)?);
                self.flush(lease, state, finished, staged).await?;
                staged = Staged::default();
            }
        }
        Ok(Pending { encoder, staged })
    }

    /// Bounds one page against the reservation it was sized for.
    fn check_page(&self, page: &Page) -> Result<(), TaskError> {
        if page.records.len() > self.settings.page_limit {
            return Err(TaskError::PageOverrun {
                observed: page.records.len(),
                limit: self.settings.page_limit,
            });
        }
        if let Some(record) = page
            .records
            .iter()
            .find(|record| record.len() > PAGE_SLOT_BYTES)
        {
            return Err(TaskError::RecordTooLarge {
                observed: record.len(),
                limit: PAGE_SLOT_BYTES,
            });
        }
        Ok(())
    }

    /// Finishes one member, uploads it as one part and checkpoints it.
    async fn flush(
        &self,
        lease: &ExportLease,
        state: &mut UploadState,
        encoder: Box<dyn MemberEncoder>,
        staged: Staged,
    ) -> Result<(), TaskError> {
        let member_checkpoint = encoder
            .safe_checkpoint()
            .ok_or_else(|| TaskError::Integrity {
                what: "the encoder is mid-structure, so this point is not resumable".to_owned(),
            })?;
        let member = close_member(state.member_ordinal, encoder, staged, lease.format)?;
        let number = next_part_number(state.parts.len())?;
        let etag = self
            .objects
            .upload_part(&state.key, &state.upload_id, number, member.body)
            .await?;
        state.push_part(PartRecord {
            number,
            etag,
            sha256: member.member.sha256.clone(),
            bytes: member.member.bytes,
        });
        state.push_member(member.member)?;
        state.member_checkpoint = member_checkpoint;
        self.authority
            .record_progress(&state.progress(lease.fence))
            .await
    }

    /// Writes the manifest last, completes the upload and verifies the object.
    async fn seal(
        &self,
        lease: &ExportLease,
        state: &mut UploadState,
        pending: Pending,
        now: Timestamp,
    ) -> Result<Sealed, TaskError> {
        let mut body = Vec::new();
        if pending.staged.records > 0 {
            let closed = close_member(
                state.member_ordinal,
                pending.encoder,
                pending.staged,
                lease.format,
            )?;
            body = closed.body;
            state.push_member(closed.member)?;
        }
        let manifest = ExportManifest::build(
            lease.completeness,
            lease.gaps.clone(),
            state.members.clone(),
            lease.snapshot,
            &now.to_wire(),
        )?;
        let sealed = Sealed {
            manifest_hash: manifest.manifest_hash(),
            records: manifest.records,
        };
        // One line, so the artifact stays a single well-formed stream whose last
        // line is the manifest describing every line before it.
        body.extend_from_slice(&manifest.to_canonical_bytes());
        body.push(b'\n');

        let bytes = length_of(&body)?;
        let sha256 = sha256_hex(&body);
        let number = next_part_number(state.parts.len())?;
        let etag = self
            .objects
            .upload_part(&state.key, &state.upload_id, number, body)
            .await?;
        state.push_part(PartRecord {
            number,
            etag,
            sha256: sha256.into(),
            bytes,
        });

        let checksum = self
            .objects
            .complete_upload(&state.key, &state.upload_id, &state.parts)
            .await?;
        self.verify(state, &checksum).await?;
        Ok(sealed)
    }

    /// Re-reads the completed object and proves it is the one just written.
    async fn verify(&self, state: &UploadState, checksum: &str) -> Result<(), TaskError> {
        let head = self.objects.head_object(&state.key).await?;
        if head.bytes != state.bytes_written {
            return Err(TaskError::Integrity {
                what: format!(
                    "the stored object is {} bytes and {} were uploaded",
                    head.bytes, state.bytes_written
                ),
            });
        }
        if head.checksum.as_ref() != checksum {
            return Err(TaskError::Integrity {
                what: "the stored object's checksum is not the completed upload's".to_owned(),
            });
        }
        Ok(())
    }

    /// The one conditional update that publishes the export.
    async fn commit(
        &self,
        lease: &ExportLease,
        state: &UploadState,
        sealed: &Sealed,
        now: Timestamp,
    ) -> Result<ExportOutcome, TaskError> {
        let request = PublishRequest {
            fence: lease.fence,
            scope: lease.scope,
            pinned_deletion_epoch: lease.pinned_deletion_epoch,
            object_key: state.key.clone(),
            manifest_hash: sealed.manifest_hash.as_str().into(),
            object_bytes: state.bytes_written,
            ready_at: now,
            expires_at: lease.expires_at,
        };
        match self.authority.publish(&request).await? {
            Publication::Ready => Ok(ExportOutcome::Published {
                object_key: state.key.clone(),
                object_bytes: state.bytes_written,
                manifest_hash: sealed.manifest_hash.as_str().into(),
                parts: part_count(state.parts.len())?,
                records: sealed.records,
            }),
            Publication::Superseded { reason } => Ok(ExportOutcome::Superseded { reason }),
        }
    }
}

/// One finished member and the exact bytes it produced.
struct ClosedMember {
    member: ExportMember,
    body: Vec<u8>,
}

/// Finishes the encoder and describes the member it produced.
fn close_member(
    ordinal: u16,
    encoder: Box<dyn MemberEncoder>,
    staged: Staged,
    format: Format,
) -> Result<ClosedMember, TaskError> {
    debug_assert!(staged.records > 0, "an empty member is never closed");
    let (body, sha256) = encoder.finish()?;
    let bytes = length_of(&body)?;
    Ok(ClosedMember {
        member: ExportMember {
            ordinal,
            name: member_name(ordinal, format).into(),
            bytes,
            sha256: sha256.into(),
            records: staged.records,
        },
        body,
    })
}

/// The next part number, bounded by the provider ceiling.
fn next_part_number(uploaded: usize) -> Result<u32, TaskError> {
    let number = part_count(uploaded)?
        .checked_add(1)
        .ok_or(TaskError::PartLimit {
            limit: PART_NUMBER_MAX,
        })?;
    if number > PART_NUMBER_MAX {
        return Err(TaskError::PartLimit {
            limit: PART_NUMBER_MAX,
        });
    }
    Ok(number)
}

/// How many parts have been uploaded.
fn part_count(uploaded: usize) -> Result<u32, TaskError> {
    u32::try_from(uploaded).map_err(|_| TaskError::PartLimit {
        limit: PART_NUMBER_MAX,
    })
}

/// The exact length of a body, as the manifest records it.
fn length_of(body: &[u8]) -> Result<u64, TaskError> {
    u64::try_from(body.len()).map_err(|_| TaskError::Integrity {
        what: "a member is longer than a 64-bit byte count".to_owned(),
    })
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use aex_observation_domain::keys::ScopeKey;
    use aex_observation_export::{
        Completeness, EncodeError, ExportCheckpoint, ExportMember, Format, PartRecord, Publication,
        ResumeError,
    };
    use aex_otlp_admission::MemoryBudget;
    use aex_wire::error::ErrorCode;
    use aex_wire::ids::WorkspaceId;
    use aex_wire::types::Timestamp;

    use super::{
        ExportAuthority, ExportLease, ExportObjects, ExportOutcome, ExportProgress, ExportTask,
        LeaseOutcome, ObjectHead, Page, PartListing, Position, PublishRequest, TaskError,
        TaskSettings, artifact_key, member_name, next_part_number, object_prefix,
    };
    use crate::budget::MemoryPlan;

    const WORKSPACE: &str = "wsp_0000000001e40r2081040g2081";
    const EXPORT: &str = "exp_0000000001e40r2081040g2081";
    const PART_BYTES: usize = 5 * 1024 * 1024;
    const PAGE_LIMIT: usize = 4;
    const PREFIX: &str = "exports/wsp/exp";
    const KEY: &str = "exports/wsp/exp.jsonl";
    /// Enough pages that two full parts are flushed before the trailing one.
    const LONG_WALK: usize = 50;

    fn workspace() -> WorkspaceId {
        WORKSPACE.parse().expect("the fixture workspace parses")
    }

    fn now() -> Timestamp {
        Timestamp::from_unix_millis(1_785_000_000_000).expect("a fixture instant")
    }

    fn lease(format: Format) -> ExportLease {
        ExportLease {
            fence: 7,
            scope: ScopeKey::Workspace(workspace()),
            snapshot: 1_784_000_000_000,
            format,
            completeness: Completeness::AllowGaps,
            gaps: Vec::new(),
            pinned_deletion_epoch: 3,
            expires_at: now(),
            partitions: vec!["OBWA#wsp#logs#2026080100#00".to_owned()],
        }
    }

    fn plan() -> MemoryPlan {
        MemoryPlan::new(PAGE_LIMIT, PART_BYTES, 1024 * 1024)
    }

    fn settings(budget: &MemoryBudget) -> TaskSettings {
        TaskSettings {
            plan: plan(),
            budget: budget.clone(),
            page_limit: PAGE_LIMIT,
            part_bytes: PART_BYTES,
            object_prefix: PREFIX.into(),
        }
    }

    fn budget() -> MemoryBudget {
        MemoryBudget::new(plan().total_bytes())
    }

    /// A record just under the per-observation ceiling, so a handful of pages
    /// fills a real 5 MiB part.
    fn record(index: usize) -> Vec<u8> {
        let filler = "x".repeat(60 * 1024);
        format!(r#"{{"i":{index},"f":"{filler}"}}"#).into_bytes()
    }

    /// A walk of `pages` pages, each carrying a full page of records.
    fn walk(pages: usize) -> Vec<Page> {
        (0..pages)
            .map(|page| Page {
                records: (0..PAGE_LIMIT)
                    .map(|index| record(page * 10 + index))
                    .collect(),
                next_cursor: if page + 1 == pages {
                    None
                } else {
                    Some(format!("cursor-{}", page + 1).into())
                },
            })
            .collect()
    }

    fn count(value: usize) -> u64 {
        u64::try_from(value).expect("a fixture count fits")
    }

    #[derive(Default)]
    struct Recorder {
        pages: Mutex<Vec<Page>>,
        page_calls: AtomicUsize,
        cursors: Mutex<Vec<Option<String>>>,
        lease_calls: AtomicUsize,
        progress: Mutex<Vec<ExportProgress>>,
        reserved_during_pages: Mutex<Vec<usize>>,
    }

    struct FakeAuthority {
        outcome: LeaseOutcome,
        stored: Option<ExportProgress>,
        publication: Publication,
        budget: MemoryBudget,
        recorder: Recorder,
    }

    impl FakeAuthority {
        fn new(budget: &MemoryBudget, pages: Vec<Page>) -> Self {
            Self {
                outcome: LeaseOutcome::Taken(Box::new(lease(Format::Ndjson))),
                stored: None,
                publication: Publication::Ready,
                budget: budget.clone(),
                recorder: Recorder {
                    pages: Mutex::new(pages),
                    ..Recorder::default()
                },
            }
        }

        fn page_calls(&self) -> usize {
            self.recorder.page_calls.load(Ordering::Acquire)
        }

        fn lease_calls(&self) -> usize {
            self.recorder.lease_calls.load(Ordering::Acquire)
        }

        fn recorded_progress(&self) -> Vec<ExportProgress> {
            self.recorder
                .progress
                .lock()
                .expect("the recorder is not poisoned")
                .clone()
        }

        fn cursors(&self) -> Vec<Option<String>> {
            self.recorder
                .cursors
                .lock()
                .expect("the recorder is not poisoned")
                .clone()
        }

        fn reserved_during_pages(&self) -> Vec<usize> {
            self.recorder
                .reserved_during_pages
                .lock()
                .expect("the recorder is not poisoned")
                .clone()
        }
    }

    #[async_trait::async_trait]
    impl ExportAuthority for FakeAuthority {
        async fn take_lease(&self, _now: Timestamp) -> Result<LeaseOutcome, TaskError> {
            self.recorder.lease_calls.fetch_add(1, Ordering::AcqRel);
            Ok(self.outcome.clone())
        }

        async fn load_progress(&self, _fence: u64) -> Result<Option<ExportProgress>, TaskError> {
            Ok(self.stored.clone())
        }

        async fn record_progress(&self, progress: &ExportProgress) -> Result<(), TaskError> {
            self.recorder
                .progress
                .lock()
                .expect("the recorder is not poisoned")
                .push(progress.clone());
            Ok(())
        }

        async fn next_page(
            &self,
            _lease: &ExportLease,
            cursor: Option<&str>,
            _limit: usize,
        ) -> Result<Page, TaskError> {
            self.recorder.page_calls.fetch_add(1, Ordering::AcqRel);
            self.recorder
                .cursors
                .lock()
                .expect("the recorder is not poisoned")
                .push(cursor.map(ToOwned::to_owned));
            self.recorder
                .reserved_during_pages
                .lock()
                .expect("the recorder is not poisoned")
                .push(self.budget.reserved());
            let mut pages = self
                .recorder
                .pages
                .lock()
                .expect("the recorder is not poisoned");
            if pages.is_empty() {
                return Ok(Page::default());
            }
            Ok(pages.remove(0))
        }

        async fn publish(&self, _request: &PublishRequest) -> Result<Publication, TaskError> {
            Ok(self.publication)
        }
    }

    #[derive(Default)]
    struct FakeObjects {
        listing: PartListing,
        head: Option<ObjectHead>,
        uploaded: Mutex<Vec<(u32, Vec<u8>)>>,
        completed: AtomicUsize,
        aborted: AtomicUsize,
    }

    impl FakeObjects {
        fn uploads(&self) -> Vec<(u32, Vec<u8>)> {
            self.uploaded
                .lock()
                .expect("the recorder is not poisoned")
                .clone()
        }

        fn uploaded_bytes(&self) -> u64 {
            self.uploads()
                .iter()
                .map(|(_, body)| count(body.len()))
                .sum()
        }

        /// What the completed object holds: the parts a previous task already
        /// uploaded plus the ones this one sent.
        fn stored_bytes(&self) -> u64 {
            self.listing
                .parts
                .iter()
                .map(|part| part.bytes)
                .sum::<u64>()
                + self.uploaded_bytes()
        }

        fn parts(&self) -> usize {
            self.uploaded
                .lock()
                .expect("the recorder is not poisoned")
                .len()
        }

        fn last_body(&self) -> Vec<u8> {
            self.uploads()
                .last()
                .map(|(_, body)| body.clone())
                .unwrap_or_default()
        }

        fn aborts(&self) -> usize {
            self.aborted.load(Ordering::Acquire)
        }

        fn completions(&self) -> usize {
            self.completed.load(Ordering::Acquire)
        }
    }

    #[async_trait::async_trait]
    impl ExportObjects for FakeObjects {
        async fn create_upload(&self, _key: &str) -> Result<Box<str>, TaskError> {
            Ok("upload-1".into())
        }

        async fn upload_part(
            &self,
            _key: &str,
            _upload_id: &str,
            number: u32,
            body: Vec<u8>,
        ) -> Result<Box<str>, TaskError> {
            self.uploaded
                .lock()
                .expect("the recorder is not poisoned")
                .push((number, body));
            Ok(format!("etag-{number}").into())
        }

        async fn list_parts(&self, _key: &str, _upload_id: &str) -> Result<PartListing, TaskError> {
            Ok(self.listing.clone())
        }

        async fn complete_upload(
            &self,
            _key: &str,
            _upload_id: &str,
            _parts: &[PartRecord],
        ) -> Result<Box<str>, TaskError> {
            self.completed.fetch_add(1, Ordering::AcqRel);
            Ok("object-checksum".into())
        }

        async fn abort_upload(&self, _key: &str, _upload_id: &str) -> Result<(), TaskError> {
            self.aborted.fetch_add(1, Ordering::AcqRel);
            Ok(())
        }

        async fn head_object(&self, _key: &str) -> Result<ObjectHead, TaskError> {
            Ok(self.head.clone().unwrap_or(ObjectHead {
                bytes: self.stored_bytes(),
                checksum: "object-checksum".into(),
            }))
        }
    }

    fn task(
        authority: FakeAuthority,
        objects: FakeObjects,
        budget: &MemoryBudget,
    ) -> ExportTask<FakeAuthority, FakeObjects> {
        ExportTask::new(authority, objects, settings(budget))
    }

    #[tokio::test]
    async fn a_complete_export_writes_the_manifest_last_and_publishes() {
        let budget = budget();
        let task = task(
            FakeAuthority::new(&budget, walk(LONG_WALK)),
            FakeObjects::default(),
            &budget,
        );

        let outcome = task.run(now()).await.expect("the export publishes");
        let ExportOutcome::Published {
            object_bytes,
            records,
            parts,
            manifest_hash,
            object_key: key,
        } = outcome
        else {
            panic!("expected a published export, got {outcome:?}");
        };
        assert_eq!(records, count(LONG_WALK * PAGE_LIMIT));
        assert_eq!(key.as_ref(), KEY);
        assert_eq!(manifest_hash.len(), 64);
        assert!(parts >= 2, "at least one full part and the trailing one");

        assert_eq!(task.objects.parts(), usize::try_from(parts).expect("fits"));
        assert_eq!(task.objects.uploaded_bytes(), object_bytes);
        assert_eq!(task.objects.completions(), 1);
        assert_eq!(task.objects.aborts(), 0);

        // The manifest is the last thing written, and it names every member.
        let manifest = String::from_utf8(task.objects.last_body()).expect("the manifest is utf8");
        assert!(manifest.contains(r#""schemaRevision":1"#), "{manifest}");
        assert!(
            manifest.contains(&member_name(0, Format::Ndjson)),
            "{manifest}"
        );
        assert!(manifest.ends_with('\n'));

        // Only the final part may sit under the 5 MiB floor S3 enforces.
        let uploads = task.objects.uploads();
        let last = uploads.len() - 1;
        for (number, body) in uploads.iter().take(last) {
            assert!(
                body.len() >= PART_BYTES,
                "part {number} of {} bytes is under the floor",
                body.len()
            );
        }
    }

    #[tokio::test]
    async fn a_budget_that_cannot_cover_the_reservations_fails_before_the_producing_loop() {
        let budget = MemoryBudget::new(1);
        let task = task(
            FakeAuthority::new(&budget, walk(3)),
            FakeObjects::default(),
            &budget,
        );

        let error = task.run(now()).await.expect_err("a short budget refuses");
        assert_eq!(error.code(), Some(ErrorCode::ExportCapacity), "{error}");
        assert_eq!(task.authority.page_calls(), 0, "nothing was streamed");
        assert_eq!(task.authority.lease_calls(), 0, "nothing was even leased");
        assert_eq!(task.objects.parts(), 0);
    }

    #[tokio::test]
    async fn the_reserved_total_does_not_grow_with_the_number_of_pages_streamed() {
        let expected = plan().total_bytes();
        let mut first_observation = Vec::new();
        for pages in [1usize, 5, LONG_WALK] {
            let budget = budget();
            let task = task(
                FakeAuthority::new(&budget, walk(pages)),
                FakeObjects::default(),
                &budget,
            );
            task.run(now()).await.expect("the export publishes");

            let during = task.authority.reserved_during_pages();
            assert_eq!(during.len(), pages, "one observation per streamed page");
            for reserved in &during {
                assert_eq!(*reserved, expected, "the plan is flat across {pages} pages");
            }
            first_observation.push(during[0]);
        }
        assert!(
            first_observation.windows(2).all(|pair| pair[0] == pair[1]),
            "a multi-GB export holds exactly what a one-page export holds: {first_observation:?}"
        );
    }

    #[tokio::test]
    async fn losing_the_publication_fence_aborts_the_upload_and_exits_zero() {
        for reason in [
            "a cancel won",
            "a deletion won",
            "a lease takeover won",
            "the export is no longer generating",
        ] {
            let budget = budget();
            let mut authority = FakeAuthority::new(&budget, walk(2));
            authority.publication = Publication::Superseded { reason };
            let task = task(authority, FakeObjects::default(), &budget);

            let outcome = task
                .run(now())
                .await
                .expect("a superseded export is not an error");
            assert_eq!(outcome.superseded_by(), Some(reason));
            assert!(!outcome.is_published());
            assert_eq!(task.objects.aborts(), 1, "the upload is aborted explicitly");
        }
    }

    #[tokio::test]
    async fn losing_the_lease_exits_zero_without_reading_a_single_observation() {
        let budget = budget();
        let mut authority = FakeAuthority::new(&budget, walk(4));
        authority.outcome = LeaseOutcome::Lost {
            reason: "a cancel won",
        };
        let task = task(authority, FakeObjects::default(), &budget);

        let outcome = task.run(now()).await.expect("a lost lease is not an error");
        assert_eq!(outcome.superseded_by(), Some("a cancel won"));
        assert_eq!(task.authority.page_calls(), 0);
        assert_eq!(task.objects.parts(), 0);
        assert_eq!(task.objects.aborts(), 0, "there is no upload to abort");
    }

    fn checkpointed() -> ExportProgress {
        ExportProgress {
            checkpoint: ExportCheckpoint {
                fence: 7,
                upload_id: "upload-1".into(),
                parts: vec![PartRecord {
                    number: 1,
                    etag: "etag-1".into(),
                    sha256: "0".repeat(64).into(),
                    bytes: 5 << 20,
                }],
                cursor: "cursor-1".into(),
                member_ordinal: 1,
                member_checkpoint: 4,
                bytes_written: 5 << 20,
            },
            members: vec![ExportMember {
                ordinal: 0,
                name: "0000.jsonl".into(),
                bytes: 5 << 20,
                sha256: "0".repeat(64).into(),
                records: 4,
            }],
        }
    }

    fn resumable(listing: PartListing) -> FakeObjects {
        FakeObjects {
            listing,
            ..FakeObjects::default()
        }
    }

    #[tokio::test]
    async fn a_truncated_part_listing_is_a_hard_integrity_failure() {
        let budget = budget();
        let mut authority = FakeAuthority::new(&budget, walk(2));
        authority.stored = Some(checkpointed());
        let task = task(
            authority,
            resumable(PartListing {
                parts: checkpointed().checkpoint.parts,
                truncated_without_marker: true,
            }),
            &budget,
        );

        let error = task
            .run(now())
            .await
            .expect_err("a truncated listing refuses");
        assert!(
            matches!(error, TaskError::Resume(ResumeError::TruncatedListing)),
            "{error:?}"
        );
        assert_eq!(task.authority.page_calls(), 0, "nothing is streamed");
        assert_eq!(task.objects.completions(), 0);
    }

    #[tokio::test]
    async fn a_part_the_provider_reports_differently_is_a_hard_integrity_failure() {
        let budget = budget();
        let mut authority = FakeAuthority::new(&budget, walk(2));
        authority.stored = Some(checkpointed());
        let mut listed = checkpointed().checkpoint.parts;
        listed[0].etag = "someone-elses-etag".into();
        let task = task(
            authority,
            resumable(PartListing {
                parts: listed,
                truncated_without_marker: false,
            }),
            &budget,
        );

        let error = task
            .run(now())
            .await
            .expect_err("a mismatched part refuses");
        assert!(
            matches!(
                error,
                TaskError::Resume(ResumeError::IntegrityMismatch { number: 1, .. })
            ),
            "{error:?}"
        );
    }

    #[tokio::test]
    async fn a_resume_continues_from_the_checkpoint_cursor_and_keeps_earlier_parts() {
        let budget = budget();
        // The walk continues past the checkpoint's cursor rather than repeating
        // it, which is what a resumed read of the pinned snapshot returns.
        let remainder = vec![
            Page {
                records: (0..PAGE_LIMIT).map(record).collect(),
                next_cursor: Some("cursor-2".into()),
            },
            Page {
                records: (0..PAGE_LIMIT).map(record).collect(),
                next_cursor: None,
            },
        ];
        let mut authority = FakeAuthority::new(&budget, remainder);
        authority.stored = Some(checkpointed());
        let task = task(
            authority,
            resumable(PartListing {
                parts: checkpointed().checkpoint.parts,
                truncated_without_marker: false,
            }),
            &budget,
        );

        let outcome = task.run(now()).await.expect("a resumed export publishes");
        assert!(outcome.is_published());
        assert_eq!(
            task.authority
                .cursors()
                .first()
                .and_then(Clone::clone)
                .as_deref(),
            Some("cursor-1"),
            "the first read continues from the checkpoint"
        );
        assert_eq!(
            task.objects.uploads().first().map(|(number, _)| *number),
            Some(2),
            "part one is already uploaded and is not re-sent"
        );
        let manifest = String::from_utf8(task.objects.last_body()).expect("the manifest is utf8");
        for member in ["0000.jsonl", "0001.jsonl"] {
            assert!(
                manifest.contains(member),
                "the member ledger survives the resume: {manifest}"
            );
        }
    }

    #[tokio::test]
    async fn a_parquet_member_is_a_typed_refusal_rather_than_a_nondeterministic_artifact() {
        let budget = budget();
        let mut authority = FakeAuthority::new(&budget, walk(2));
        authority.outcome = LeaseOutcome::Taken(Box::new(lease(Format::Parquet)));
        let task = task(authority, FakeObjects::default(), &budget);

        let error = task.run(now()).await.expect_err("parquet is refused");
        assert!(
            matches!(
                error,
                TaskError::Encode(EncodeError::Unsupported {
                    format: "parquet",
                    ..
                })
            ),
            "{error:?}"
        );
        assert_eq!(task.objects.aborts(), 1, "the started upload is aborted");
        assert_eq!(task.objects.completions(), 0);
    }

    #[tokio::test]
    async fn a_require_export_over_an_open_gap_refuses_and_aborts() {
        let budget = budget();
        let mut authority = FakeAuthority::new(&budget, walk(1));
        let mut refused = lease(Format::Ndjson);
        refused.completeness = Completeness::Require;
        refused.gaps = vec!["gap_a".to_owned()];
        authority.outcome = LeaseOutcome::Taken(Box::new(refused));
        let task = task(authority, FakeObjects::default(), &budget);

        let error = task.run(now()).await.expect_err("an open gap refuses");
        assert!(matches!(error, TaskError::Manifest(_)), "{error:?}");
        assert_eq!(task.objects.aborts(), 1);
        assert_eq!(task.objects.completions(), 0);
    }

    #[tokio::test]
    async fn a_page_beyond_the_reservation_is_refused() {
        let budget = budget();
        let task = task(
            FakeAuthority::new(
                &budget,
                vec![Page {
                    records: (0..=PAGE_LIMIT).map(record).collect(),
                    next_cursor: None,
                }],
            ),
            FakeObjects::default(),
            &budget,
        );

        let error = task
            .run(now())
            .await
            .expect_err("an oversized page refuses");
        assert!(
            matches!(
                error,
                TaskError::PageOverrun {
                    limit: PAGE_LIMIT,
                    ..
                }
            ),
            "{error:?}"
        );
    }

    #[tokio::test]
    async fn a_record_beyond_the_per_observation_ceiling_is_refused() {
        let budget = budget();
        let task = task(
            FakeAuthority::new(
                &budget,
                vec![Page {
                    records: vec![vec![b'x'; 65 * 1024]],
                    next_cursor: None,
                }],
            ),
            FakeObjects::default(),
            &budget,
        );

        let error = task
            .run(now())
            .await
            .expect_err("an oversized record refuses");
        assert!(
            matches!(error, TaskError::RecordTooLarge { .. }),
            "{error:?}"
        );
    }

    #[tokio::test]
    async fn a_walk_that_cannot_progress_is_refused_rather_than_looped_on() {
        let budget = budget();
        let task = task(
            FakeAuthority::new(
                &budget,
                vec![
                    Page {
                        records: vec![record(0)],
                        next_cursor: Some("cursor-1".into()),
                    },
                    Page {
                        records: Vec::new(),
                        next_cursor: Some("cursor-1".into()),
                    },
                ],
            ),
            FakeObjects::default(),
            &budget,
        );

        let error = task.run(now()).await.expect_err("a stalled walk refuses");
        assert!(matches!(error, TaskError::Integrity { .. }), "{error:?}");
    }

    #[tokio::test]
    async fn an_object_whose_length_disagrees_with_the_upload_never_publishes() {
        let budget = budget();
        let objects = FakeObjects {
            head: Some(ObjectHead {
                bytes: 1,
                checksum: "object-checksum".into(),
            }),
            ..FakeObjects::default()
        };
        let task = task(FakeAuthority::new(&budget, walk(1)), objects, &budget);

        let error = task.run(now()).await.expect_err("a short object refuses");
        assert!(matches!(error, TaskError::Integrity { .. }), "{error:?}");
        assert_eq!(task.objects.aborts(), 1);
    }

    #[tokio::test]
    async fn an_object_whose_checksum_disagrees_with_the_upload_never_publishes() {
        let budget = budget();
        let objects = FakeObjects {
            head: Some(ObjectHead {
                bytes: 0,
                checksum: "someone-elses-checksum".into(),
            }),
            ..FakeObjects::default()
        };
        let task = task(FakeAuthority::new(&budget, walk(1)), objects, &budget);
        let error = task.run(now()).await.expect_err("a foreign object refuses");
        assert!(matches!(error, TaskError::Integrity { .. }), "{error:?}");
    }

    #[tokio::test]
    async fn every_full_part_is_checkpointed_under_the_fence_before_the_next_one() {
        let budget = budget();
        let task = task(
            FakeAuthority::new(&budget, walk(LONG_WALK)),
            FakeObjects::default(),
            &budget,
        );
        task.run(now()).await.expect("the export publishes");

        let progress = task.authority.recorded_progress();
        assert!(!progress.is_empty(), "at least one full part was flushed");
        for (index, recorded) in progress.iter().enumerate() {
            assert_eq!(recorded.checkpoint.fence, 7, "written under the held fence");
            assert_eq!(recorded.checkpoint.parts.len(), index + 1);
            assert_eq!(recorded.members.len(), index + 1);
            assert_eq!(
                recorded.checkpoint.upload_id.as_ref(),
                "upload-1",
                "the upload identity is durable"
            );
            assert!(
                !recorded.checkpoint.cursor.is_empty(),
                "a mid-walk checkpoint carries the cursor to resume from"
            );
            assert!(recorded.checkpoint.bytes_written > 0);
        }
    }

    #[test]
    fn the_stored_cursor_round_trips_and_an_empty_one_means_exhausted() {
        assert_eq!(Position::from_stored(""), Position::Exhausted);
        assert_eq!(
            Position::from_stored("cursor-9"),
            Position::After("cursor-9".into())
        );
        assert_eq!(Position::Exhausted.to_stored().as_ref(), "");
        assert_eq!(Position::Start.as_cursor(), None);
        assert_eq!(Position::Exhausted.as_cursor(), None);
        assert_eq!(
            Position::After("cursor-9".into()).to_stored().as_ref(),
            "cursor-9"
        );
    }

    #[test]
    fn the_object_key_and_member_names_come_from_the_leased_format() {
        let prefix = object_prefix(
            workspace(),
            EXPORT.parse().expect("the fixture export parses"),
        );
        assert_eq!(prefix, format!("exports/{WORKSPACE}/{EXPORT}"));
        assert_eq!(
            artifact_key(&prefix, Format::Ndjson),
            format!("exports/{WORKSPACE}/{EXPORT}.jsonl")
        );
        assert_eq!(artifact_key(PREFIX, Format::Ndjson), KEY);
        assert_eq!(member_name(0, Format::Ndjson), "0000.jsonl");
        assert_eq!(member_name(12, Format::Parquet), "0012.parquet");
    }

    #[test]
    fn the_part_number_ceiling_is_the_providers() {
        assert_eq!(next_part_number(0).expect("first part"), 1);
        assert_eq!(next_part_number(9_999).expect("last part"), 10_000);
        assert!(matches!(
            next_part_number(10_000),
            Err(TaskError::PartLimit { limit: 10_000 })
        ));
    }
}
