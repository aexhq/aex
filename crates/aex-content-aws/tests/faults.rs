//! Failure-path cases for the content object adapter.

mod support;

use aex_content_aws::errors::{
    ContentObjectError, ObjectIdempotence, PRECONDITION_FAILED, classify_code,
};
use aex_content_aws::multipart::ProviderPart;
use aex_session_dynamodb::error::Resolution;

use support::{manifest, provider_parts};

#[test]
fn an_ambiguous_completion_is_never_re_issued_and_never_aborted() {
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
    assert!(!error.retryable());
    assert!(
        error.to_string().contains("HeadObject"),
        "the type has to say how to resolve it, or a caller will guess: {error}"
    );
}

#[test]
fn a_precondition_failure_is_the_signal_the_caller_resolves_rather_than_a_hard_failure() {
    // `put_immutable` and `delete_fenced` both inspect the raw code before
    // classification, because for them a 412 is an outcome and not an error.
    assert_eq!(PRECONDITION_FAILED, "PreconditionFailed");
    assert_eq!(
        classify_code(PRECONDITION_FAILED, ObjectIdempotence::Read),
        ContentObjectError::Contended,
        "anywhere that does not resolve it itself, a lost condition is contention"
    );
}

#[test]
fn a_truncated_part_listing_is_an_integrity_failure_and_never_a_short_completion() {
    // The adapter refuses to complete from a partial list; this asserts the
    // manifest checker agrees, so both layers fail closed.
    let mut partial = provider_parts();
    partial.truncate(1);
    let error = manifest()
        .agrees_with(&partial)
        .expect_err("a short listing");
    assert!(
        matches!(error, ContentObjectError::IntegrityMismatch { .. }),
        "{error}"
    );
}

#[test]
fn a_part_the_provider_never_checksummed_stops_the_completion() {
    let mut unverified = provider_parts();
    unverified[0].checksum_sha256 = None;
    assert!(manifest().agrees_with(&unverified).is_err());
}

#[test]
fn a_reordered_part_listing_is_caught_by_number_before_it_is_caught_by_bytes() {
    let held = vec![
        ProviderPart {
            part_number: 2,
            ..provider_parts()[1].clone()
        },
        ProviderPart {
            part_number: 1,
            ..provider_parts()[0].clone()
        },
    ];
    let error = manifest().agrees_with(&held).expect_err("reordered");
    assert!(error.to_string().contains("number"), "{error}");
}

#[test]
fn a_missing_object_never_becomes_a_not_found_and_never_becomes_a_five_hundred() {
    let error = classify_code("NoSuchKey", ObjectIdempotence::Read);
    assert!(
        matches!(error, ContentObjectError::ContentMissing { .. }),
        "{error}"
    );
    assert!(
        error.to_string().contains("reuploaded"),
        "the message has to name the only recovery there is: {error}"
    );
}

#[test]
fn an_already_aborted_upload_and_an_already_deleted_object_are_both_idempotent_successes() {
    // Both are handled by code inspection in the adapter rather than by the
    // classifier, which this asserts by exclusion: the classifier would
    // otherwise turn NoSuchUpload into an unavailability.
    assert!(matches!(
        classify_code("NoSuchUpload", ObjectIdempotence::Read),
        ContentObjectError::Unavailable { .. }
    ));
}
