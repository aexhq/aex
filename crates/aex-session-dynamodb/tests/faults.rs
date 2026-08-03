//! Failure-path cases: cancellation decoding, the error-mapping table, and the
//! ambiguous-commit contract.
//!
//! The property under test throughout is that a failure keeps its meaning. A
//! precondition failure names its participant, an ambiguous commit stays
//! ambiguous, and nothing here is ever remapped to a generic `500`.

use aex_session_dynamodb::error::{
    Idempotence, Resolution, RetryPolicy, StoreError, classify_code, decode_cancellation,
};
use aex_session_dynamodb::plan::Participant;
use aws_sdk_dynamodb::operation::transact_write_items::TransactWriteItemsError;
use aws_sdk_dynamodb::types::CancellationReason;
use aws_sdk_dynamodb::types::error::TransactionCanceledException;

/// One row of the error-mapping table: a service code and the predicate its
/// typed error must satisfy.
type Row = (&'static str, fn(&StoreError) -> bool);

fn cancellation(codes: &[&str]) -> TransactWriteItemsError {
    let reasons = codes
        .iter()
        .map(|code| CancellationReason::builder().code(*code).build())
        .collect::<Vec<_>>();
    TransactWriteItemsError::TransactionCanceledException(
        TransactionCanceledException::builder()
            .set_cancellation_reasons(Some(reasons))
            .build(),
    )
}

/// One reason vector for the compiled admission plan, with `index` failing.
#[test]
fn a_transaction_conflict_is_retryable_and_a_validation_error_is_not() {
    let participants = [Participant::SESSION_HEAD, Participant::SESSION_RUN];
    let contended = decode_cancellation(
        &cancellation(&["TransactionConflict", "None"]),
        &participants,
    );
    assert_eq!(contended, StoreError::Contended);
    assert!(contended.retryable());

    let invalid = decode_cancellation(&cancellation(&["None", "ValidationError"]), &participants);
    assert!(matches!(invalid, StoreError::Invalid { .. }), "{invalid}");
    assert!(!invalid.retryable());
}

#[test]
fn the_error_mapping_table_holds_for_every_row_the_plan_declares() {
    let write = Idempotence::Write(Resolution::IdempotencyReceipt);
    let rows: [Row; 9] = [
        ("ProvisionedThroughputExceededException", |error| {
            matches!(error, StoreError::Throttled { .. })
        }),
        ("ThrottlingException", |error| {
            matches!(error, StoreError::Throttled { .. })
        }),
        ("RequestLimitExceeded", |error| {
            matches!(error, StoreError::Throttled { .. })
        }),
        ("TransactionInProgressException", |error| {
            matches!(error, StoreError::Contended)
        }),
        ("IdempotentParameterMismatchException", |error| {
            matches!(error, StoreError::IdempotencyConflict)
        }),
        ("ValidationException", |error| {
            matches!(error, StoreError::Invalid { .. })
        }),
        ("ItemCollectionSizeLimitExceededException", |error| {
            matches!(error, StoreError::Invalid { .. })
        }),
        ("ResourceNotFoundException", |error| {
            matches!(error, StoreError::Misconfigured { .. })
        }),
        ("InternalServerError", |error| {
            matches!(
                error,
                StoreError::CommitAmbiguous {
                    resolve_by: Resolution::IdempotencyReceipt
                }
            )
        }),
    ];
    for (code, expected) in rows {
        let error = classify_code(code, write);
        assert!(expected(&error), "`{code}` mapped to {error}");
    }
}

#[test]
fn provider_internal_errors_are_ambiguous_only_after_a_write() {
    let read = classify_code("InternalServerError", Idempotence::Read);
    assert!(matches!(read, StoreError::Unavailable { .. }), "{read}");

    let write = classify_code(
        "InternalServerError",
        Idempotence::Write(Resolution::TargetItem),
    );
    assert_eq!(
        write,
        StoreError::CommitAmbiguous {
            resolve_by: Resolution::TargetItem
        }
    );
    assert!(
        !write.retryable(),
        "a provider 500 write is never replayed blindly"
    );
}

#[test]
fn an_invalid_endpoint_is_definitive_unavailability_even_for_a_write() {
    for idempotence in [
        Idempotence::Read,
        Idempotence::Write(Resolution::TargetItem),
    ] {
        let error = classify_code("InvalidEndpointException", idempotence);
        assert!(matches!(error, StoreError::Unavailable { .. }), "{error}");
        assert!(error.retryable());
    }
}

#[test]
fn a_denied_request_is_never_masked_as_a_not_found() {
    let error = classify_code("AccessDeniedException", Idempotence::Read);
    assert_eq!(error, StoreError::Denied);
    assert!(!matches!(error, StoreError::Misconfigured { .. }));
}

#[test]
fn an_ambiguous_write_is_never_retryable_and_always_names_how_to_resolve() {
    for resolution in [
        Resolution::IdempotencyReceipt,
        Resolution::TargetItem,
        Resolution::ObjectHead,
    ] {
        let error = classify_code("RequestTimeout", Idempotence::Write(resolution));
        assert_eq!(
            error,
            StoreError::CommitAmbiguous {
                resolve_by: resolution
            }
        );
        assert!(!error.retryable());
        assert!(error.to_string().contains(resolution.as_str()));
    }
}

#[test]
fn the_bounded_retry_policy_is_the_pinned_one() {
    let policy = RetryPolicy::PINNED;
    assert_eq!(policy.attempts, 3);
    assert_eq!(policy.base, std::time::Duration::from_millis(50));
    assert_eq!(policy.cap, std::time::Duration::from_secs(2));
}

#[test]
fn a_cancellation_whose_reason_vector_does_not_match_the_plan_is_a_bug_not_a_guess() {
    let participants = [Participant::SESSION_HEAD, Participant::SESSION_RUN];
    let error = decode_cancellation(&cancellation(&["ConditionalCheckFailed"]), &participants);
    assert!(matches!(error, StoreError::Invalid { .. }), "{error}");
}
