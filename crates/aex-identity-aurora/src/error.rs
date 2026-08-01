//! Transport failure to store failure, for the identity schema.
//!
//! The mapping is the control mapping plus the identity constraint names, so
//! there is exactly one place a constraint becomes a typed conflict.

use aex_identity_app::ports::StoreError;
use aex_rds_data::{CommitFailure, DataApiError};

/// Maps a classified transport failure onto the store vocabulary.
#[must_use]
pub fn map_store_error(error: DataApiError) -> StoreError {
    aex_control_aurora::map_store_error(error)
}

/// Maps a commit outcome onto the store vocabulary.
///
/// A lost commit is [`StoreError::Unknown`], which the application turns into a
/// retryable error naming the preassigned identity. It is never `Unavailable`:
/// the difference is whether a credential may already exist.
#[must_use]
pub fn map_commit_failure(failure: CommitFailure) -> StoreError {
    aex_control_aurora::map_commit_failure(failure)
}

/// The identity constraints an application maps a conflict by name from.
///
/// Listing them is what makes "map by constraint name" checkable: a constraint
/// the DDL declares and this list omits is a conflict nobody handles.
pub const CONFLICT_CONSTRAINTS: &[&str] = &[
    "at_scopes_ck",
    "dev_user_code_uk",
    "ec_lower_ck",
    "ext_provider_uk",
    "user_email_lower_ck",
    "user_email_uk",
];

#[cfg(test)]
mod tests {
    use super::{CONFLICT_CONSTRAINTS, map_commit_failure, map_store_error};
    use aex_identity_app::ports::StoreError;
    use aex_rds_data::{CommitFailure, DataApiError};

    #[test]
    fn every_identity_conflict_keeps_its_constraint_name() {
        for constraint in CONFLICT_CONSTRAINTS {
            assert_eq!(
                map_store_error(DataApiError::UniqueViolation {
                    constraint: (*constraint).to_owned()
                }),
                StoreError::Conflict {
                    constraint: (*constraint).to_owned()
                }
            );
        }
    }

    #[test]
    fn the_constraint_list_is_sorted_and_duplicate_free() {
        let mut sorted = CONFLICT_CONSTRAINTS.to_vec();
        sorted.sort_unstable();
        assert_eq!(sorted, CONFLICT_CONSTRAINTS);
        sorted.dedup();
        assert_eq!(sorted.len(), CONFLICT_CONSTRAINTS.len());
    }

    #[test]
    fn a_lost_commit_is_unknown_so_the_caller_reconciles_rather_than_re_mints() {
        assert_eq!(
            map_commit_failure(CommitFailure::Unknown(DataApiError::Timeout)),
            StoreError::Unknown
        );
        assert!(StoreError::Unknown.retryable());
    }
}
