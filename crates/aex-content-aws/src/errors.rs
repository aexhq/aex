//! The object-store error vocabulary and the mapping from an S3 condition onto
//! it.
//!
//! Two mappings matter more than the rest. `404 NoSuchKey` on a **committed**
//! descriptor is [`ContentObjectError::ContentMissing`], which surfaces as the
//! public `content_missing` and instructs an exact-byte reupload; it is never
//! remapped to `404 not_found`, which would read as an authorization result, and
//! never to a `500`. And an ambiguous `CompleteMultipartUpload` is
//! [`ContentObjectError::CommitAmbiguous`], resolved by `HeadObject` on the
//! final key — re-issuing the completion is exactly the wrong move.

use aex_session_dynamodb::error::Resolution;
use aws_sdk_s3::error::SdkError;
use aws_smithy_types::error::metadata::ProvideErrorMetadata;

/// The code S3 returns when a conditional header was not satisfied.
pub const PRECONDITION_FAILED: &str = "PreconditionFailed";

/// Every way an object operation can fail.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ContentObjectError {
    /// The key already holds a **different** body.
    ///
    /// Under SHA-256 this can only mean a key-derivation bug or a substituted
    /// object, so it is hard, non-retryable and alarms. Substituted bytes are
    /// never returned.
    #[error("`{key}` already holds a different body")]
    DigestCollision {
        /// The object key.
        key: String,
    },
    /// A committed descriptor's object is gone.
    #[error("the body for `{digest}` is missing and must be reuploaded byte for byte")]
    ContentMissing {
        /// The customer-visible digest.
        digest: String,
    },
    /// The provider's view of the upload disagrees with the manifest.
    #[error("integrity mismatch: {detail}")]
    IntegrityMismatch {
        /// What disagreed.
        detail: String,
    },
    /// The requested range is outside what a single read may serve.
    #[error("a ranged read of {requested} bytes exceeds the {ceiling} byte ceiling")]
    InvalidRange {
        /// What was asked for.
        requested: u64,
        /// The ceiling.
        ceiling: u64,
    },
    /// A concurrent conditional request won.
    #[error("the conditional request contended with a concurrent writer")]
    Contended,
    /// The service asked the caller to slow down.
    #[error("throttled")]
    Throttled,
    /// The caller's role is denied. Never masked as not-found.
    #[error("the caller is denied this object action")]
    Forbidden,
    /// The write may or may not have landed.
    #[error("the object commit outcome is unknown; {}", resolve_by.as_str())]
    CommitAmbiguous {
        /// How the caller must establish the outcome.
        resolve_by: Resolution,
    },
    /// The service was unavailable.
    #[error("the object store is unavailable: {detail}")]
    Unavailable {
        /// What the service reported.
        detail: String,
    },
    /// The request was malformed, which is always a bug here.
    #[error("invalid object request: {detail}")]
    Invalid {
        /// What was wrong.
        detail: String,
    },
}

impl ContentObjectError {
    /// Whether the bounded retry policy may re-issue the request.
    ///
    /// [`ContentObjectError::CommitAmbiguous`] is deliberately absent: it is
    /// resolved by reading, never by writing again.
    #[must_use]
    pub const fn retryable(&self) -> bool {
        matches!(
            self,
            Self::Contended | Self::Throttled | Self::Unavailable { .. }
        )
    }
}

/// Whether an ambiguous outcome on this operation is a write, and how to
/// resolve it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ObjectIdempotence {
    /// A read. An ambiguous outcome is unavailability.
    Read,
    /// A write. An ambiguous outcome must be resolved by reading.
    Write(Resolution),
}

/// The service error code, when the failure carried one.
#[must_use]
pub fn code_of<E, R>(error: &SdkError<E, R>) -> Option<String>
where
    E: ProvideErrorMetadata,
{
    match error {
        SdkError::ServiceError(service) => service.err().code().map(str::to_owned),
        _ => None,
    }
}

/// Maps one S3 SDK failure onto the object vocabulary.
#[must_use]
pub fn classify<E, R>(error: &SdkError<E, R>, idempotence: ObjectIdempotence) -> ContentObjectError
where
    E: ProvideErrorMetadata,
{
    match error {
        SdkError::ConstructionFailure(_) => ContentObjectError::Invalid {
            detail: "the request could not be constructed".to_owned(),
        },
        SdkError::TimeoutError(_) | SdkError::DispatchFailure(_) | SdkError::ResponseError(_) => {
            match idempotence {
                ObjectIdempotence::Read => ContentObjectError::Unavailable {
                    detail: "the request did not complete".to_owned(),
                },
                ObjectIdempotence::Write(resolve_by) => {
                    ContentObjectError::CommitAmbiguous { resolve_by }
                }
            }
        }
        SdkError::ServiceError(service) => {
            classify_code(service.err().code().unwrap_or("Unknown"), idempotence)
        }
        _ => ContentObjectError::Unavailable {
            detail: "an unrecognised SDK failure".to_owned(),
        },
    }
}

/// Maps one S3 error code onto the object vocabulary.
///
/// Split out from [`classify`] so the whole table is assertable without
/// synthesising an `SdkError` for every row.
#[must_use]
pub fn classify_code(code: &str, idempotence: ObjectIdempotence) -> ContentObjectError {
    match code {
        // A caller that issued a conditional create resolves this itself, by
        // comparing the stored object; anywhere else it is contention.
        PRECONDITION_FAILED | "ConditionalRequestConflict" | "OperationAborted" => {
            ContentObjectError::Contended
        }
        "NoSuchKey" | "NotFound" => ContentObjectError::ContentMissing {
            digest: "unknown".to_owned(),
        },
        "AccessDenied" | "AllAccessDisabled" => ContentObjectError::Forbidden,
        "InvalidPart"
        | "InvalidPartOrder"
        | "EntityTooSmall"
        | "BadDigest"
        | "XAmzContentSHA256Mismatch"
        | "InvalidDigest" => ContentObjectError::IntegrityMismatch {
            detail: format!("the service reported `{code}`"),
        },
        "InvalidRange" => ContentObjectError::InvalidRange {
            requested: 0,
            ceiling: crate::object_key::MAX_RANGE_BYTES,
        },
        "SlowDown" | "RequestLimitExceeded" | "TooManyRequests" => ContentObjectError::Throttled,
        "RequestTimeout" | "RequestTimeoutException" => match idempotence {
            ObjectIdempotence::Read => ContentObjectError::Unavailable {
                detail: "the request timed out".to_owned(),
            },
            ObjectIdempotence::Write(resolve_by) => {
                ContentObjectError::CommitAmbiguous { resolve_by }
            }
        },
        "InternalError" | "ServiceUnavailable" => ContentObjectError::Unavailable {
            detail: format!("the service reported `{code}`"),
        },
        "NoSuchBucket" | "InvalidBucketName" => ContentObjectError::Invalid {
            detail: format!("the composition addressed `{code}`"),
        },
        other => ContentObjectError::Unavailable {
            detail: format!("the service reported `{other}`"),
        },
    }
}

#[cfg(test)]
mod tests {
    use aex_session_dynamodb::error::Resolution;

    use super::{ContentObjectError, ObjectIdempotence, classify_code};

    #[test]
    fn an_ambiguous_completion_is_resolved_by_a_head_and_never_by_a_retry() {
        let error = classify_code(
            "RequestTimeout",
            ObjectIdempotence::Write(Resolution::ObjectHead),
        );
        assert_eq!(
            error,
            ContentObjectError::CommitAmbiguous {
                resolve_by: Resolution::ObjectHead
            }
        );
        assert!(
            !error.retryable(),
            "re-issuing a completion whose outcome is unknown is the one thing that must not happen"
        );
    }

    #[test]
    fn a_denial_is_never_masked_as_a_missing_object() {
        assert_eq!(
            classify_code("AccessDenied", ObjectIdempotence::Read),
            ContentObjectError::Forbidden
        );
    }

    #[test]
    fn a_missing_object_is_content_missing_and_never_a_generic_failure() {
        assert!(matches!(
            classify_code("NoSuchKey", ObjectIdempotence::Read),
            ContentObjectError::ContentMissing { .. }
        ));
    }

    #[test]
    fn every_integrity_code_lands_on_one_typed_mismatch() {
        for code in [
            "InvalidPart",
            "InvalidPartOrder",
            "EntityTooSmall",
            "BadDigest",
            "XAmzContentSHA256Mismatch",
        ] {
            assert!(
                matches!(
                    classify_code(code, ObjectIdempotence::Read),
                    ContentObjectError::IntegrityMismatch { .. }
                ),
                "`{code}` must quarantine the upload"
            );
        }
    }

    #[test]
    fn throttling_is_retryable_and_an_unknown_code_is_merely_unavailable() {
        assert!(classify_code("SlowDown", ObjectIdempotence::Read).retryable());
        assert!(matches!(
            classify_code("Teleported", ObjectIdempotence::Read),
            ContentObjectError::Unavailable { .. }
        ));
    }
}
