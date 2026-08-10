//! Multipart upload staging.
//!
//! `Created -> PartsGranted -> Completing -> Ready -> Consumed`, with `Aborted`
//! reachable only from the two earlier states and `Expired` from anything before
//! completion. `Consumed` is terminal and reachable **only** from a registry
//! `set` that pins the upload in the same transaction, so an upload is
//! single-use by construction rather than by convention.
//!
//! # The durable multipart handle
//!
//! [`Upload`] carries [`Upload::provider_upload_id`] and [`Upload::object_key`]
//! as **non-optional** fields, so an upload that names no live multipart upload
//! is unconstructible (E D-1). Without them `upload_abort` cannot issue the
//! exact `AbortMultipartUpload` and pending-upload expiry cannot run at all.
//!
//! # Resolving an ambiguous completion
//!
//! [`resolve_completing`] is the whole of E's D-3: `HeadObject` on the durable
//! object key is the **sole** oracle and it is total. The key is
//! content-addressed and every write to it is conditional, so "an object exists
//! at this key with this digest and this length" is exactly equivalent to "these
//! bytes are committed for this workspace". Nothing here ever guesses, and
//! nothing here can delete an object: the upload lifecycle holds no object-delete
//! verb (E D-4).

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

/// Largest number of parts one `upload_parts_grant` call may cover (E D-9).
///
/// A maximal 10 000-part upload therefore needs at least ten grant calls. Such
/// an upload is at least 50 GB, so the extra round trips are noise against the
/// transfer they are pacing, and the bound is what keeps both the response and
/// the persisted part block inside their ceilings.
pub const PART_GRANT_MAX_PER_CALL: usize = 1_000;

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

    /// Whether the two-value wire enum shows this state as `staging`.
    ///
    /// `Ready` is `ready`; every terminal state is `gone` on every route and has
    /// no wire spelling at all (E D-5).
    #[must_use]
    pub const fn is_staging(self) -> bool {
        matches!(self, Self::Created | Self::PartsGranted | Self::Completing)
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
///
/// `sha256` is the part's own digest, which `presign_part` needs before it can
/// sign and which the completion manifest needs again to prove the chain. It is
/// declared by the client at first grant and is then **fixed**: a re-grant of the
/// same part number carrying a different digest is refused, because the part plan
/// is settled at `upload_create` (E D-9).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PlannedPart {
    /// One-based part number.
    pub number: u32,
    /// How many bytes it carries.
    pub bytes: u64,
    /// The part's own SHA-256, once the client has declared it.
    pub sha256: Option<ContentDigest>,
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

    /// The planned part with this number.
    #[must_use]
    pub fn part(&self, number: u32) -> Option<&PlannedPart> {
        self.parts.iter().find(|part| part.number == number)
    }
}

/// One part the caller asks to have presigned.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PartGrantRequest {
    /// Which part.
    pub number: u32,
    /// The part's own SHA-256, which S3 verifies on the way up.
    pub sha256: ContentDigest,
    /// The exact `Content-Length` the grant will sign.
    pub size_bytes: u64,
}

/// A part the caller says it uploaded.
///
/// Sizes are **not** carried: they come from the stored plan, and per-part
/// digests from the stored declarations, so a completion request carries only
/// what the server cannot already know (E D-9).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PartReceipt {
    /// Which part.
    pub number: u32,
    /// The `ETag` the provider returned for it.
    pub etag: String,
}

/// One part of the manifest a completion was begun under.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubmittedPart {
    /// Which part.
    pub number: u32,
    /// The `ETag` the provider returned for it.
    pub etag: String,
}

/// The durable proof that the object exists (E D-1).
///
/// Written at `finish_complete` from either a completion response or a
/// `HeadObject`, so a `Ready` row is provable rather than assumed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompletionEvidence {
    /// The `ETag` a fenced delete would condition on.
    pub etag: String,
    /// The SHA-256 checksum the provider stored, `COMPOSITE` for a multipart
    /// object.
    pub checksum_sha256: Option<String>,
    /// The full-object CRC64NVME checksum.
    pub checksum_crc64_nvme: Option<String>,
    /// How many parts the assembled object has.
    pub part_count: Option<u64>,
}

/// What the object store says about the assembled object.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifiedObject {
    /// The verified size.
    pub size_bytes: u64,
    /// The verified digest.
    pub sha256: ContentDigest,
    /// The evidence that proves it.
    pub evidence: CompletionEvidence,
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
    /// The provider's multipart upload identity.
    ///
    /// Non-optional: the S3 multipart upload is created **before** this row is
    /// written (E D-2), so a row that names no handle is a state this type
    /// cannot express.
    pub provider_upload_id: String,
    /// The exact unversioned final object key.
    ///
    /// Persisted rather than re-derived, so cleanup is never welded to the key
    /// algorithm.
    pub object_key: String,
    /// The size the caller declared.
    pub declared_size: u64,
    /// The digest the caller declared.
    pub declared_sha256: ContentDigest,
    /// The media type the caller declared.
    pub content_type: Option<String>,
    /// The plan.
    pub parts: PartPlan,
    /// The manifest a completion was begun under, so a retry is deterministic.
    pub completion_manifest: Vec<SubmittedPart>,
    /// The evidence that the object exists, once it does.
    pub completion: Option<CompletionEvidence>,
    /// Which registry entry consumed it.
    pub consumed_by: Option<RegistrySelector>,
    /// When it was staged.
    pub created_at: Timestamp,
    /// When it lapses.
    pub expires_at: Timestamp,
}

impl Upload {
    /// The declared parts a presigner needs, in ascending order.
    ///
    /// # Errors
    ///
    /// [`UploadError::PartHashMissing`] naming the first part whose digest the
    /// client never declared, because a manifest cannot be built without it.
    pub fn declared_parts(&self) -> Result<Vec<(u32, u64, ContentDigest)>, UploadError> {
        self.parts
            .parts
            .iter()
            .map(|part| {
                part.sha256
                    .map(|digest| (part.number, part.bytes, digest))
                    .ok_or(UploadError::PartHashMissing { part: part.number })
            })
            .collect()
    }
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
    /// A grant named a part the plan does not contain.
    #[error("part {part} is not in this upload's plan")]
    UnknownPart {
        /// Which part.
        part: u32,
    },
    /// One call asked for more grants than a call may carry.
    #[error("a grant call covers at most {max} parts, not {requested}")]
    TooManyPartsPerCall {
        /// How many were asked for.
        requested: usize,
        /// The bound.
        max: usize,
    },
    /// A re-grant declared a different digest for a part already declared.
    #[error("part {part} was already declared with a different SHA-256")]
    PartHashChanged {
        /// Which part.
        part: u32,
    },
    /// A part's declared size disagrees with the settled plan.
    #[error("part {part} is {declared} bytes in the plan and {requested} in the request")]
    PartSizeDisagrees {
        /// Which part.
        part: u32,
        /// What the plan says.
        declared: u64,
        /// What the request says.
        requested: u64,
    },
    /// A part reached completion without a declared digest.
    #[error("part {part} has no declared SHA-256, so no manifest can be built")]
    PartHashMissing {
        /// Which part.
        part: u32,
    },
}

/// Plans the parts for an object of a given size.
///
/// Every part but the last carries [`PART_MIN_BYTES`]..=[`PART_MAX_BYTES`]; the
/// final part has no minimum. No part carries a digest yet — the client declares
/// those at first grant (E D-9).
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
                sha256: None,
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
        parts.push(PlannedPart {
            number,
            bytes,
            sha256: None,
        });
        remaining -= bytes;
        number += 1;
    }
    Ok(PartPlan { parts })
}

/// Issues grants for the named parts, settling their declared digests.
///
/// # Errors
///
/// Returns [`UploadError::WrongState`] once the upload has moved past
/// `PartsGranted`, [`UploadError::TooManyPartsPerCall`] above
/// [`PART_GRANT_MAX_PER_CALL`], [`UploadError::UnknownPart`] for a number the
/// plan does not carry, [`UploadError::PartSizeDisagrees`] when the request
/// contradicts the settled plan, and [`UploadError::PartHashChanged`] when a
/// re-grant redeclares a part's digest.
pub fn grant_parts(
    upload: &Upload,
    requests: &[PartGrantRequest],
    now: Timestamp,
) -> Result<UploadCommit, UploadError> {
    let _ = now;
    if !matches!(
        upload.state,
        UploadState::Created | UploadState::PartsGranted
    ) {
        return Err(UploadError::WrongState { from: upload.state });
    }
    if requests.len() > PART_GRANT_MAX_PER_CALL {
        return Err(UploadError::TooManyPartsPerCall {
            requested: requests.len(),
            max: PART_GRANT_MAX_PER_CALL,
        });
    }

    let mut parts = upload.parts.clone();
    let mut granted = Vec::with_capacity(requests.len());
    for request in requests {
        let planned = parts
            .parts
            .iter_mut()
            .find(|part| part.number == request.number)
            .ok_or(UploadError::UnknownPart {
                part: request.number,
            })?;
        if planned.bytes != request.size_bytes {
            return Err(UploadError::PartSizeDisagrees {
                part: request.number,
                declared: planned.bytes,
                requested: request.size_bytes,
            });
        }
        match planned.sha256 {
            Some(existing) if existing != request.sha256 => {
                return Err(UploadError::PartHashChanged {
                    part: request.number,
                });
            }
            Some(_) => {}
            None => planned.sha256 = Some(request.sha256),
        }
        granted.push(request.number);
    }
    granted.sort_unstable();
    granted.dedup();

    Ok(UploadCommit {
        upload: Upload {
            state: UploadState::PartsGranted,
            parts,
            ..upload.clone()
        },
        granted,
    })
}

/// Begins completion against the caller's part receipts.
///
/// The receipts are recorded as [`Upload::completion_manifest`], so a retried
/// completion is deterministic rather than dependent on the client resending
/// identical input.
///
/// # Errors
///
/// Returns [`UploadError`] when the upload is in the wrong state, when the
/// receipts are not contiguous and ascending, when the plan disagrees with them,
/// or when any part reached completion with no declared digest.
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
    if receipts.is_empty() || receipts.len() != upload.parts.parts.len() {
        return Err(UploadError::PartsNotContiguous);
    }
    let mut manifest = Vec::with_capacity(receipts.len());
    for (index, receipt) in receipts.iter().enumerate() {
        let expected = u32::try_from(index + 1).unwrap_or(u32::MAX);
        if receipt.number != expected {
            return Err(UploadError::PartsNotContiguous);
        }
        let planned = upload
            .parts
            .part(receipt.number)
            .ok_or(UploadError::UnknownPart {
                part: receipt.number,
            })?;
        let is_last = index + 1 == receipts.len();
        let too_small = !is_last && planned.bytes < PART_MIN_BYTES;
        if too_small || planned.bytes > PART_MAX_BYTES || planned.bytes == 0 {
            return Err(UploadError::PartSizeOutOfRange {
                part: receipt.number,
                bytes: planned.bytes,
            });
        }
        if planned.sha256.is_none() {
            return Err(UploadError::PartHashMissing {
                part: receipt.number,
            });
        }
        manifest.push(SubmittedPart {
            number: receipt.number,
            etag: receipt.etag.clone(),
        });
    }
    Ok(UploadCommit {
        upload: Upload {
            state: UploadState::Completing,
            completion_manifest: manifest,
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
            completion: Some(verified.evidence.clone()),
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
/// may already be assembled is resolved through [`resolve_completing`], never
/// aborted on a guess.
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

/// What a `HeadObject` on the durable object key established (E D-3).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HeadOracle {
    /// The key holds an object.
    Present {
        /// Its stored length.
        content_length: u64,
        /// The digest metadata the create wrote, when it is parseable.
        declared_digest: Option<ContentDigest>,
        /// What that object proves.
        evidence: CompletionEvidence,
    },
    /// The key holds nothing.
    Absent,
    /// The head could not be taken. This is not evidence of anything.
    Unavailable,
}

/// What the D-3 oracle decided.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AmbiguityResolution {
    /// The bytes are committed. Transition to `Ready`; never abort, never delete.
    Committed(Box<UploadCommit>),
    /// The bytes are not committed, so `AbortMultipartUpload` is now safe.
    NotCommitted,
    /// An object exists whose digest or length disagrees with the declaration.
    ///
    /// Impossible under content addressing. Refuse loudly, leave the row where it
    /// is, and re-arm: this must be investigated, never swept.
    Integrity {
        /// What disagreed.
        detail: String,
    },
    /// Nothing could be established. Re-arm with backoff.
    Retry,
    /// The upload is already `Ready` or terminal; there is no ambiguity.
    AlreadySettled,
}

/// Resolves an upload against the sole oracle: `HeadObject` on its object key.
///
/// This is total rather than probabilistic. The key is content-addressed and
/// every write to it is conditional, so "an object exists at this key with this
/// digest and this length" is exactly equivalent to "these bytes are committed
/// for this workspace". Whether *this* multipart upload or a concurrent upload of
/// identical bytes produced it is a distinction with no observable difference.
#[must_use]
pub fn resolve_completing(
    upload: &Upload,
    oracle: &HeadOracle,
    now: Timestamp,
) -> AmbiguityResolution {
    if upload.state.is_terminal() || upload.state == UploadState::Ready {
        return AmbiguityResolution::AlreadySettled;
    }
    match oracle {
        HeadOracle::Unavailable => AmbiguityResolution::Retry,
        HeadOracle::Absent => AmbiguityResolution::NotCommitted,
        HeadOracle::Present {
            content_length,
            declared_digest,
            evidence,
        } => {
            if *content_length != upload.declared_size {
                return AmbiguityResolution::Integrity {
                    detail: format!(
                        "the object at the upload's key is {content_length} bytes and the upload \
                         declared {}",
                        upload.declared_size
                    ),
                };
            }
            if *declared_digest != Some(upload.declared_sha256) {
                return AmbiguityResolution::Integrity {
                    detail: "the object at the upload's key carries a different declared digest"
                        .to_owned(),
                };
            }
            let completing = Upload {
                state: UploadState::Completing,
                ..upload.clone()
            };
            let verified = VerifiedObject {
                size_bytes: *content_length,
                sha256: upload.declared_sha256,
                evidence: evidence.clone(),
            };
            match finish_complete(&completing, &verified, now) {
                Ok(commit) => AmbiguityResolution::Committed(Box::new(commit)),
                Err(error) => AmbiguityResolution::Integrity {
                    detail: error.to_string(),
                },
            }
        }
    }
}

/// What an expiry sweep should do with one upload (E D-5).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExpiryOutcome {
    /// Nothing would change, so the sweep writes nothing.
    Unchanged,
    /// The upload lapses. The provider effect is issued first, then this write.
    Expire(Box<UploadCommit>),
    /// The row is deleted outright.
    ///
    /// "Uploads are transport plumbing, not assets": a `Ready` row's object is
    /// already an independent content descriptor with its own reachability and
    /// its own garbage collection, and the upload row has no further job. Before
    /// this outcome existed those rows were immortal.
    DeleteRow,
}

/// Decides what an expiry sweep does with an upload that reached its deadline.
///
/// A `Ready` row past its grace is deleted rather than transitioned, and so are
/// `Aborted` and `Expired` rows — all three have no remaining consumer, and a row
/// with neither a TTL nor a delete outcome never goes away. `Consumed` is left
/// alone: a registry pointer may still name it.
#[must_use]
pub fn expire(upload: &Upload, now: Timestamp) -> ExpiryOutcome {
    if now.unix_millis() < upload.expires_at.unix_millis() {
        return ExpiryOutcome::Unchanged;
    }
    match upload.state {
        UploadState::Consumed => ExpiryOutcome::Unchanged,
        UploadState::Ready | UploadState::Aborted | UploadState::Expired => {
            ExpiryOutcome::DeleteRow
        }
        UploadState::Created | UploadState::PartsGranted | UploadState::Completing => {
            ExpiryOutcome::Expire(Box::new(UploadCommit {
                upload: Upload {
                    state: UploadState::Expired,
                    ..upload.clone()
                },
                granted: Vec::new(),
            }))
        }
    }
}

#[cfg(test)]
mod tests {
    use aex_content_domain::{ContentDigest, RegisteredName, RegistryKind};
    use aex_wire::ids::{PrefixedId as _, UploadId, Uuid7, WorkspaceId};
    use aex_wire::types::Timestamp;

    use super::{
        AmbiguityResolution, CompletionEvidence, ExpiryOutcome, HeadOracle, PART_GRANT_MAX_PER_CALL,
        PART_MAX_BYTES, PART_MAX_COUNT, PART_MIN_BYTES, PartGrantRequest, PartReceipt,
        RegistrySelector, Upload, UploadError, UploadState, VerifiedObject, abort, begin_complete,
        consume, expire, finish_complete, grant_parts, plan_parts, resolve_completing,
    };

    fn moment(millis: i64) -> Timestamp {
        Timestamp::from_unix_millis(millis).expect("in range")
    }

    fn evidence() -> CompletionEvidence {
        CompletionEvidence {
            etag: "\"assembled\"".to_owned(),
            checksum_sha256: Some("composite=".to_owned()),
            checksum_crc64_nvme: Some("crc=".to_owned()),
            part_count: Some(1),
        }
    }

    fn upload(size: u64) -> Upload {
        Upload {
            id: UploadId::from_uuid7(Uuid7::compose(1, [1; 10])),
            workspace: WorkspaceId::from_uuid7(Uuid7::compose(1, [2; 10])),
            state: UploadState::Created,
            provider_upload_id: "provider-mpu-1".to_owned(),
            object_key: "wks/ab/cd/abcd".to_owned(),
            declared_size: size,
            declared_sha256: ContentDigest::of(b"body"),
            content_type: None,
            parts: plan_parts(size).expect("plannable"),
            completion_manifest: Vec::new(),
            completion: None,
            consumed_by: None,
            created_at: moment(0),
            expires_at: moment(86_400_000),
        }
    }

    fn granted(size: u64) -> Upload {
        let staged = upload(size);
        let requests: Vec<PartGrantRequest> = staged
            .parts
            .parts
            .iter()
            .map(|part| PartGrantRequest {
                number: part.number,
                sha256: ContentDigest::of(&part.number.to_be_bytes()),
                size_bytes: part.bytes,
            })
            .collect();
        grant_parts(&staged, &requests, moment(1))
            .expect("grants")
            .upload
    }

    fn receipts(upload: &Upload) -> Vec<PartReceipt> {
        upload
            .parts
            .parts
            .iter()
            .map(|part| PartReceipt {
                number: part.number,
                etag: format!("\"{}\"", part.number),
            })
            .collect()
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
                assert_eq!(part.sha256, None, "a plan declares no digests");
                if index + 1 < plan.parts.len() {
                    assert!(part.bytes >= PART_MIN_BYTES);
                }
            }
        }
    }

    #[test]
    fn a_part_digest_is_settled_at_first_grant_and_never_redeclared() {
        let staged = upload(1_024);
        let first = grant_parts(
            &staged,
            &[PartGrantRequest {
                number: 1,
                sha256: ContentDigest::of(b"part one"),
                size_bytes: 1_024,
            }],
            moment(1),
        )
        .expect("grants")
        .upload;
        assert_eq!(
            first.parts.part(1).expect("planned").sha256,
            Some(ContentDigest::of(b"part one"))
        );

        // A re-grant of the same digest is legal and idempotent.
        assert!(
            grant_parts(
                &first,
                &[PartGrantRequest {
                    number: 1,
                    sha256: ContentDigest::of(b"part one"),
                    size_bytes: 1_024,
                }],
                moment(2)
            )
            .is_ok()
        );

        assert_eq!(
            grant_parts(
                &first,
                &[PartGrantRequest {
                    number: 1,
                    sha256: ContentDigest::of(b"another body"),
                    size_bytes: 1_024,
                }],
                moment(2)
            ),
            Err(UploadError::PartHashChanged { part: 1 })
        );
    }

    #[test]
    fn a_grant_call_is_capped_and_refuses_a_part_outside_the_plan() {
        let staged = upload(1_024);
        assert_eq!(
            grant_parts(
                &staged,
                &[PartGrantRequest {
                    number: 2,
                    sha256: ContentDigest::of(b"nope"),
                    size_bytes: 1_024,
                }],
                moment(1)
            ),
            Err(UploadError::UnknownPart { part: 2 })
        );
        let overlong: Vec<PartGrantRequest> = (1..=u32::try_from(PART_GRANT_MAX_PER_CALL + 1)
            .expect("bounded"))
            .map(|number| PartGrantRequest {
                number,
                sha256: ContentDigest::of(b"x"),
                size_bytes: 1_024,
            })
            .collect();
        assert_eq!(
            grant_parts(&staged, &overlong, moment(1)),
            Err(UploadError::TooManyPartsPerCall {
                requested: PART_GRANT_MAX_PER_CALL + 1,
                max: PART_GRANT_MAX_PER_CALL,
            })
        );
    }

    #[test]
    fn completion_cannot_begin_before_every_part_has_declared_its_digest() {
        let staged = upload(1_024);
        assert_eq!(
            begin_complete(&staged, &receipts(&staged), moment(1)),
            Err(UploadError::PartHashMissing { part: 1 })
        );
    }

    #[test]
    fn a_completion_records_the_manifest_it_began_under() {
        let ready = granted(1_024);
        let completing = begin_complete(&ready, &receipts(&ready), moment(2))
            .expect("begins")
            .upload;
        assert_eq!(completing.completion_manifest.len(), 1);
        assert_eq!(completing.completion_manifest[0].etag, "\"1\"");
    }

    #[test]
    fn consumed_is_reachable_only_from_ready() {
        let staged = granted(1_024);
        assert_eq!(
            consume(&staged, selector(), moment(1)),
            Err(UploadError::WrongState {
                from: UploadState::PartsGranted
            })
        );

        let completing = begin_complete(&staged, &receipts(&staged), moment(1))
            .expect("begins")
            .upload;
        let ready = finish_complete(
            &completing,
            &VerifiedObject {
                size_bytes: 1_024,
                sha256: ContentDigest::of(b"body"),
                evidence: evidence(),
            },
            moment(2),
        )
        .expect("finishes")
        .upload;
        assert_eq!(ready.state, UploadState::Ready);
        assert_eq!(ready.completion, Some(evidence()));

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
        let staged = granted(1_024);
        assert!(abort(&staged, moment(1)).is_ok());
        let completing = begin_complete(&staged, &receipts(&staged), moment(1))
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
        let staged = granted(1_024);
        let completing = begin_complete(&staged, &receipts(&staged), moment(1))
            .expect("begins")
            .upload;
        assert!(matches!(
            finish_complete(
                &completing,
                &VerifiedObject {
                    size_bytes: 1_023,
                    sha256: ContentDigest::of(b"body"),
                    evidence: evidence(),
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
                    sha256: ContentDigest::of(b"other"),
                    evidence: evidence(),
                },
                moment(2)
            ),
            Err(UploadError::DigestMismatch { .. })
        ));
    }

    #[test]
    fn the_head_oracle_decides_every_one_of_the_five_ambiguity_cases() {
        let staged = granted(1_024);
        let completing = begin_complete(&staged, &receipts(&staged), moment(1))
            .expect("begins")
            .upload;

        // Case 2: the object exists and agrees.
        let resolved = resolve_completing(
            &completing,
            &HeadOracle::Present {
                content_length: 1_024,
                declared_digest: Some(ContentDigest::of(b"body")),
                evidence: evidence(),
            },
            moment(2),
        );
        let AmbiguityResolution::Committed(commit) = resolved else {
            panic!("a matching object is a committed completion");
        };
        assert_eq!(commit.upload.state, UploadState::Ready);
        assert_eq!(commit.upload.completion, Some(evidence()));

        // Case 3: the object is absent, so the abort is safe.
        assert_eq!(
            resolve_completing(&completing, &HeadOracle::Absent, moment(2)),
            AmbiguityResolution::NotCommitted
        );

        // Case 4: an object exists whose length disagrees.
        assert!(matches!(
            resolve_completing(
                &completing,
                &HeadOracle::Present {
                    content_length: 999,
                    declared_digest: Some(ContentDigest::of(b"body")),
                    evidence: evidence(),
                },
                moment(2)
            ),
            AmbiguityResolution::Integrity { .. }
        ));

        // Case 4 again: the digest disagrees.
        assert!(matches!(
            resolve_completing(
                &completing,
                &HeadOracle::Present {
                    content_length: 1_024,
                    declared_digest: Some(ContentDigest::of(b"substituted")),
                    evidence: evidence(),
                },
                moment(2)
            ),
            AmbiguityResolution::Integrity { .. }
        ));

        // Case 5: nothing was established.
        assert_eq!(
            resolve_completing(&completing, &HeadOracle::Unavailable, moment(2)),
            AmbiguityResolution::Retry
        );
    }

    #[test]
    fn a_settled_upload_is_never_re_resolved() {
        let staged = granted(1_024);
        let completing = begin_complete(&staged, &receipts(&staged), moment(1))
            .expect("begins")
            .upload;
        let ready = finish_complete(
            &completing,
            &VerifiedObject {
                size_bytes: 1_024,
                sha256: ContentDigest::of(b"body"),
                evidence: evidence(),
            },
            moment(2),
        )
        .expect("finishes")
        .upload;
        assert_eq!(
            resolve_completing(&ready, &HeadOracle::Absent, moment(3)),
            AmbiguityResolution::AlreadySettled
        );
    }

    #[test]
    fn expiry_fires_only_after_the_grace_window() {
        let staged = upload(1_024);
        assert_eq!(expire(&staged, moment(86_399_999)), ExpiryOutcome::Unchanged);
        let ExpiryOutcome::Expire(commit) = expire(&staged, moment(86_400_000)) else {
            panic!("a staged upload lapses");
        };
        assert_eq!(commit.upload.state, UploadState::Expired);
    }

    #[test]
    fn a_ready_row_past_its_grace_is_deleted_rather_than_left_immortal() {
        let staged = granted(1_024);
        let completing = begin_complete(&staged, &receipts(&staged), moment(1))
            .expect("begins")
            .upload;
        let ready = finish_complete(
            &completing,
            &VerifiedObject {
                size_bytes: 1_024,
                sha256: ContentDigest::of(b"body"),
                evidence: evidence(),
            },
            moment(2),
        )
        .expect("finishes")
        .upload;
        assert_eq!(expire(&ready, moment(86_399_999)), ExpiryOutcome::Unchanged);
        assert_eq!(expire(&ready, moment(86_400_000)), ExpiryOutcome::DeleteRow);

        let aborted = abort(&staged, moment(1)).expect("aborts").upload;
        assert_eq!(expire(&aborted, moment(86_400_000)), ExpiryOutcome::DeleteRow);

        // A consumed row is still named by a registry pointer, so it stays.
        let consumed = consume(&ready, selector(), moment(3)).expect("consumes").upload;
        assert_eq!(expire(&consumed, moment(86_400_000)), ExpiryOutcome::Unchanged);
    }

    #[test]
    fn non_contiguous_receipts_are_rejected() {
        let staged = granted(PART_MIN_BYTES * 2);
        assert_eq!(
            begin_complete(
                &staged,
                &[
                    PartReceipt {
                        number: 2,
                        etag: "\"two\"".to_owned()
                    },
                    PartReceipt {
                        number: 1,
                        etag: "\"one\"".to_owned()
                    },
                ],
                moment(1)
            ),
            Err(UploadError::PartsNotContiguous)
        );
        assert_eq!(
            begin_complete(
                &staged,
                &[PartReceipt {
                    number: 1,
                    etag: "\"one\"".to_owned()
                }],
                moment(1)
            ),
            Err(UploadError::PartsNotContiguous)
        );
    }
}
