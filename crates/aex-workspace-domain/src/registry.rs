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
use aex_wire::ids::{UploadId, WorkspaceId};
use aex_wire::types::{ETag, Timestamp};

use crate::upload::UploadState;

/// What a registry name points at.
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

/// A registry pointer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegistryPointer {
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
    /// What it points at.
    pub value: RegisteredValueRef,
    /// The canonical digest of the value.
    pub sha256: ContentDigest,
    /// How large the value is.
    pub size_bytes: u64,
    /// When the pointer was created.
    pub created_at: Timestamp,
    /// When it last changed.
    pub updated_at: Timestamp,
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
    /// What it points at.
    pub value: RegisteredValueRef,
    /// The canonical digest of the value.
    pub sha256: ContentDigest,
    /// How large the value is.
    pub size_bytes: u64,
    /// The state of the upload being consumed, when one is.
    pub upload_state: Option<UploadState>,
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
        Some(pointer) if pointer.etag == *expected => Ok(()),
        Some(pointer) => Err(RegistryRejection::PreconditionFailed {
            current: Some(pointer.etag.clone()),
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
        && existing.workspace != proposed.workspace
    {
        return Err(RegistryRejection::InvalidValue(ValueError::WrongWorkspace));
    }
    if let RegisteredValueRef::Upload { .. } = proposed.value {
        match proposed.upload_state {
            Some(UploadState::Ready) => {}
            Some(state) => return Err(RegistryRejection::UploadNotReady(state)),
            None => return Err(RegistryRejection::UploadNotReady(UploadState::Created)),
        }
    }

    let consumed_upload = match &proposed.value {
        RegisteredValueRef::Upload { upload } => Some(*upload),
        RegisteredValueRef::Content { .. } => None,
    };

    let Some(existing) = current else {
        let revision = Revision::FIRST;
        return Ok((
            SetOutcome::Created,
            RegistryCommit {
                pointer: RegistryPointer {
                    workspace: proposed.workspace,
                    kind: proposed.kind,
                    name: proposed.name.clone(),
                    revision,
                    etag: etag_of(proposed.kind, revision, &proposed.sha256),
                    value: proposed.value.clone(),
                    sha256: proposed.sha256,
                    size_bytes: proposed.size_bytes,
                    created_at: now,
                    updated_at: now,
                },
                consumed_upload,
                wrote: true,
            },
        ));
    };

    // `Unchanged` requires an identical canonical digest *and* identical
    // non-content metadata. It writes nothing at all, so a concurrent editor's
    // `If-Match` survives an idempotent retry.
    if existing.sha256 == proposed.sha256
        && existing.size_bytes == proposed.size_bytes
        && existing.kind == proposed.kind
        && existing.name == proposed.name
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

    let revision = existing.revision.next();
    Ok((
        SetOutcome::Replaced,
        RegistryCommit {
            pointer: RegistryPointer {
                revision,
                etag: etag_of(proposed.kind, revision, &proposed.sha256),
                value: proposed.value.clone(),
                sha256: proposed.sha256,
                size_bytes: proposed.size_bytes,
                updated_at: now,
                ..existing.clone()
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
        ProposedValue, RegisteredValueRef, RegistryRejection, SetOutcome, delete, etag_of, set,
    };

    fn workspace() -> WorkspaceId {
        WorkspaceId::from_uuid7(Uuid7::compose(1, [1; 10]))
    }

    fn moment(millis: i64) -> Timestamp {
        Timestamp::from_unix_millis(millis).expect("in range")
    }

    fn proposed(body: &[u8]) -> ProposedValue {
        ProposedValue {
            workspace: workspace(),
            kind: RegistryKind::File,
            name: RegisteredName::parse("readme").expect("valid"),
            value: RegisteredValueRef::Content {
                digest: ContentDigest::of(body),
            },
            sha256: ContentDigest::of(body),
            size_bytes: body.len() as u64,
            upload_state: None,
        }
    }

    #[test]
    fn an_identical_set_writes_nothing_at_all() {
        let (outcome, created) = set(None, &proposed(b"one"), None, moment(0)).expect("creates");
        assert_eq!(outcome, SetOutcome::Created);
        assert_eq!(created.pointer.revision, Revision::FIRST);

        let (outcome, unchanged) =
            set(Some(&created.pointer), &proposed(b"one"), None, moment(10)).expect("unchanged");
        assert_eq!(outcome, SetOutcome::Unchanged);
        assert!(!unchanged.wrote);
        assert_eq!(unchanged.pointer.revision, created.pointer.revision);
        assert_eq!(unchanged.pointer.etag, created.pointer.etag);
        assert_eq!(unchanged.pointer.updated_at, created.pointer.updated_at);
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
        assert_eq!(replaced.pointer.revision, created.revision.next());
        assert_ne!(replaced.pointer.etag, created.etag);
        assert_eq!(replaced.pointer.created_at, created.created_at);
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
                Some(&created.etag),
                moment(1)
            )
            .is_ok()
        );

        let stale = etag_of(RegistryKind::File, Revision(99), &ContentDigest::of(b"x"));
        assert_eq!(
            set(Some(&created), &proposed(b"three"), Some(&stale), moment(2)),
            Err(RegistryRejection::PreconditionFailed {
                current: Some(created.etag.clone())
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
