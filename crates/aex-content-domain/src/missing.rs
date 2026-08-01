//! `content_missing`.
//!
//! A metadata row naming an absent or checksum-mismatched body is a typed,
//! terminal, non-retryable condition with exactly one remediation. It is never
//! retried and never substituted: silently serving different bytes than the
//! digest promises would be worse than failing.

use aex_wire::error::ErrorCode;
use aex_wire::ids::WorkspaceId;

use crate::descriptor::ContentDescriptor;
use crate::digest::ContentDigest;
use crate::placement::PlacementClass;

/// Whether a body was found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ContentOutcome {
    /// The body is present and matches its descriptor.
    Present(ContentDescriptor),
    /// The body is not usable.
    Missing(ContentMissing),
}

impl ContentOutcome {
    /// The descriptor, when the body is present.
    #[must_use]
    pub const fn present(&self) -> Option<&ContentDescriptor> {
        match self {
            Self::Present(descriptor) => Some(descriptor),
            Self::Missing(_) => None,
        }
    }
}

/// Why a body is not usable.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum MissingReason {
    /// The object is not there.
    ObjectAbsent,
    /// The stored checksum does not match.
    ChecksumMismatch,
    /// Garbage collection purged it.
    PurgedByGc,
}

/// What the caller can do about a missing body.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Remediation {
    /// Upload the body again.
    Reupload,
}

/// A body that is named but not usable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContentMissing {
    /// The owning workspace.
    pub workspace: WorkspaceId,
    /// The body that is missing.
    pub digest: ContentDigest,
    /// Which storage class it should have been in.
    pub placement: PlacementClass,
    /// Why it is not usable.
    pub reason: MissingReason,
}

impl ContentMissing {
    /// The stable public error code.
    #[must_use]
    pub const fn code(&self) -> ErrorCode {
        ErrorCode::ContentMissing
    }

    /// Always `false`. A retry cannot conjure bytes that are gone.
    #[must_use]
    pub const fn retryable(&self) -> bool {
        false
    }

    /// The only remediation.
    #[must_use]
    pub const fn remediation(&self) -> Remediation {
        Remediation::Reupload
    }
}

#[cfg(test)]
mod tests {
    use aex_wire::error::ErrorCode;
    use aex_wire::ids::{PrefixedId as _, Uuid7, WorkspaceId};

    use super::{ContentMissing, MissingReason, Remediation};
    use crate::digest::ContentDigest;
    use crate::placement::PlacementClass;

    #[test]
    fn every_reason_is_terminal_and_reuploadable() {
        for reason in [
            MissingReason::ObjectAbsent,
            MissingReason::ChecksumMismatch,
            MissingReason::PurgedByGc,
        ] {
            let missing = ContentMissing {
                workspace: WorkspaceId::from_uuid7(Uuid7::compose(1, [1; 10])),
                digest: ContentDigest::of(b"body"),
                placement: PlacementClass::Object,
                reason,
            };
            assert_eq!(missing.code(), ErrorCode::ContentMissing);
            assert!(!missing.retryable());
            assert_eq!(missing.remediation(), Remediation::Reupload);
        }
    }
}
