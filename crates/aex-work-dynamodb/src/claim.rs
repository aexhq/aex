//! Claim, renew, fenced commit and poison.
//!
//! The fence is the point of the whole module. A worker claims by advancing a
//! monotonic counter, and every write it later makes conditions on still holding
//! that exact fence. A `ConditionalCheckFailed` on the fenced commit therefore
//! means the lease was stolen mid step, and the losing worker **discards its
//! prepared effect and does not retry**: a newer owner may already have
//! superseded it, so retrying would apply a decision the system has moved past.

use aex_session_dynamodb::attr::{n, s, stamp};
use aex_session_dynamodb::error::StoreError;
use aex_session_dynamodb::plan::{Participant, key};
use aex_wire::types::Timestamp;
use aws_sdk_dynamodb::types::builders::{PutBuilder, UpdateBuilder};

use crate::codec;
use crate::keys;

/// A worker's hold on one work record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkClaim {
    /// Which record.
    pub work_id: String,
    /// The fence the claim advanced to.
    pub fence: u64,
    /// Who holds it.
    pub owner: String,
    /// Which attempt this is.
    pub attempt: u64,
    /// When the lease expires.
    pub lease_expires_at: Timestamp,
}

/// Builds the claim update.
///
/// The condition admits a `pending` record, or a `claimed` one whose lease has
/// visibly expired. Expiry is compared against the caller's clock rather than
/// left to a TTL, because a lease that AWS has not got round to reclaiming is
/// still expired.
///
/// # Errors
///
/// [`StoreError::Key`] when the work identity could not enter a key.
pub fn claim(
    table: &str,
    work_id: &str,
    owner: &str,
    now: Timestamp,
    lease_until: Timestamp,
) -> Result<UpdateBuilder, StoreError> {
    let work_key = keys::work(work_id)?;
    Ok(aws_sdk_dynamodb::types::Update::builder()
        .table_name(table)
        .set_key(Some(key(&work_key.pk, &work_key.sk)))
        .condition_expression(
            "attribute_exists(pk) AND #state IN (:pending, :claimed) \
             AND attempt < maxAttempts AND (#state = :pending OR leaseExpiresAt < :now)",
        )
        .update_expression(
            "SET #state = :claimed, claimOwner = :owner, leaseExpiresAt = :lease, \
             fence = fence + :one, attempt = attempt + :one, updatedAt = :now",
        )
        .expression_attribute_names("#state", "state")
        .expression_attribute_values(":pending", s("pending"))
        .expression_attribute_values(":claimed", s("claimed"))
        .expression_attribute_values(":owner", s(owner.to_owned()))
        .expression_attribute_values(":lease", stamp(lease_until))
        .expression_attribute_values(":now", stamp(now))
        .expression_attribute_values(":one", n(1)))
}

/// Builds the lease renewal.
///
/// Renewal does **not** advance the fence: extending a lease is not a new claim,
/// and advancing the fence would invalidate the holder's own in-flight commit.
///
/// # Errors
///
/// As [`claim`].
pub fn renew(
    table: &str,
    hold: &WorkClaim,
    now: Timestamp,
    lease_until: Timestamp,
) -> Result<UpdateBuilder, StoreError> {
    let work_key = keys::work(&hold.work_id)?;
    Ok(aws_sdk_dynamodb::types::Update::builder()
        .table_name(table)
        .set_key(Some(key(&work_key.pk, &work_key.sk)))
        .condition_expression("fence = :fence AND claimOwner = :owner AND #state = :claimed")
        .update_expression("SET leaseExpiresAt = :lease, updatedAt = :now")
        .expression_attribute_names("#state", "state")
        .expression_attribute_values(":fence", n(hold.fence))
        .expression_attribute_values(":owner", s(hold.owner.clone()))
        .expression_attribute_values(":claimed", s("claimed"))
        .expression_attribute_values(":lease", stamp(lease_until))
        .expression_attribute_values(":now", stamp(now)))
}

/// Builds the fenced retirement that rides inside the domain's own transaction.
///
/// Retiring the record removes both due-index attributes, so the index holds
/// only outstanding work, and sets the TTL that reclaims the row 24 hours later.
///
/// # Errors
///
/// As [`claim`].
pub fn complete(
    table: &str,
    hold: &WorkClaim,
    now: Timestamp,
) -> Result<UpdateBuilder, StoreError> {
    let work_key = keys::work(&hold.work_id)?;
    Ok(aws_sdk_dynamodb::types::Update::builder()
        .table_name(table)
        .set_key(Some(key(&work_key.pk, &work_key.sk)))
        .condition_expression("fence = :fence AND claimOwner = :owner AND #state = :claimed")
        .update_expression(
            "SET #state = :done, updatedAt = :now, expiresAtEpochSeconds = :ttl \
             REMOVE dueShardPk, dueShardSk",
        )
        .expression_attribute_names("#state", "state")
        .expression_attribute_values(":fence", n(hold.fence))
        .expression_attribute_values(":owner", s(hold.owner.clone()))
        .expression_attribute_values(":claimed", s("claimed"))
        .expression_attribute_values(":done", s("done"))
        .expression_attribute_values(":now", stamp(now))
        .expression_attribute_values(":ttl", codec::retirement_ttl(now)))
}

/// Builds the poison update for a record whose attempt budget is spent.
///
/// # Errors
///
/// As [`claim`].
pub fn poison(
    table: &str,
    hold: &WorkClaim,
    failure_code: &str,
    now: Timestamp,
) -> Result<UpdateBuilder, StoreError> {
    let work_key = keys::work(&hold.work_id)?;
    Ok(aws_sdk_dynamodb::types::Update::builder()
        .table_name(table)
        .set_key(Some(key(&work_key.pk, &work_key.sk)))
        .condition_expression("fence = :fence AND claimOwner = :owner AND attempt >= maxAttempts")
        .update_expression(
            "SET #state = :poisoned, lastFailure = :failure, updatedAt = :now \
             REMOVE dueShardPk, dueShardSk",
        )
        .expression_attribute_names("#state", "state")
        .expression_attribute_values(":fence", n(hold.fence))
        .expression_attribute_values(":owner", s(hold.owner.clone()))
        .expression_attribute_values(":poisoned", s("poisoned"))
        .expression_attribute_values(":failure", s(failure_code.to_owned()))
        .expression_attribute_values(":now", stamp(now)))
}

/// Builds the immutable enqueue of one work record.
///
/// # Errors
///
/// [`StoreError`] when the payload is refused or a key component is unusable.
pub fn enqueue(table: &str, record: &codec::WorkRecord) -> Result<PutBuilder, StoreError> {
    let item = codec::encode_work(record).map_err(|error| StoreError::Invalid {
        detail: error.to_string(),
    })?;
    Ok(aws_sdk_dynamodb::types::Put::builder()
        .table_name(table)
        .set_item(Some(item))
        .condition_expression(aex_session_dynamodb::plan::IMMUTABLE))
}

/// Builds the dedupe claim written in the same transaction as the record.
///
/// # Errors
///
/// [`StoreError::Key`] when the digest could not enter a key.
pub fn enqueue_dedupe(table: &str, record: &codec::WorkRecord) -> Result<PutBuilder, StoreError> {
    let item = codec::encode_dedupe(&record.dedupe_key, &record.work_id, record.created_at)?;
    Ok(aws_sdk_dynamodb::types::Put::builder()
        .table_name(table)
        .set_item(Some(item))
        .condition_expression(aex_session_dynamodb::plan::IMMUTABLE))
}

/// The participant a fenced retirement commits as.
pub const WAKE_DONE: Participant = Participant::WORK_WAKE_DONE;

/// The participant a dedupe claim commits as.
pub const DEDUPE: Participant = Participant::WORK_DEDUPE;

#[cfg(test)]
mod tests {
    use aex_wire::types::Timestamp;

    use super::{WorkClaim, claim, complete, poison, renew};

    const TABLE: &str = "dev-eu-west-1-regional-work";

    fn stamp(millis: i64) -> Timestamp {
        Timestamp::from_unix_millis(millis).expect("in range")
    }

    fn hold() -> WorkClaim {
        WorkClaim {
            work_id: "wrk_01".to_owned(),
            fence: 3,
            owner: "worker-1".to_owned(),
            attempt: 1,
            lease_expires_at: stamp(30_000),
        }
    }

    fn condition(builder: aws_sdk_dynamodb::types::builders::UpdateBuilder) -> String {
        builder
            .build()
            .expect("a complete update")
            .condition_expression()
            .expect("conditional")
            .to_owned()
    }

    fn update(builder: aws_sdk_dynamodb::types::builders::UpdateBuilder) -> String {
        builder
            .build()
            .expect("a complete update")
            .update_expression()
            .to_owned()
    }

    #[test]
    fn a_claim_admits_a_pending_record_or_an_expired_lease_and_nothing_else() {
        let expression =
            condition(claim(TABLE, "wrk_01", "worker-1", stamp(0), stamp(30_000)).expect("builds"));
        assert!(expression.contains("#state IN (:pending, :claimed)"));
        assert!(expression.contains("attempt < maxAttempts"));
        assert!(expression.contains("(#state = :pending OR leaseExpiresAt < :now)"));
    }

    #[test]
    fn a_claim_advances_the_fence_and_the_attempt_together() {
        let expression =
            update(claim(TABLE, "wrk_01", "worker-1", stamp(0), stamp(1)).expect("builds"));
        assert!(expression.contains("fence = fence + :one"));
        assert!(expression.contains("attempt = attempt + :one"));
    }

    #[test]
    fn a_renewal_never_advances_the_fence() {
        let expression = update(renew(TABLE, &hold(), stamp(0), stamp(60_000)).expect("builds"));
        assert!(
            !expression.contains("fence"),
            "extending a lease is not a new claim: {expression}"
        );
        assert!(expression.contains("leaseExpiresAt = :lease"));
    }

    #[test]
    fn every_post_claim_write_is_fenced_by_the_exact_claim() {
        for expression in [
            condition(renew(TABLE, &hold(), stamp(0), stamp(1)).expect("builds")),
            condition(complete(TABLE, &hold(), stamp(0)).expect("builds")),
            condition(poison(TABLE, &hold(), "boom", stamp(0)).expect("builds")),
        ] {
            assert!(expression.contains("fence = :fence"), "{expression}");
            assert!(expression.contains("claimOwner = :owner"), "{expression}");
        }
    }

    #[test]
    fn retiring_and_poisoning_both_leave_the_due_index() {
        for expression in [
            update(complete(TABLE, &hold(), stamp(0)).expect("builds")),
            update(poison(TABLE, &hold(), "boom", stamp(0)).expect("builds")),
        ] {
            assert!(
                expression.contains("REMOVE dueShardPk, dueShardSk"),
                "{expression}"
            );
        }
    }

    #[test]
    fn only_retirement_sets_the_reclamation_ttl() {
        assert!(
            update(complete(TABLE, &hold(), stamp(0)).expect("builds"))
                .contains("expiresAtEpochSeconds = :ttl")
        );
        assert!(
            !update(poison(TABLE, &hold(), "boom", stamp(0)).expect("builds"))
                .contains("expiresAtEpochSeconds"),
            "a poisoned record is forensic evidence and is never reclaimed on a timer"
        );
    }

    #[test]
    fn poisoning_requires_the_attempt_budget_to_be_spent() {
        assert!(
            condition(poison(TABLE, &hold(), "boom", stamp(0)).expect("builds"))
                .contains("attempt >= maxAttempts")
        );
    }

    #[test]
    fn a_work_identity_carrying_the_separator_cannot_build_any_expression() {
        assert!(claim(TABLE, "wrk#evil", "worker-1", stamp(0), stamp(1)).is_err());
    }
}
