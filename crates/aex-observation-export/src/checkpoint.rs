//! The multipart checkpoint and its fenced publication.
//!
//! A checkpoint is sufficient to resume: no in-memory-only state is required.
//! Resume re-reads the checkpoint, lists the provider's parts **to exhaustion**,
//! and compares every part number, size and `ETag` against it. A truncated listing
//! without a marker is a hard integrity failure, never a silent short list — a
//! short list would look exactly like a completed upload and would publish a
//! truncated artifact.

/// One uploaded part.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PartRecord {
    /// The part number, one-based.
    pub number: u32,
    /// The provider `ETag`.
    pub etag: Box<str>,
    /// The part's `SHA-256`.
    pub sha256: Box<str>,
    /// The part's exact byte length.
    pub bytes: u64,
}

/// The durable resume point.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExportCheckpoint {
    /// The lease fence this checkpoint was written under.
    pub fence: u64,
    /// The multipart upload identity.
    pub upload_id: Box<str>,
    /// Every part uploaded so far, in order.
    pub parts: Vec<PartRecord>,
    /// The query cursor to resume from.
    pub cursor: Box<str>,
    /// Which member the task is inside.
    pub member_ordinal: u16,
    /// The member encoder's safe checkpoint.
    pub member_checkpoint: u64,
    /// How many bytes have been written.
    pub bytes_written: u64,
}

/// Why a resume was refused.
#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
pub enum ResumeError {
    /// The provider's part listing does not match the checkpoint.
    #[error("part {number} does not match the checkpoint: {what}")]
    IntegrityMismatch {
        /// The offending part number.
        number: u32,
        /// Which attribute differs.
        what: &'static str,
    },
    /// The provider truncated the listing without giving a marker.
    #[error("the part listing was truncated with no continuation marker")]
    TruncatedListing,
    /// Another worker took the lease.
    #[error("the lease moved from fence {held} to fence {observed}")]
    LeaseLost {
        /// The fence this task holds.
        held: u64,
        /// The fence the authority reports.
        observed: u64,
    },
}

/// Verifies a provider part listing against the checkpoint.
///
/// # Errors
///
/// Returns [`ResumeError::TruncatedListing`] when the provider signalled more
/// parts without a marker, and [`ResumeError::IntegrityMismatch`] naming the
/// first part whose number, size or `ETag` differs.
pub fn verify_parts(
    checkpoint: &ExportCheckpoint,
    listed: &[PartRecord],
    truncated_without_marker: bool,
) -> Result<(), ResumeError> {
    if truncated_without_marker {
        return Err(ResumeError::TruncatedListing);
    }
    if listed.len() < checkpoint.parts.len() {
        return Err(ResumeError::IntegrityMismatch {
            number: u32::try_from(listed.len() + 1).unwrap_or(u32::MAX),
            what: "missing from the provider listing",
        });
    }
    for (expected, observed) in checkpoint.parts.iter().zip(listed) {
        if expected.number != observed.number {
            return Err(ResumeError::IntegrityMismatch {
                number: expected.number,
                what: "part number",
            });
        }
        if expected.bytes != observed.bytes {
            return Err(ResumeError::IntegrityMismatch {
                number: expected.number,
                what: "size",
            });
        }
        if expected.etag != observed.etag {
            return Err(ResumeError::IntegrityMismatch {
                number: expected.number,
                what: "etag",
            });
        }
    }
    Ok(())
}

/// The conditions publication is fenced on.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PublishFence {
    /// The state the row must be in.
    pub state_is_generating: bool,
    /// The fence the task holds.
    pub fence: u64,
    /// The fence the authority holds.
    pub authority_fence: u64,
    /// Whether a cancel was requested.
    pub cancel_requested: bool,
    /// The deletion epoch the export pinned.
    pub pinned_deletion_epoch: u64,
    /// The scope's live deletion epoch.
    pub live_deletion_epoch: u64,
}

/// What publication did.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum Publication {
    /// The export became `ready`.
    Ready,
    /// A cancel, a deletion or a lease takeover won.
    ///
    /// The task aborts its multipart upload, deletes nothing else and exits
    /// **zero**: the cancel is authoritative, not an error. Treating it as a
    /// failure would produce spurious alarms and retry storms on a correct
    /// outcome.
    Superseded {
        /// What won.
        reason: &'static str,
    },
}

/// Evaluates the one conditional update that publishes an export.
#[must_use]
pub const fn publish(fence: &PublishFence) -> Publication {
    if !fence.state_is_generating {
        return Publication::Superseded {
            reason: "the export is no longer generating",
        };
    }
    if fence.fence != fence.authority_fence {
        return Publication::Superseded {
            reason: "a lease takeover won",
        };
    }
    if fence.cancel_requested {
        return Publication::Superseded {
            reason: "a cancel won",
        };
    }
    if fence.pinned_deletion_epoch != fence.live_deletion_epoch {
        return Publication::Superseded {
            reason: "a deletion won",
        };
    }
    Publication::Ready
}

#[cfg(test)]
mod tests {
    use super::{
        ExportCheckpoint, PartRecord, Publication, PublishFence, ResumeError, publish, verify_parts,
    };

    fn part(number: u32, bytes: u64, etag: &str) -> PartRecord {
        PartRecord {
            number,
            etag: etag.into(),
            sha256: "0".repeat(64).into(),
            bytes,
        }
    }

    fn checkpoint() -> ExportCheckpoint {
        ExportCheckpoint {
            fence: 3,
            upload_id: "upload".into(),
            parts: vec![part(1, 5 << 20, "a"), part(2, 5 << 20, "b")],
            cursor: "cur_x".into(),
            member_ordinal: 0,
            member_checkpoint: 200,
            bytes_written: 10 << 20,
        }
    }

    #[test]
    fn a_matching_listing_resumes() {
        verify_parts(&checkpoint(), &checkpoint().parts, false).expect("resumes");
    }

    #[test]
    fn a_truncated_listing_without_a_marker_is_a_hard_failure() {
        assert_eq!(
            verify_parts(&checkpoint(), &checkpoint().parts, true),
            Err(ResumeError::TruncatedListing)
        );
    }

    #[test]
    fn a_differing_size_or_etag_names_the_part() {
        let mut wrong_size = checkpoint().parts;
        wrong_size[1].bytes = 1;
        assert_eq!(
            verify_parts(&checkpoint(), &wrong_size, false),
            Err(ResumeError::IntegrityMismatch {
                number: 2,
                what: "size"
            })
        );

        let mut wrong_etag = checkpoint().parts;
        wrong_etag[0].etag = "z".into();
        assert_eq!(
            verify_parts(&checkpoint(), &wrong_etag, false),
            Err(ResumeError::IntegrityMismatch {
                number: 1,
                what: "etag"
            })
        );
    }

    #[test]
    fn a_short_listing_is_never_taken_as_a_completed_upload() {
        let short = vec![part(1, 5 << 20, "a")];
        assert!(matches!(
            verify_parts(&checkpoint(), &short, false),
            Err(ResumeError::IntegrityMismatch { .. })
        ));
    }

    #[test]
    fn publication_is_fenced_on_state_lease_cancel_and_deletion() {
        let healthy = PublishFence {
            state_is_generating: true,
            fence: 3,
            authority_fence: 3,
            cancel_requested: false,
            pinned_deletion_epoch: 1,
            live_deletion_epoch: 1,
        };
        assert_eq!(publish(&healthy), Publication::Ready);

        for (mutated, expected) in [
            (
                PublishFence {
                    state_is_generating: false,
                    ..healthy
                },
                "the export is no longer generating",
            ),
            (
                PublishFence {
                    authority_fence: 4,
                    ..healthy
                },
                "a lease takeover won",
            ),
            (
                PublishFence {
                    cancel_requested: true,
                    ..healthy
                },
                "a cancel won",
            ),
            (
                PublishFence {
                    live_deletion_epoch: 2,
                    ..healthy
                },
                "a deletion won",
            ),
        ] {
            assert_eq!(
                publish(&mutated),
                Publication::Superseded { reason: expected }
            );
        }
    }

    #[test]
    fn a_lost_lease_names_both_fences() {
        let error = ResumeError::LeaseLost {
            held: 3,
            observed: 4,
        };
        assert!(error.to_string().contains('3') && error.to_string().contains('4'));
    }
}
