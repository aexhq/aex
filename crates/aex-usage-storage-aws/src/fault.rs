//! How an AWS failure becomes a typed [`PortError`].
//!
//! The whole poison story rests on this mapping, so it is one function rather
//! than a judgement call at each call site.
//!
//! - A **service error** is a definite outcome: the request reached the service
//!   and the service refused it. Throttles and internal errors are retryable; a
//!   validation failure or a decode failure never is.
//! - A **transport error** — a timeout, a dispatch failure, an unparseable
//!   response — leaves a *read* retryable and a *write* ambiguous. Retrying an
//!   ambiguous commit blindly is how a duplicate settlement is produced
//!   (`U-29`), so the two are never collapsed into one "error".

use aex_usage_application::ports::PortError;
use aws_sdk_dynamodb::error::{ProvideErrorMetadata, SdkError};

/// Whether re-issuing the failed call is safe.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Idempotence {
    /// The call only read. Re-issuing it changes nothing.
    Read,
    /// The call may have committed. Re-issuing it blindly may double-apply.
    Write,
}

/// Service error codes that mean "try again", not "this is wrong".
const RETRYABLE: [&str; 8] = [
    "InternalServerError",
    "ItemCollectionSizeLimitExceededException",
    "LimitExceededException",
    "ProvisionedThroughputExceededException",
    "RequestLimitExceeded",
    "ServiceUnavailable",
    "ThrottlingException",
    "TransactionInProgressException",
];

/// Service error codes that mean the request itself can never succeed.
const TERMINAL: [&str; 3] = [
    "ValidationException",
    "AccessDeniedException",
    "SerializationException",
];

/// Maps one SDK failure onto the port vocabulary.
#[must_use]
pub fn classify<E, R>(
    what: &'static str,
    idempotence: Idempotence,
    error: &SdkError<E, R>,
) -> PortError
where
    E: ProvideErrorMetadata,
{
    match error {
        SdkError::ServiceError(inner) => service(what, inner.err()),
        SdkError::TimeoutError(_) | SdkError::DispatchFailure(_) | SdkError::ResponseError(_) => {
            match idempotence {
                Idempotence::Read => PortError::Unavailable {
                    what,
                    reason: "the request did not reach the service".to_owned(),
                },
                // Never folded into a retry: the commit may have happened.
                Idempotence::Write => PortError::CommitAmbiguous {
                    what,
                    reason: "the request may have committed before the transport failed".to_owned(),
                },
            }
        }
        _ => PortError::Unavailable {
            what,
            reason: "the request could not be constructed or sent".to_owned(),
        },
    }
}

/// Maps one definite service refusal.
fn service<E: ProvideErrorMetadata>(what: &'static str, error: &E) -> PortError {
    let code = error.code().unwrap_or("Unknown").to_owned();
    let reason = error
        .message()
        .map_or_else(|| code.clone(), |message| format!("{code}: {message}"));
    match code.as_str() {
        "ConditionalCheckFailedException" | "TransactionCanceledException" => {
            PortError::Conflict { what }
        }
        "ResourceNotFoundException" => PortError::NotFound { what, id: reason },
        code if TERMINAL.contains(&code) => PortError::Corrupt { what, reason },
        code if RETRYABLE.contains(&code) => PortError::Unavailable { what, reason },
        // An unrecognised service error is a definite refusal, not an ambiguous
        // commit: the service answered. Treating it as retryable is the safe
        // reading, because nothing was written.
        _ => PortError::Unavailable { what, reason },
    }
}

#[cfg(test)]
mod tests {
    use super::{Idempotence, classify};
    use aex_usage_application::ports::PortError;
    use aws_sdk_dynamodb::error::SdkError;
    use aws_sdk_dynamodb::operation::get_item::GetItemError;
    use aws_sdk_dynamodb::types::error::{
        InternalServerError, ProvisionedThroughputExceededException, ResourceNotFoundException,
    };
    use aws_smithy_runtime_api::client::orchestrator::HttpResponse;
    use aws_smithy_runtime_api::http::StatusCode;
    use aws_smithy_types::body::SdkBody;
    use aws_smithy_types::error::ErrorMetadata;

    /// The metadata the wire supplies. A locally built exception carries none,
    /// so a test that omitted it would exercise the unknown-code path instead of
    /// the one the service actually produces.
    fn meta(code: &str, message: &str) -> ErrorMetadata {
        ErrorMetadata::builder().code(code).message(message).build()
    }

    fn response() -> HttpResponse {
        HttpResponse::new(StatusCode::try_from(400).expect("status"), SdkBody::empty())
    }

    fn service(error: GetItemError) -> SdkError<GetItemError, HttpResponse> {
        SdkError::service_error(error, response())
    }

    #[test]
    fn a_throttle_is_retryable_and_an_internal_error_is_too() {
        let throttled = service(GetItemError::ProvisionedThroughputExceededException(
            ProvisionedThroughputExceededException::builder()
                .message("slow down")
                .meta(meta("ProvisionedThroughputExceededException", "slow down"))
                .build(),
        ));
        let error = classify("authority", Idempotence::Read, &throttled);
        assert!(error.retryable(), "{error}");
        assert!(!error.terminal());

        let internal = service(GetItemError::InternalServerError(
            InternalServerError::builder()
                .message("boom")
                .meta(meta("InternalServerError", "boom"))
                .build(),
        ));
        assert!(classify("authority", Idempotence::Read, &internal).retryable());
    }

    #[test]
    fn a_missing_table_is_terminal_rather_than_retried_forever() {
        let missing = service(GetItemError::ResourceNotFoundException(
            ResourceNotFoundException::builder()
                .message("no table")
                .meta(meta("ResourceNotFoundException", "no table"))
                .build(),
        ));
        let error = classify("authority", Idempotence::Read, &missing);
        assert!(matches!(error, PortError::NotFound { .. }), "{error}");
        assert!(error.terminal());
        assert!(!error.retryable());
    }

    #[test]
    fn a_validation_failure_is_terminal_because_the_same_request_never_succeeds() {
        let refused = service(GetItemError::generic(meta(
            "ValidationException",
            "the key is not a key",
        )));
        let error = classify("authority", Idempotence::Write, &refused);
        assert!(matches!(error, PortError::Corrupt { .. }), "{error}");
        assert!(error.terminal());
    }

    #[test]
    fn an_unrecognised_service_error_is_retryable_rather_than_ambiguous() {
        // The service answered, so nothing was written. Treating it as an
        // ambiguous commit would park a record that is simply retryable.
        let odd = service(GetItemError::generic(meta("SomethingNew", "unknown")));
        let error = classify("authority", Idempotence::Write, &odd);
        assert!(error.retryable(), "{error}");
    }

    #[test]
    fn a_transport_failure_is_retryable_on_a_read_and_ambiguous_on_a_write() {
        // The distinction is the whole duplicate-settlement guard: a write whose
        // outcome is unknown must never be blindly re-issued.
        let timeout: SdkError<GetItemError, HttpResponse> = SdkError::timeout_error("timed out");
        assert!(classify("authority", Idempotence::Read, &timeout).retryable());

        let ambiguous = classify("authority", Idempotence::Write, &timeout);
        assert!(matches!(ambiguous, PortError::CommitAmbiguous { .. }));
        assert!(
            !ambiguous.retryable(),
            "an ambiguous commit must not be retried in place"
        );
        assert!(!ambiguous.terminal());
    }
}
