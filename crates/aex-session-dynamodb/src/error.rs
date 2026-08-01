//! The one regional store error vocabulary, and the mapping from an AWS
//! condition onto it.
//!
//! Two properties matter. A cancelled transaction names the *participant* whose
//! condition failed, not the index that failed, because an index is meaningless
//! to a caller and drifts the moment a plan gains an action. And an ambiguous
//! write is its own type, [`StoreError::CommitAmbiguous`], carrying how to
//! resolve it — a blind retry after an ambiguous commit is the most dangerous
//! thing a transactional adapter can do, so it is not expressible here.

use std::time::Duration;

use aws_sdk_dynamodb::error::SdkError;
use aws_sdk_dynamodb::operation::transact_write_items::TransactWriteItemsError;
use aws_smithy_runtime_api::client::result::ServiceError;
use aws_smithy_types::error::metadata::ProvideErrorMetadata;

use crate::attr::{CodecError, Item};
use crate::plan::Participant;

/// How a caller must resolve an ambiguous write before doing anything else.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Resolution {
    /// Re-read the durable idempotency receipt for this operation.
    IdempotencyReceipt,
    /// Re-read the item the write targeted and compare its revision.
    TargetItem,
    /// `HeadObject` the final key rather than re-issuing the completion.
    ObjectHead,
}

impl Resolution {
    /// The imperative form, for a diagnostic.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::IdempotencyReceipt => "re-read the idempotency receipt",
            Self::TargetItem => "re-read the target item",
            Self::ObjectHead => "HeadObject the final key",
        }
    }
}

/// Every way a regional store operation can fail.
///
/// `Eq` is deliberately absent: `observed` carries raw `AttributeValue`s, which
/// hold floating-point-shaped numeric text and therefore have no total equality.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum StoreError {
    /// A condition expression was not satisfied.
    ///
    /// `observed` carries the row the condition saw, because every mutating
    /// request sets `ReturnValuesOnConditionCheckFailure = ALL_OLD` so the
    /// failure is diagnosable without a second read.
    #[error("the `{participant}` precondition was not satisfied")]
    PreconditionFailed {
        /// Which named participant lost.
        participant: Participant,
        /// The row as the condition saw it, when the service returned it.
        observed: Option<Box<Item>>,
    },
    /// Another transaction touched an item concurrently.
    #[error("the transaction contended with a concurrent writer")]
    Contended,
    /// The service asked the caller to slow down.
    #[error("throttled; retry after {}ms", retry_after.as_millis())]
    Throttled {
        /// How long to wait before the next attempt.
        retry_after: Duration,
    },
    /// The request was malformed. Always a bug in this crate or its caller.
    #[error("invalid request: {detail}")]
    Invalid {
        /// What the service objected to.
        detail: String,
    },
    /// The same idempotency identity was reused with a different payload.
    #[error("the idempotency identity was reused with a different intent")]
    IdempotencyConflict,
    /// The write may or may not have committed.
    #[error("the commit outcome is unknown; {}", resolve_by.as_str())]
    CommitAmbiguous {
        /// How the caller must establish the outcome.
        resolve_by: Resolution,
    },
    /// The service was unavailable.
    #[error("the store is unavailable: {detail}")]
    Unavailable {
        /// What the service reported.
        detail: String,
    },
    /// A table or index named by the composition does not exist.
    ///
    /// This fails startup and readiness closed. It is never a customer `404`.
    #[error("`{table}` does not exist; the composition is misconfigured")]
    Misconfigured {
        /// The logical table that is absent.
        table: String,
    },
    /// The caller's role is denied the action.
    #[error("the caller is denied this action")]
    Denied,
    /// A stored row could not be decoded.
    #[error("a stored row is corrupt: {0}")]
    Corrupt(#[from] CodecError),
    /// A key component was unusable.
    #[error("a key component is unusable: {0}")]
    Key(#[from] crate::component::KeyError),
    /// A preflight measurement exceeded a hard ceiling.
    #[error("the item measures {measured} bytes; the ceiling is {ceiling}")]
    ItemTooLarge {
        /// The measured encoded size.
        measured: usize,
        /// The ceiling that was exceeded.
        ceiling: usize,
    },
}

impl StoreError {
    /// Whether the bounded retry policy may re-issue the request.
    ///
    /// [`StoreError::CommitAmbiguous`] is deliberately **not** retryable: the
    /// caller resolves it by reading, never by writing again.
    #[must_use]
    pub const fn retryable(&self) -> bool {
        matches!(
            self,
            Self::Contended | Self::Throttled { .. } | Self::Unavailable { .. }
        )
    }
}

/// The bounded retry policy every regional adapter shares (D-26).
///
/// SDK adaptive retry is disabled in favour of this, so a retry storm cannot be
/// produced by an SDK default and the backoff is testable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RetryPolicy {
    /// Total attempts, the first included.
    pub attempts: u32,
    /// Base delay before the first retry.
    pub base: Duration,
    /// The delay ceiling.
    pub cap: Duration,
}

impl RetryPolicy {
    /// The pinned policy: three attempts, 50 ms base, 2 s cap.
    pub const PINNED: Self = Self {
        attempts: 3,
        base: Duration::from_millis(50),
        cap: Duration::from_secs(2),
    };

    /// The un-jittered delay before attempt `attempt`, counting from one.
    ///
    /// The caller applies full jitter over `[0, backoff]`; the policy owns the
    /// ceiling so a test can assert the bound without a random source.
    #[must_use]
    pub fn backoff(&self, attempt: u32) -> Duration {
        if attempt <= 1 {
            return Duration::ZERO;
        }
        let doublings = attempt - 2;
        let scaled = self
            .base
            .checked_mul(1_u32.checked_shl(doublings).unwrap_or(u32::MAX))
            .unwrap_or(self.cap);
        scaled.min(self.cap)
    }
}

/// Whether an ambiguous outcome on this operation is a write.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Idempotence {
    /// A read. An ambiguous outcome is simply unavailability.
    Read,
    /// A write. An ambiguous outcome must be resolved by reading.
    Write(Resolution),
}

/// Classifies any `DynamoDB` SDK failure that is not a transaction cancellation.
///
/// Cancellations carry per-participant reasons and go through
/// [`decode_cancellation`] instead.
pub fn classify<E, R>(error: &SdkError<E, R>, idempotence: Idempotence) -> StoreError
where
    E: ProvideErrorMetadata,
{
    match error {
        SdkError::ConstructionFailure(_) => StoreError::Invalid {
            detail: "the request could not be constructed".to_owned(),
        },
        SdkError::TimeoutError(_) | SdkError::DispatchFailure(_) | SdkError::ResponseError(_) => {
            match idempotence {
                Idempotence::Read => StoreError::Unavailable {
                    detail: "the request did not complete".to_owned(),
                },
                Idempotence::Write(resolve_by) => StoreError::CommitAmbiguous { resolve_by },
            }
        }
        SdkError::ServiceError(service) => classify_service(service, idempotence),
        _ => StoreError::Unavailable {
            detail: "an unrecognised SDK failure".to_owned(),
        },
    }
}

fn classify_service<E, R>(service: &ServiceError<E, R>, idempotence: Idempotence) -> StoreError
where
    E: ProvideErrorMetadata,
{
    let code = service.err().code().unwrap_or("Unknown").to_owned();
    classify_code(&code, idempotence)
}

/// Maps one service error code onto the store vocabulary.
///
/// Split out from [`classify`] so the whole table is assertable without
/// synthesising an `SdkError` for every row.
#[must_use]
pub fn classify_code(code: &str, idempotence: Idempotence) -> StoreError {
    match code {
        "ProvisionedThroughputExceededException"
        | "ThrottlingException"
        | "RequestLimitExceeded"
        | "SlowDown" => StoreError::Throttled {
            retry_after: RetryPolicy::PINNED.base,
        },
        "TransactionInProgressException" => StoreError::Contended,
        "IdempotentParameterMismatchException" => StoreError::IdempotencyConflict,
        "ResourceNotFoundException" => StoreError::Misconfigured {
            table: "an addressed table or index".to_owned(),
        },
        "ValidationException" | "SerializationException" => StoreError::Invalid {
            detail: format!("the service rejected the request as `{code}`"),
        },
        "ItemCollectionSizeLimitExceededException" => StoreError::Invalid {
            detail: "an item collection exceeded its size limit; there are no LSIs here".to_owned(),
        },
        "AccessDeniedException" | "NotAuthorized" => StoreError::Denied,
        "InternalServerError" | "InvalidEndpointException" | "ServiceUnavailable" => {
            StoreError::Unavailable {
                detail: format!("the service reported `{code}`"),
            }
        }
        "RequestTimeout" | "RequestTimeoutException" => match idempotence {
            Idempotence::Read => StoreError::Unavailable {
                detail: "the request timed out".to_owned(),
            },
            Idempotence::Write(resolve_by) => StoreError::CommitAmbiguous { resolve_by },
        },
        other => StoreError::Unavailable {
            detail: format!("the service reported `{other}`"),
        },
    }
}

/// The cancellation code `DynamoDB` uses for a failed condition.
pub const CONDITIONAL_CHECK_FAILED: &str = "ConditionalCheckFailed";

/// Decodes a `TransactionCanceledException` into a typed, participant-named
/// error.
///
/// The reason vector is positional: reason *i* belongs to action *i*. Mapping
/// it back to `participants[i]` is what makes the failure decodable by a caller
/// that never saw the compiled request.
///
/// A reason vector shorter than the plan, or a cancellation with no reasons at
/// all, is [`StoreError::Invalid`] rather than a guess.
#[must_use]
pub fn decode_cancellation(
    error: &TransactWriteItemsError,
    participants: &[Participant],
) -> StoreError {
    let TransactWriteItemsError::TransactionCanceledException(cancelled) = error else {
        return classify_code(
            error.code().unwrap_or("Unknown"),
            Idempotence::Write(Resolution::IdempotencyReceipt),
        );
    };
    let reasons = cancelled.cancellation_reasons();
    if reasons.is_empty() {
        return StoreError::Invalid {
            detail: "the transaction was cancelled with no reason vector".to_owned(),
        };
    }
    if reasons.len() != participants.len() {
        return StoreError::Invalid {
            detail: format!(
                "the transaction was cancelled with {} reasons for {} participants",
                reasons.len(),
                participants.len()
            ),
        };
    }
    for (index, reason) in reasons.iter().enumerate() {
        let code = reason.code().unwrap_or("None");
        if code == "None" {
            continue;
        }
        let participant = participants[index];
        return match code {
            CONDITIONAL_CHECK_FAILED => StoreError::PreconditionFailed {
                participant,
                observed: reason.item().cloned().map(Box::new),
            },
            "TransactionConflict" => StoreError::Contended,
            "ProvisionedThroughputExceeded" | "ThrottlingError" => StoreError::Throttled {
                retry_after: RetryPolicy::PINNED.base,
            },
            "ValidationError" => StoreError::Invalid {
                detail: format!(
                    "participant `{participant}` was rejected: {}",
                    reason.message().unwrap_or("no message")
                ),
            },
            "ItemCollectionSizeLimitExceeded" => StoreError::Invalid {
                detail: format!("participant `{participant}` exceeded an item collection limit"),
            },
            other => StoreError::Unavailable {
                detail: format!("participant `{participant}` was cancelled as `{other}`"),
            },
        };
    }
    StoreError::Invalid {
        detail: "the transaction was cancelled but every reason was `None`".to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use aws_sdk_dynamodb::operation::transact_write_items::TransactWriteItemsError;
    use aws_sdk_dynamodb::types::CancellationReason;
    use aws_sdk_dynamodb::types::error::TransactionCanceledException;

    use super::{
        Idempotence, Resolution, RetryPolicy, StoreError, classify_code, decode_cancellation,
    };
    use crate::plan::Participant;

    fn cancelled(codes: &[&str]) -> TransactWriteItemsError {
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

    const PLAN: [Participant; 3] = [
        Participant::new("authz.placement"),
        Participant::new("session.head"),
        Participant::new("work.dedupe"),
    ];

    #[test]
    fn a_failed_condition_names_the_participant_at_its_index() {
        let error = decode_cancellation(
            &cancelled(&["None", "ConditionalCheckFailed", "None"]),
            &PLAN,
        );
        assert!(
            matches!(
                error,
                StoreError::PreconditionFailed {
                    participant: Participant::SESSION_HEAD,
                    ..
                }
            ),
            "{error}"
        );
    }

    #[test]
    fn the_first_failing_participant_wins_when_several_fail() {
        let error = decode_cancellation(
            &cancelled(&["ConditionalCheckFailed", "ConditionalCheckFailed", "None"]),
            &PLAN,
        );
        match error {
            StoreError::PreconditionFailed { participant, .. } => {
                assert_eq!(participant.as_str(), "authz.placement");
            }
            other => panic!("{other}"),
        }
    }

    #[test]
    fn a_transaction_conflict_is_contention_not_a_precondition_failure() {
        let error =
            decode_cancellation(&cancelled(&["None", "TransactionConflict", "None"]), &PLAN);
        assert_eq!(error, StoreError::Contended);
        assert!(error.retryable());
    }

    #[test]
    fn a_reason_vector_that_does_not_match_the_plan_is_a_bug_not_a_guess() {
        let error = decode_cancellation(&cancelled(&["None", "ConditionalCheckFailed"]), &PLAN);
        assert!(matches!(error, StoreError::Invalid { .. }), "{error}");
    }

    #[test]
    fn an_all_none_reason_vector_is_a_bug() {
        let error = decode_cancellation(&cancelled(&["None", "None", "None"]), &PLAN);
        assert!(matches!(error, StoreError::Invalid { .. }), "{error}");
    }

    #[test]
    fn a_timeout_on_a_write_is_ambiguous_and_is_never_retryable() {
        let error = classify_code(
            "RequestTimeout",
            Idempotence::Write(Resolution::IdempotencyReceipt),
        );
        assert_eq!(
            error,
            StoreError::CommitAmbiguous {
                resolve_by: Resolution::IdempotencyReceipt
            }
        );
        assert!(!error.retryable(), "an ambiguous commit is never retried");
    }

    #[test]
    fn a_timeout_on_a_read_is_merely_unavailable() {
        let error = classify_code("RequestTimeout", Idempotence::Read);
        assert!(matches!(error, StoreError::Unavailable { .. }), "{error}");
        assert!(error.retryable());
    }

    #[test]
    fn an_absent_table_fails_closed_and_is_never_a_customer_not_found() {
        let error = classify_code("ResourceNotFoundException", Idempotence::Read);
        assert!(matches!(error, StoreError::Misconfigured { .. }), "{error}");
        assert!(!error.retryable());
    }

    #[test]
    fn a_reused_token_with_a_different_payload_is_a_conflict() {
        let error = classify_code("IdempotentParameterMismatchException", Idempotence::Read);
        assert_eq!(error, StoreError::IdempotencyConflict);
    }

    #[test]
    fn the_bounded_policy_never_exceeds_its_cap() {
        let policy = RetryPolicy::PINNED;
        assert_eq!(policy.backoff(1), Duration::ZERO);
        assert_eq!(policy.backoff(2), Duration::from_millis(50));
        assert_eq!(policy.backoff(3), Duration::from_millis(100));
        for attempt in 1..64 {
            assert!(policy.backoff(attempt) <= policy.cap);
        }
    }
}
