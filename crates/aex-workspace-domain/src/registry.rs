//! The named registry.
//!
//! One current value per `(workspace, kind, name)`. `set` reports
//! `Created | Replaced | Unchanged`, and `Unchanged` leaves the revision, the
//! `updated_at` and the `ETag` **untouched** (D-08): an idempotent retry must not
//! invalidate every concurrent editor's `If-Match`.
//!
//! `revision` is a strong monotone concurrency token, not a version. No route
//! reads, lists, restores or copies a prior value, so there is no history to
//! garbage-collect and no way to resurrect a deleted one.

use aex_content_domain::{ContentDigest, RegisteredName, RegistryKind, Revision};
use aex_wire::CanonicalJson;
use aex_wire::ids::{UploadId, WorkspaceId};
use aex_wire::types::{ETag, Timestamp};

use crate::upload::UploadState;

/// Private lifecycle carried by the single current registry pointer.
///
/// Only files use asynchronous admission in the launch surface, but keeping the
/// state beside the pointer (rather than inferring it from a value document)
/// lets list reads remain one projected `DynamoDB` query. The revision remains a
/// private stale-completion fence and is never a public file version.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum RegistryState {
    /// An asynchronous source was admitted and prior bytes are no longer current.
    Pending,
    /// The current value has verified immutable bytes.
    Ready,
    /// The current asynchronous source failed under a stable bounded code.
    Failed,
}

/// Where a registered value's payload bytes come from on the way **in**.
///
/// This is an input vocabulary only. A durable pointer never stores it: a set
/// that consumes an upload resolves it to a content digest at admission time and
/// stores that digest inside the value document (D-15). A pointer that named an
/// upload would outlive its own referent, because `upload_complete` marks the
/// upload `Consumed` and the row eventually ages out.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RegisteredValueRef {
    /// A body already in content storage.
    Content {
        /// Which body.
        digest: ContentDigest,
    },
    /// A completed upload being consumed by this set.
    Upload {
        /// Which upload.
        upload: UploadId,
    },
}

/// The complete non-payload value of a registered resource, canonicalized.
///
/// The document is the resource: `sha256` and `size_bytes` on a pointer are
/// `SHA-256(JCS(valueDoc))` and `len(JCS(valueDoc))` for every kind, including
/// the two kinds that have no payload at all (D-4). Digesting the payload
/// instead would make a metadata-only edit — a changed `mountPath` over
/// unchanged bytes — compare `Unchanged` and be silently discarded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValueDocument(CanonicalJson);

impl ValueDocument {
    /// Adopts an already-canonical document.
    #[must_use]
    pub const fn new(canonical: CanonicalJson) -> Self {
        Self(canonical)
    }

    /// The canonical document.
    #[must_use]
    pub const fn canonical(&self) -> &CanonicalJson {
        &self.0
    }

    /// The canonical text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }

    /// The canonical bytes.
    #[must_use]
    pub fn as_bytes(&self) -> &[u8] {
        self.0.as_bytes()
    }

    /// `SHA-256(JCS(valueDoc))`.
    #[must_use]
    pub fn digest(&self) -> ContentDigest {
        ContentDigest::of(self.as_bytes())
    }

    /// `len(JCS(valueDoc))`.
    #[must_use]
    pub fn size_bytes(&self) -> u64 {
        self.as_bytes().len() as u64
    }
}

/// Everything about a registry pointer except its value document.
///
/// A listing projects exactly these attributes and never reads `valueDoc`, so a
/// thousand-row page cannot carry a thousand value documents. The split is the
/// storage-side twin of the wire split at D-6: a collection row has no field a
/// value could go in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegistryRow {
    /// The owning workspace.
    pub workspace: WorkspaceId,
    /// Which registry.
    pub kind: RegistryKind,
    /// Which name.
    pub name: RegisteredName,
    /// The concurrency token.
    pub revision: Revision,
    /// The entity tag derived from it.
    pub etag: ETag,
    /// `SHA-256(JCS(valueDoc))`.
    pub sha256: ContentDigest,
    /// `len(JCS(valueDoc))`.
    pub size_bytes: u64,
    /// Lifecycle of this one current overwrite.
    pub state: RegistryState,
    /// Stable bounded failure code, present only in [`RegistryState::Failed`].
    pub failure_code: Option<String>,
    /// When the pointer was created.
    pub created_at: Timestamp,
    /// When it last changed.
    pub updated_at: Timestamp,
}

/// A registry pointer, complete.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegistryPointer {
    /// The collection columns.
    pub row: RegistryRow,
    /// The complete non-payload value.
    pub value_doc: ValueDocument,
}

/// What a caller proposes to store.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProposedValue {
    /// The owning workspace.
    pub workspace: WorkspaceId,
    /// Which registry.
    pub kind: RegistryKind,
    /// Which name.
    pub name: RegisteredName,
    /// The complete non-payload value.
    pub value_doc: ValueDocument,
    /// Where the payload came from, for the kinds that have one.
    pub payload: Option<RegisteredValueRef>,
    /// The state of the upload being consumed, when one is.
    pub upload_state: Option<UploadState>,
    /// Lifecycle the new current pointer publishes.
    pub state: RegistryState,
    /// Stable bounded failure code, present only for a failed proposal.
    pub failure_code: Option<String>,
}

impl ProposedValue {
    /// `SHA-256(JCS(valueDoc))`.
    #[must_use]
    pub fn sha256(&self) -> ContentDigest {
        self.value_doc.digest()
    }

    /// `len(JCS(valueDoc))`.
    #[must_use]
    pub fn size_bytes(&self) -> u64 {
        self.value_doc.size_bytes()
    }
}

/// The deterministic entity tag of a pointer.
///
/// Pure in `(kind, revision, digest)`, so an adapter can never invent one and two
/// adapters can never disagree about the tag of the same pointer.
#[must_use]
pub fn etag_of(kind: RegistryKind, revision: Revision, digest: &ContentDigest) -> ETag {
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"aex.registry.etag.v1");
    hasher.update(&[kind.discriminant()]);
    hasher.update(&revision.0.to_le_bytes());
    hasher.update(digest.as_bytes());
    let bytes = hasher.finalize();
    let mut rendered = String::with_capacity(32);
    for byte in &bytes.as_bytes()[..16] {
        rendered.push(char::from(hex_digit(byte >> 4)));
        rendered.push(char::from(hex_digit(byte & 0x0f)));
    }
    ETag::parse(&rendered).unwrap_or_else(|_| unreachable!("32 hex characters is a valid ETag"))
}

const fn hex_digit(value: u8) -> u8 {
    match value {
        0..=9 => b'0' + value,
        _ => b'a' + value - 10,
    }
}

/// What a `set` did.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum SetOutcome {
    /// The name was free.
    Created,
    /// The value changed.
    Replaced,
    /// The value was already exactly this.
    Unchanged,
}

/// The pointer a `set` results in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegistryCommit {
    /// The pointer after the change.
    pub pointer: RegistryPointer,
    /// The upload this set consumes, when one is consumed.
    pub consumed_upload: Option<UploadId>,
    /// Whether anything was written at all.
    pub wrote: bool,
}

/// The pointer a `delete` removes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeleteCommit {
    /// The pointer that was removed, when one existed.
    pub removed: Option<RegistryPointer>,
    /// Whether anything was written at all. Delete is idempotent.
    pub wrote: bool,
}

/// Why a value was rejected.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum ValueError {
    /// The value's declared size and digest disagree with the stored body.
    #[error("declared value does not match the stored body")]
    DigestMismatch,
    /// The value named a different workspace.
    #[error("value belongs to another workspace")]
    WrongWorkspace,
}

/// Why a registry change was refused.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RegistryRejection {
    /// An `If-Match` did not match.
    #[error("precondition failed")]
    PreconditionFailed {
        /// The tag the pointer actually carries, when it exists.
        current: Option<ETag>,
    },
    /// The proposed value is not storable.
    #[error(transparent)]
    InvalidValue(#[from] ValueError),
    /// The upload being consumed is not `Ready`.
    #[error("upload is {0:?}, not ready")]
    UploadNotReady(UploadState),
}

fn precondition(
    current: Option<&RegistryPointer>,
    if_match: Option<&ETag>,
) -> Result<(), RegistryRejection> {
    let Some(expected) = if_match else {
        // `If-Match` is optional; without it replacement is last-writer-wins.
        return Ok(());
    };
    match current {
        Some(pointer) if pointer.row.etag == *expected => Ok(()),
        Some(pointer) => Err(RegistryRejection::PreconditionFailed {
            current: Some(pointer.row.etag.clone()),
        }),
        None => Err(RegistryRejection::PreconditionFailed { current: None }),
    }
}

/// Creates or replaces a registry pointer.
///
/// # Errors
///
/// Returns [`RegistryRejection`] on a failed `If-Match`, a value that does not
/// belong to the workspace, or an upload that is not `Ready`.
pub fn set(
    current: Option<&RegistryPointer>,
    proposed: &ProposedValue,
    if_match: Option<&ETag>,
    now: Timestamp,
) -> Result<(SetOutcome, RegistryCommit), RegistryRejection> {
    precondition(current, if_match)?;

    if let Some(existing) = current
        && existing.row.workspace != proposed.workspace
    {
        return Err(RegistryRejection::InvalidValue(ValueError::WrongWorkspace));
    }
    if let Some(RegisteredValueRef::Upload { .. }) = proposed.payload {
        match proposed.upload_state {
            Some(UploadState::Ready) => {}
            Some(state) => return Err(RegistryRejection::UploadNotReady(state)),
            None => return Err(RegistryRejection::UploadNotReady(UploadState::Created)),
        }
    }

    let consumed_upload = match &proposed.payload {
        Some(RegisteredValueRef::Upload { upload }) => Some(*upload),
        Some(RegisteredValueRef::Content { .. }) | None => None,
    };
    let sha256 = proposed.sha256();
    let size_bytes = proposed.size_bytes();

    let Some(existing) = current else {
        let revision = Revision::FIRST;
        return Ok((
            SetOutcome::Created,
            RegistryCommit {
                pointer: RegistryPointer {
                    row: RegistryRow {
                        workspace: proposed.workspace,
                        kind: proposed.kind,
                        name: proposed.name.clone(),
                        revision,
                        etag: etag_of(proposed.kind, revision, &sha256),
                        sha256,
                        size_bytes,
                        state: proposed.state,
                        failure_code: proposed.failure_code.clone(),
                        created_at: now,
                        updated_at: now,
                    },
                    value_doc: proposed.value_doc.clone(),
                },
                consumed_upload,
                wrote: true,
            },
        ));
    };

    // `Unchanged` requires an identical canonical *value document*, which is
    // what `sha256` digests since D-4. It writes nothing at all, so a concurrent
    // editor's `If-Match` survives an idempotent retry — and a metadata-only
    // edit over unchanged payload bytes correctly reports `Replaced` rather than
    // being silently discarded.
    if existing.row.sha256 == sha256
        && existing.row.size_bytes == size_bytes
        && existing.row.kind == proposed.kind
        && existing.row.name == proposed.name
    {
        return Ok((
            SetOutcome::Unchanged,
            RegistryCommit {
                pointer: existing.clone(),
                consumed_upload: None,
                wrote: false,
            },
        ));
    }

    let revision = existing.row.revision.next();
    Ok((
        SetOutcome::Replaced,
        RegistryCommit {
            pointer: RegistryPointer {
                row: RegistryRow {
                    revision,
                    etag: etag_of(proposed.kind, revision, &sha256),
                    sha256,
                    size_bytes,
                    state: proposed.state,
                    failure_code: proposed.failure_code.clone(),
                    updated_at: now,
                    ..existing.row.clone()
                },
                value_doc: proposed.value_doc.clone(),
            },
            consumed_upload,
            wrote: true,
        },
    ))
}

/// Removes a registry pointer.
///
/// Idempotent: deleting an absent name reports `wrote = false`.
///
/// # Errors
///
/// Returns [`RegistryRejection::PreconditionFailed`] on a failed `If-Match`.
pub fn delete(
    current: Option<&RegistryPointer>,
    if_match: Option<&ETag>,
) -> Result<DeleteCommit, RegistryRejection> {
    precondition(current, if_match)?;
    Ok(DeleteCommit {
        removed: current.cloned(),
        wrote: current.is_some(),
    })
}

#[cfg(test)]
mod tests {
    use aex_content_domain::{ContentDigest, RegisteredName, RegistryKind, Revision};
    use aex_wire::ids::{PrefixedId as _, Uuid7, WorkspaceId};
    use aex_wire::types::Timestamp;

    use super::{
        ProposedValue, RegisteredValueRef, RegistryRejection, RegistryState, SetOutcome,
        ValueDocument, delete, etag_of, set,
    };

    fn workspace() -> WorkspaceId {
        WorkspaceId::from_uuid7(Uuid7::compose(1, [1; 10]))
    }

    fn moment(millis: i64) -> Timestamp {
        Timestamp::from_unix_millis(millis).expect("in range")
    }

    fn document(mount_path: &str, body: &[u8]) -> ValueDocument {
        let digest = ContentDigest::of(body);
        ValueDocument::new(
            aex_wire::CanonicalJson::parse(&format!(
                r#"{{"mountPath":"{mount_path}","mediaType":"text/plain","mode":"0644",
                    "content":{{"sha256":"{}","sizeBytes":"{}"}}}}"#,
                digest.to_wire(),
                body.len()
            ))
            .expect("valid JSON"),
        )
    }

    fn proposed(body: &[u8]) -> ProposedValue {
        proposed_at("/readme", body)
    }

    fn proposed_at(mount_path: &str, body: &[u8]) -> ProposedValue {
        ProposedValue {
            workspace: workspace(),
            kind: RegistryKind::File,
            name: RegisteredName::parse("readme").expect("valid"),
            value_doc: document(mount_path, body),
            payload: Some(RegisteredValueRef::Content {
                digest: ContentDigest::of(body),
            }),
            upload_state: None,
            state: RegistryState::Ready,
            failure_code: None,
        }
    }

    #[test]
    fn an_identical_set_writes_nothing_at_all() {
        let (outcome, created) = set(None, &proposed(b"one"), None, moment(0)).expect("creates");
        assert_eq!(outcome, SetOutcome::Created);
        assert_eq!(created.pointer.row.revision, Revision::FIRST);

        let (outcome, unchanged) =
            set(Some(&created.pointer), &proposed(b"one"), None, moment(10)).expect("unchanged");
        assert_eq!(outcome, SetOutcome::Unchanged);
        assert!(!unchanged.wrote);
        assert_eq!(unchanged.pointer.row.revision, created.pointer.row.revision);
        assert_eq!(unchanged.pointer.row.etag, created.pointer.row.etag);
        assert_eq!(
            unchanged.pointer.row.updated_at,
            created.pointer.row.updated_at
        );
    }

    #[test]
    fn a_metadata_only_edit_replaces_rather_than_being_silently_discarded() {
        // The payload bytes are byte-identical; only `mountPath` moved. Digesting
        // the payload instead of the value document would report `Unchanged` here
        // and throw the customer's edit away (D-4).
        let created = set(None, &proposed_at("/a", b"one"), None, moment(0))
            .expect("creates")
            .1
            .pointer;
        let (outcome, replaced) =
            set(Some(&created), &proposed_at("/b", b"one"), None, moment(10)).expect("replaces");
        assert_eq!(outcome, SetOutcome::Replaced);
        assert!(replaced.wrote);
        assert_eq!(replaced.pointer.row.revision, created.row.revision.next());
        assert_ne!(replaced.pointer.value_doc, created.value_doc);
    }

    #[test]
    fn the_digest_and_the_size_are_the_canonical_documents_own() {
        let proposal = proposed(b"one");
        let created = set(None, &proposal, None, moment(0))
            .expect("creates")
            .1
            .pointer;
        assert_eq!(created.row.sha256, created.value_doc.digest());
        assert_eq!(created.row.size_bytes, created.value_doc.size_bytes());
        assert_eq!(
            created.row.size_bytes,
            created.value_doc.as_str().len() as u64
        );
        // The payload's own digest is *inside* the document, never the row's.
        assert_ne!(created.row.sha256, ContentDigest::of(b"one"));
    }

    #[test]
    fn a_different_value_advances_the_revision_by_exactly_one() {
        let created = set(None, &proposed(b"one"), None, moment(0))
            .expect("creates")
            .1
            .pointer;
        let (outcome, replaced) =
            set(Some(&created), &proposed(b"two"), None, moment(10)).expect("replaces");
        assert_eq!(outcome, SetOutcome::Replaced);
        assert_eq!(replaced.pointer.row.revision, created.row.revision.next());
        assert_ne!(replaced.pointer.row.etag, created.row.etag);
        assert_eq!(replaced.pointer.row.created_at, created.row.created_at);
    }

    #[test]
    fn if_match_is_optional_but_exact_when_present() {
        let created = set(None, &proposed(b"one"), None, moment(0))
            .expect("creates")
            .1
            .pointer;
        assert!(
            set(
                Some(&created),
                &proposed(b"two"),
                Some(&created.row.etag),
                moment(1)
            )
            .is_ok()
        );

        let stale = etag_of(RegistryKind::File, Revision(99), &ContentDigest::of(b"x"));
        assert_eq!(
            set(Some(&created), &proposed(b"three"), Some(&stale), moment(2)),
            Err(RegistryRejection::PreconditionFailed {
                current: Some(created.row.etag.clone())
            })
        );
        assert_eq!(
            delete(None, Some(&stale)),
            Err(RegistryRejection::PreconditionFailed { current: None })
        );
    }

    #[test]
    fn delete_is_idempotent() {
        let created = set(None, &proposed(b"one"), None, moment(0))
            .expect("creates")
            .1
            .pointer;
        let removed = delete(Some(&created), None).expect("deletes");
        assert!(removed.wrote);
        let again = delete(None, None).expect("idempotent");
        assert!(!again.wrote);
    }

    #[test]
    fn the_etag_is_deterministic_in_its_three_inputs() {
        let digest = ContentDigest::of(b"one");
        let base = etag_of(RegistryKind::File, Revision(3), &digest);
        assert_eq!(base, etag_of(RegistryKind::File, Revision(3), &digest));
        assert_ne!(base, etag_of(RegistryKind::Skill, Revision(3), &digest));
        assert_ne!(base, etag_of(RegistryKind::File, Revision(4), &digest));
        assert_ne!(
            base,
            etag_of(RegistryKind::File, Revision(3), &ContentDigest::of(b"two"))
        );
    }
}
