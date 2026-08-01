//! Multipart upload staging.
//!
//! `Created -> PartsGranted -> Completing -> Ready -> Consumed`, with `Aborted`
//! reachable only from the two earlier states and `Expired` from anything before
//! completion. `Consumed` is terminal and reachable **only** from a registry
//! `set` that pins the upload in the same transaction, so an upload is
//! single-use by construction rather than by convention.

use aex_content_domain::{ContentDigest, RegisteredName, RegistryKind};
use aex_wire::ids::{UploadId, WorkspaceId};
use aex_wire::types::Timestamp;
use time::Duration;

/// How long a staged upload survives without completing.
pub const UPLOAD_GRACE: Duration = Duration::hours(24);

/// How long a part grant is valid.
pub const PART_GRANT_TTL: Duration = Duration::minutes(5);

/// Smallest non-final part.
pub const PART_MIN_BYTES: u64 = 5 * 1024 * 1024;

/// Largest part.
pub const PART_MAX_BYTES: u64 = 5 * 1024 * 1024 * 1024;

/// Largest number of parts.
pub const PART_MAX_COUNT: u32 = 10_000;

/// Where an upload is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum UploadState {
    /// Staged, no parts granted.
    Created,
    /// Part grants issued.
    PartsGranted,
    /// Completion started.
    Completing,
    /// Verified and usable.
    Ready,
    /// Pinned by a registry set.
    Consumed,
    /// Abandoned by the caller.
    Aborted,
    /// Timed out.
    Expired,
}

impl UploadState {
    /// Every state, in lifecycle order.
    pub const ALL: [Self; 7] = [
        Self::Created,
        Self::PartsGranted,
        Self::Completing,
        Self::Ready,
        Self::Consumed,
        Self::Aborted,
        Self::Expired,
    ];

    /// Whether no transition leaves this state.
    #[must_use]
    pub const fn is_terminal(self) -> bool {
        matches!(self, Self::Consumed | Self::Aborted | Self::Expired)
    }

    /// Whether an explicit abort is still accepted.
    #[must_use]
    pub const fn can_abort(self) -> bool {
        matches!(self, Self::Created | Self::PartsGranted)
    }
}

/// Which registry entry consumed an upload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegistrySelector {
    /// The owning workspace.
    pub workspace: WorkspaceId,
    /// Which registry.
    pub kind: RegistryKind,
    /// Which name.
    pub name: RegisteredName,
}

/// One planned part.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PlannedPart {
    /// One-based part number.
    pub number: u32,
    /// How many bytes it carries.
    pub bytes: u64,
}

/// The whole part plan.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PartPlan {
    /// The parts, ascending and contiguous.
    pub parts: Vec<PlannedPart>,
}

impl PartPlan {
    /// The total the plan covers.
    #[must_use]
    pub fn total_bytes(&self) -> u64 {
        self.parts.iter().map(|part| part.bytes).sum()
    }
}

/// A part the caller says it uploaded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PartReceipt {
    /// Which part.
    pub number: u32,
    /// How many bytes it carried.
    pub bytes: u64,
}

/// What the object store says about the assembled object.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifiedObject {
    /// The verified size.
    pub size_bytes: u64,
    /// The verified digest.
    pub sha256: ContentDigest,
}

/// One staged upload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Upload {
    /// Its identity.
    pub id: UploadId,
    /// The owning workspace.
    pub workspace: WorkspaceId,
    /// Where it is.
    pub state: UploadState,
    /// The size the caller declared.
    pub declared_size: u64,
    /// The digest the caller declared.
    pub declared_sha256: ContentDigest,
    /// The media type the caller declared.
    pub content_type: Option<String>,
    /// The plan.
    pub parts: PartPlan,
    /// Which registry entry consumed it.
    pub consumed_by: Option<RegistrySelector>,
    /// When it was staged.
    pub created_at: Timestamp,
    /// When it lapses.
    pub expires_at: Timestamp,
}

/// The upload after a transition.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UploadCommit {
    /// The upload after the transition.
    pub upload: Upload,
    /// The parts a grant covers, when the transition granted any.
    pub granted: Vec<u32>,
}

/// Why an upload transition was refused.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum UploadError {
    /// The transition is not legal from the current state.
    #[error("upload is {from:?}")]
    WrongState {
        /// The current state.
        from: UploadState,
    },
    /// The upload has already been consumed.
    #[error("upload was already consumed by a registry entry")]
    AlreadyConsumed(Box<RegistrySelector>),
    /// The verified size disagrees with the declared one.
    #[error("declared {declared} bytes but verified {verified}")]
    SizeMismatch {
        /// What was declared.
        declared: u64,
        /// What was verified.
        verified: u64,
    },
    /// The verified digest disagrees with the declared one.
    #[error("declared and verified digests differ")]
    DigestMismatch {
        /// What was declared.
        declared: ContentDigest,
        /// What was verified.
        verified: ContentDigest,
    },
    /// The receipts were not contiguous and ascending.
    #[error("part receipts must be contiguous and ascending")]
    PartsNotContiguous,
    /// A part fell outside its allowed size range.
    #[error("part {part} of {bytes} bytes is out of range")]
    PartSizeOutOfRange {
        /// Which part.
        part: u32,
        /// How many bytes it carried.
        bytes: u64,
    },
    /// The object was empty or larger than the plan allows.
    #[error("an object of {bytes} bytes cannot be planned")]
    Unplannable {
        /// The requested size.
        bytes: u64,
    },
}

/// Plans the parts for an object of a given size.
///
/// Every part but the last carries [`PART_MIN_BYTES`]..=[`PART_MAX_BYTES`]; the
/// final part has no minimum.
///
/// # Errors
///
/// Returns [`UploadError::Unplannable`] for an empty object or one that would
/// need more than [`PART_MAX_COUNT`] parts.
pub fn plan_parts(size: u64) -> Result<PartPlan, UploadError> {
    if size == 0 {
        return Err(UploadError::Unplannable { bytes: size });
    }
    if size > PART_MAX_BYTES * u64::from(PART_MAX_COUNT) {
        return Err(UploadError::Unplannable { bytes: size });
    }
    if size <= PART_MIN_BYTES {
        return Ok(PartPlan {
            parts: vec![PlannedPart {
                number: 1,
                bytes: size,
            }],
        });
    }

    // Choose the smallest legal part size that keeps the count inside the bound.
    let mut part_bytes = PART_MIN_BYTES;
    while size.div_ceil(part_bytes) > u64::from(PART_MAX_COUNT) {
        part_bytes = part_bytes.saturating_mul(2).min(PART_MAX_BYTES);
    }

    let mut parts = Vec::new();
    let mut remaining = size;
    let mut number = 1_u32;
    while remaining > 0 {
        let bytes = remaining.min(part_bytes);
        parts.push(PlannedPart { number, bytes });
        remaining -= bytes;
        number += 1;
    }
    Ok(PartPlan { parts })
}

/// Issues grants for the named parts.
///
/// # Errors
///
/// Returns [`UploadError::WrongState`] once the upload has moved past
/// `PartsGranted`.
pub fn grant_parts(
    upload: &Upload,
    numbers: &[u32],
    now: Timestamp,
) -> Result<UploadCommit, UploadError> {
    let _ = now;
    if !matches!(
        upload.state,
        UploadState::Created | UploadState::PartsGranted
    ) {
        return Err(UploadError::WrongState { from: upload.state });
    }
    let mut granted: Vec<u32> = numbers
        .iter()
        .copied()
        .filter(|number| upload.parts.parts.iter().any(|part| part.number == *number))
        .collect();
    granted.sort_unstable();
    granted.dedup();
    Ok(UploadCommit {
        upload: Upload {
            state: UploadState::PartsGranted,
            ..upload.clone()
        },
        granted,
    })
}

/// Begins completion against the caller's part receipts.
///
/// # Errors
///
/// Returns [`UploadError`] when the upload is in the wrong state, or when the
/// receipts are not contiguous, ascending and inside their size range.
pub fn begin_complete(
    upload: &Upload,
    receipts: &[PartReceipt],
    now: Timestamp,
) -> Result<UploadCommit, UploadError> {
    let _ = now;
    if !matches!(
        upload.state,
        UploadState::Created | UploadState::PartsGranted
    ) {
        return Err(UploadError::WrongState { from: upload.state });
    }
    if receipts.is_empty() || receipts.len() > PART_MAX_COUNT as usize {
        return Err(UploadError::PartsNotContiguous);
    }
    for (index, receipt) in receipts.iter().enumerate() {
        let expected = u32::try_from(index + 1).unwrap_or(u32::MAX);
        if receipt.number != expected {
            return Err(UploadError::PartsNotContiguous);
        }
        let is_last = index + 1 == receipts.len();
        let too_small = !is_last && receipt.bytes < PART_MIN_BYTES;
        if too_small || receipt.bytes > PART_MAX_BYTES || receipt.bytes == 0 {
            return Err(UploadError::PartSizeOutOfRange {
                part: receipt.number,
                bytes: receipt.bytes,
            });
        }
    }
    Ok(UploadCommit {
        upload: Upload {
            state: UploadState::Completing,
            ..upload.clone()
        },
        granted: Vec::new(),
    })
}

/// Finishes completion against the object store's verification.
///
/// # Errors
///
/// Returns [`UploadError`] when the upload is not `Completing`, or when the
/// verified size or digest disagrees with what was declared.
pub fn finish_complete(
    upload: &Upload,
    verified: &VerifiedObject,
    now: Timestamp,
) -> Result<UploadCommit, UploadError> {
    let _ = now;
    if upload.state != UploadState::Completing {
        return Err(UploadError::WrongState { from: upload.state });
    }
    if verified.size_bytes != upload.declared_size {
        return Err(UploadError::SizeMismatch {
            declared: upload.declared_size,
            verified: verified.size_bytes,
        });
    }
    if verified.sha256 != upload.declared_sha256 {
        return Err(UploadError::DigestMismatch {
            declared: upload.declared_sha256,
            verified: verified.sha256,
        });
    }
    Ok(UploadCommit {
        upload: Upload {
            state: UploadState::Ready,
            ..upload.clone()
        },
        granted: Vec::new(),
    })
}

/// Marks an upload consumed by a registry entry.
///
/// Reachable only from `Ready`, which is what makes an upload single-use.
///
/// # Errors
///
/// Returns [`UploadError::AlreadyConsumed`] for a second consumption and
/// [`UploadError::WrongState`] from anything but `Ready`.
pub fn consume(
    upload: &Upload,
    by: RegistrySelector,
    now: Timestamp,
) -> Result<UploadCommit, UploadError> {
    let _ = now;
    if let Some(existing) = &upload.consumed_by {
        return Err(UploadError::AlreadyConsumed(Box::new(existing.clone())));
    }
    if upload.state != UploadState::Ready {
        return Err(UploadError::WrongState { from: upload.state });
    }
    Ok(UploadCommit {
        upload: Upload {
            state: UploadState::Consumed,
            consumed_by: Some(by),
            ..upload.clone()
        },
        granted: Vec::new(),
    })
}

/// Abandons an upload.
///
/// # Errors
///
/// Returns [`UploadError::WrongState`] once completion has begun: an object that
/// may already be assembled is expired, never aborted.
pub fn abort(upload: &Upload, now: Timestamp) -> Result<UploadCommit, UploadError> {
    let _ = now;
    if !upload.state.can_abort() {
        return Err(UploadError::WrongState { from: upload.state });
    }
    Ok(UploadCommit {
        upload: Upload {
            state: UploadState::Aborted,
            ..upload.clone()
        },
        granted: Vec::new(),
    })
}

/// Expires an upload that has run out its grace window.
///
/// Returns `None` when nothing would change, so a sweep writes nothing for
/// uploads it does not own.
#[must_use]
pub fn expire(upload: &Upload, now: Timestamp) -> Option<UploadCommit> {
    if upload.state.is_terminal() || upload.state == UploadState::Ready {
        return None;
    }
    if now.unix_millis() < upload.expires_at.unix_millis() {
        return None;
    }
    Some(UploadCommit {
        upload: Upload {
            state: UploadState::Expired,
            ..upload.clone()
        },
        granted: Vec::new(),
    })
}

#[cfg(test)]
mod tests {
    use aex_content_domain::{ContentDigest, RegisteredName, RegistryKind};
    use aex_wire::ids::{PrefixedId as _, UploadId, Uuid7, WorkspaceId};
    use aex_wire::types::Timestamp;

    use super::{
        PART_MAX_BYTES, PART_MAX_COUNT, PART_MIN_BYTES, PartReceipt, RegistrySelector, Upload,
        UploadError, UploadState, VerifiedObject, abort, begin_complete, consume, expire,
        finish_complete, plan_parts,
    };

    fn moment(millis: i64) -> Timestamp {
        Timestamp::from_unix_millis(millis).expect("in range")
    }

    fn upload(size: u64) -> Upload {
        Upload {
            id: UploadId::from_uuid7(Uuid7::compose(1, [1; 10])),
            workspace: WorkspaceId::from_uuid7(Uuid7::compose(1, [2; 10])),
            state: UploadState::Created,
            declared_size: size,
            declared_sha256: ContentDigest::of(b"body"),
            content_type: None,
            parts: plan_parts(size).expect("plannable"),
            consumed_by: None,
            created_at: moment(0),
            expires_at: moment(86_400_000),
        }
    }

    fn selector() -> RegistrySelector {
        RegistrySelector {
            workspace: WorkspaceId::from_uuid7(Uuid7::compose(1, [2; 10])),
            kind: RegistryKind::File,
            name: RegisteredName::parse("readme").expect("valid"),
        }
    }

    #[test]
    fn part_plans_are_bounded_contiguous_and_ascending() {
        assert_eq!(plan_parts(0), Err(UploadError::Unplannable { bytes: 0 }));
        for size in [
            1_u64,
            PART_MIN_BYTES,
            PART_MIN_BYTES + 1,
            PART_MIN_BYTES * 7,
        ] {
            let plan = plan_parts(size).expect("plannable");
            assert_eq!(plan.total_bytes(), size);
            assert!(plan.parts.len() <= PART_MAX_COUNT as usize);
            for (index, part) in plan.parts.iter().enumerate() {
                assert_eq!(part.number, u32::try_from(index + 1).expect("bounded"));
                assert!(part.bytes <= PART_MAX_BYTES);
                if index + 1 < plan.parts.len() {
                    assert!(part.bytes >= PART_MIN_BYTES);
                }
            }
        }
    }

    #[test]
    fn consumed_is_reachable_only_from_ready() {
        let staged = upload(1_024);
        assert_eq!(
            consume(&staged, selector(), moment(1)),
            Err(UploadError::WrongState {
                from: UploadState::Created
            })
        );

        let completing = begin_complete(
            &staged,
            &[PartReceipt {
                number: 1,
                bytes: 1_024,
            }],
            moment(1),
        )
        .expect("begins")
        .upload;
        let ready = finish_complete(
            &completing,
            &VerifiedObject {
                size_bytes: 1_024,
                sha256: ContentDigest::of(b"body"),
            },
            moment(2),
        )
        .expect("finishes")
        .upload;
        assert_eq!(ready.state, UploadState::Ready);

        let consumed = consume(&ready, selector(), moment(3))
            .expect("consumes")
            .upload;
        assert_eq!(consumed.state, UploadState::Consumed);
        assert!(matches!(
            consume(&consumed, selector(), moment(4)),
            Err(UploadError::AlreadyConsumed(_))
        ));
    }

    #[test]
    fn abort_is_rejected_once_completion_has_begun() {
        let staged = upload(1_024);
        assert!(abort(&staged, moment(1)).is_ok());
        let completing = begin_complete(
            &staged,
            &[PartReceipt {
                number: 1,
                bytes: 1_024,
            }],
            moment(1),
        )
        .expect("begins")
        .upload;
        assert_eq!(
            abort(&completing, moment(2)),
            Err(UploadError::WrongState {
                from: UploadState::Completing
            })
        );
    }

    #[test]
    fn completion_verifies_size_and_digest() {
        let completing = begin_complete(
            &upload(1_024),
            &[PartReceipt {
                number: 1,
                bytes: 1_024,
            }],
            moment(1),
        )
        .expect("begins")
        .upload;
        assert!(matches!(
            finish_complete(
                &completing,
                &VerifiedObject {
                    size_bytes: 1_023,
                    sha256: ContentDigest::of(b"body")
                },
                moment(2)
            ),
            Err(UploadError::SizeMismatch { .. })
        ));
        assert!(matches!(
            finish_complete(
                &completing,
                &VerifiedObject {
                    size_bytes: 1_024,
                    sha256: ContentDigest::of(b"other")
                },
                moment(2)
            ),
            Err(UploadError::DigestMismatch { .. })
        ));
    }

    #[test]
    fn expiry_fires_only_after_the_grace_window() {
        let staged = upload(1_024);
        assert_eq!(expire(&staged, moment(86_399_999)), None);
        let expired = expire(&staged, moment(86_400_000)).expect("expires").upload;
        assert_eq!(expired.state, UploadState::Expired);
        assert_eq!(expire(&expired, moment(86_400_001)), None);
    }

    #[test]
    fn non_contiguous_receipts_are_rejected() {
        let staged = upload(PART_MIN_BYTES * 2);
        assert_eq!(
            begin_complete(
                &staged,
                &[
                    PartReceipt {
                        number: 2,
                        bytes: PART_MIN_BYTES
                    },
                    PartReceipt {
                        number: 1,
                        bytes: PART_MIN_BYTES
                    },
                ],
                moment(1)
            ),
            Err(UploadError::PartsNotContiguous)
        );
        assert!(matches!(
            begin_complete(
                &staged,
                &[
                    PartReceipt {
                        number: 1,
                        bytes: 10
                    },
                    PartReceipt {
                        number: 2,
                        bytes: PART_MIN_BYTES
                    },
                ],
                moment(1)
            ),
            Err(UploadError::PartSizeOutOfRange { part: 1, .. })
        ));
    }
}
