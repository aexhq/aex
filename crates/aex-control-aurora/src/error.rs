//! Transport failure to store failure.
//!
//! The mapping is total and deterministic: a constraint violation keeps the
//! exact constraint name, so the application maps a conflict by name rather than
//! by guessing from a message. A [`CommitFailure::Unknown`] becomes
//! [`StoreError::Unknown`] and nothing else — this crate never re-executes and
//! never regenerates an identity.

use aex_control_app::ports::StoreError;
use aex_rds_data::{CommitFailure, DataApiError};

/// Maps a classified transport failure onto the store vocabulary.
#[must_use]
pub fn map_store_error(error: DataApiError) -> StoreError {
    match error {
        DataApiError::UniqueViolation { constraint }
        | DataApiError::ForeignKeyViolation { constraint }
        | DataApiError::CheckViolation { constraint } => StoreError::Conflict { constraint },
        DataApiError::NotNullViolation { column } => StoreError::Conflict {
            constraint: column_not_null(&column),
        },
        DataApiError::IntegrityConstraintViolation { message } => StoreError::Conflict {
            constraint: integrity_constraint_name(&message),
        },
        DataApiError::PermissionDenied => StoreError::PermissionDenied,
        DataApiError::Resuming
        | DataApiError::Timeout
        | DataApiError::Throttled
        | DataApiError::Serialization
        | DataApiError::Deadlock
        | DataApiError::Unavailable { .. }
        | DataApiError::DeadlineExceeded => StoreError::Unavailable,
        // A transaction the service no longer knows about may or may not have
        // committed; treating it as "nothing happened" is the mistake this arm
        // exists to prevent.
        DataApiError::TransactionNotFound | DataApiError::TransactionExpired => StoreError::Unknown,
        DataApiError::Decode(reason) => StoreError::Decode(reason.to_string()),
        DataApiError::ResultTooLarge { bytes } => StoreError::Fatal(format!(
            "a read returned {bytes} bytes; the page bound is wrong"
        )),
        DataApiError::FieldTooLarge { index, bytes } => StoreError::Fatal(format!(
            "column {index} returned {bytes} bytes; the projection is wrong"
        )),
        DataApiError::UndefinedObject { object } => {
            StoreError::Fatal(format!("undefined object `{object}`; the schema is behind"))
        }
        DataApiError::Fatal { code, message } => StoreError::Fatal(match code {
            Some(code) => format!("{code}: {message}"),
            None => message,
        }),
    }
}

/// Maps a commit outcome onto the store vocabulary.
///
/// A rolled-back commit is a conflict the caller can act on; an unknown one is
/// [`StoreError::Unknown`], which the application turns into a retryable error
/// naming the preassigned identity.
#[must_use]
pub fn map_commit_failure(failure: CommitFailure) -> StoreError {
    match failure {
        CommitFailure::RolledBack(error) => map_store_error(error),
        CommitFailure::Unknown(_) => StoreError::Unknown,
    }
}

/// The synthetic constraint name a not-null violation reports as.
fn column_not_null(column: &str) -> String {
    format!("{column}_not_null")
}

/// The constraint a `RAISE` from a constraint trigger names.
///
/// The two triggers this schema declares raise
/// `integrity_constraint_violation`, so the message is the only place their
/// identity survives. Matching on the text is deliberate and narrow: both
/// messages are ours, and both are asserted by the migration suite.
fn integrity_constraint_name(message: &str) -> String {
    let lowered = message.to_ascii_lowercase();
    if lowered.contains("no active owner") {
        return "membership_owner_required".to_owned();
    }
    if lowered.contains("placement is immutable") {
        return "wsp_placement_immutable".to_owned();
    }
    "integrity_constraint_violation".to_owned()
}

#[cfg(test)]
mod tests {
    use super::{map_commit_failure, map_store_error};
    use aex_control_app::ports::StoreError;
    use aex_rds_data::{CommitFailure, DataApiError};

    #[test]
    fn a_constraint_violation_keeps_its_exact_name() {
        assert_eq!(
            map_store_error(DataApiError::UniqueViolation {
                constraint: "org_slug_uk".to_owned()
            }),
            StoreError::Conflict {
                constraint: "org_slug_uk".to_owned()
            }
        );
        assert_eq!(
            map_store_error(DataApiError::CheckViolation {
                constraint: "wsp_status_ck".to_owned()
            }),
            StoreError::Conflict {
                constraint: "wsp_status_ck".to_owned()
            }
        );
    }

    #[test]
    fn both_constraint_triggers_are_named_from_their_message() {
        assert_eq!(
            map_store_error(DataApiError::IntegrityConstraintViolation {
                message: "ERROR: organization 7c1f has no active owner".to_owned()
            }),
            StoreError::Conflict {
                constraint: "membership_owner_required".to_owned()
            }
        );
        assert_eq!(
            map_store_error(DataApiError::IntegrityConstraintViolation {
                message: "ERROR: workspace placement is immutable".to_owned()
            }),
            StoreError::Conflict {
                constraint: "wsp_placement_immutable".to_owned()
            }
        );
    }

    #[test]
    fn a_transient_transport_failure_is_unavailable_and_never_unknown() {
        for error in [
            DataApiError::Resuming,
            DataApiError::Timeout,
            DataApiError::Throttled,
            DataApiError::Serialization,
            DataApiError::Deadlock,
            DataApiError::DeadlineExceeded,
            DataApiError::Unavailable {
                message: "reset".to_owned(),
            },
        ] {
            assert_eq!(
                map_store_error(error.clone()),
                StoreError::Unavailable,
                "{error:?}"
            );
        }
    }

    #[test]
    fn a_lost_transaction_is_unknown_rather_than_unavailable() {
        assert_eq!(
            map_store_error(DataApiError::TransactionNotFound),
            StoreError::Unknown
        );
        assert_eq!(
            map_store_error(DataApiError::TransactionExpired),
            StoreError::Unknown
        );
    }

    #[test]
    fn a_lost_commit_is_unknown_and_a_rollback_keeps_its_cause() {
        assert_eq!(
            map_commit_failure(CommitFailure::Unknown(DataApiError::Timeout)),
            StoreError::Unknown
        );
        assert_eq!(
            map_commit_failure(CommitFailure::RolledBack(DataApiError::UniqueViolation {
                constraint: "mem_org_user_uk".to_owned()
            })),
            StoreError::Conflict {
                constraint: "mem_org_user_uk".to_owned()
            }
        );
    }

    #[test]
    fn a_privilege_failure_is_never_retryable() {
        let mapped = map_store_error(DataApiError::PermissionDenied);
        assert_eq!(mapped, StoreError::PermissionDenied);
        assert!(!mapped.retryable());
    }

    #[test]
    fn a_breached_page_bound_is_fatal_rather_than_silently_truncated() {
        assert!(matches!(
            map_store_error(DataApiError::ResultTooLarge { bytes: 1_000_000 }),
            StoreError::Fatal(_)
        ));
    }
}
