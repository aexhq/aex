//! The `DynamoDB`-backed [`LeaseStore`].
//!
//! Two rules are carried here rather than by review.
//!
//! - **A claim returns the head in the same conditional write.** The activation-pool
//!   evaluation measured redundant strongly-consistent reads dominating the cost of
//!   becoming an owner, so there is exactly one round trip and `ReturnValues: ALL_NEW`
//!   carries the head back.
//! - **A renewal never moves the fence.** Extending a lease proves nothing changed hands,
//!   and advancing the fence would fence out the very owner doing the renewing.

use aex_brain_application::ports::{
    BoxFuture, Claim, ClaimError, LeaseStore, ReleaseDisposition, StoreError,
};
use aex_brain_domain::ids::{AgentKey, Fence, OwnerToken, Timestamp};
use aex_session_dynamodb::attr::{n, s, stamp};
use aex_session_dynamodb::plan::key as item_key;
use aws_sdk_dynamodb::operation::update_item::UpdateItemError;
use aws_sdk_dynamodb::types::ReturnValue;

use crate::journal::{BrainStore, store_key_error, translate_error, transport};
use crate::{control, keys, translate};

/// How long a claimant waits past a visibly expired lease before stealing it.
///
/// Clock skew therefore changes *when* a steal happens, never *whether* a stale owner can
/// still write: the fence decides that, and it decides it without reference to any clock.
pub const STEAL_GRACE: core::time::Duration = core::time::Duration::from_secs(5);

fn is_condition<R>(error: &aws_sdk_dynamodb::error::SdkError<UpdateItemError, R>) -> bool {
    error
        .as_service_error()
        .is_some_and(UpdateItemError::is_conditional_check_failed_exception)
}

impl LeaseStore for BrainStore {
    fn claim<'a>(
        &'a self,
        key: &'a AgentKey,
        owner: OwnerToken,
        ttl: core::time::Duration,
        now: Timestamp,
    ) -> BoxFuture<'a, Result<Claim, ClaimError>> {
        Box::pin(async move {
            let control_key =
                keys::control(key).map_err(|error| ClaimError::Store(store_key_error(&error)))?;
            let ttl_millis = i64::try_from(ttl.as_millis()).unwrap_or(i64::MAX);
            let grace = i64::try_from(STEAL_GRACE.as_millis()).unwrap_or(i64::MAX);
            let expires = now.plus_millis(ttl_millis);
            let stealable_before = now.millis().saturating_sub(grace).max(0);
            let wire_now = translate::at(now, "now")
                .map_err(|error| ClaimError::Store(translate_error(&error)))?;
            let wire_expires = translate::at(expires, "expires")
                .map_err(|error| ClaimError::Store(translate_error(&error)))?;
            let wire_stealable =
                translate::at(Timestamp::from_millis(stealable_before), "stealable")
                    .map_err(|error| ClaimError::Store(translate_error(&error)))?;

            let output = self
                .client()
                .update_item()
                .table_name(self.table())
                .set_key(Some(item_key(&control_key.pk, &control_key.sk)))
                .condition_expression(
                    "attribute_exists(pk) AND attribute_not_exists(finishReason) \
                     AND (attribute_not_exists(leaseExpiresAt) OR leaseExpiresAt < :stealable \
                          OR claimOwner = :owner)",
                )
                .update_expression(
                    "SET claimOwner = :owner, leaseExpiresAt = :expires, updatedAt = :now \
                     ADD fence :one",
                )
                .expression_attribute_values(":owner", s(owner.0.as_hyphenated().to_string()))
                .expression_attribute_values(":stealable", stamp(wire_stealable))
                .expression_attribute_values(":expires", stamp(wire_expires))
                .expression_attribute_values(":now", stamp(wire_now))
                .expression_attribute_values(":one", n(1))
                // The head comes back with the write, so becoming an owner is one round
                // trip rather than a write plus a strongly-consistent read.
                .return_values(ReturnValue::AllNew)
                .send()
                .await
                .map_err(|error| {
                    if is_condition(&error) {
                        ClaimError::HeldByOther {
                            expires_at: Timestamp::from_millis(0),
                        }
                    } else {
                        ClaimError::Store(transport("claim", &error))
                    }
                })?;

            let attributes = output.attributes().ok_or_else(|| {
                ClaimError::Store(StoreError::Undecodable {
                    location: "agent control".to_owned(),
                    reason: "the conditional write returned no attributes".to_owned(),
                })
            })?;
            let head = control::decode(attributes, *key, Vec::new()).map_err(|error| {
                ClaimError::Store(StoreError::Undecodable {
                    location: "agent control".to_owned(),
                    reason: error.to_string(),
                })
            })?;
            if head.finish.is_some() {
                return Err(ClaimError::Terminal);
            }
            Ok(Claim {
                key: *key,
                owner,
                fence: head.fence,
                expires_at: expires,
                head,
            })
        })
    }

    fn renew<'a>(
        &'a self,
        claim: &'a Claim,
        ttl: core::time::Duration,
        now: Timestamp,
    ) -> BoxFuture<'a, Result<Claim, ClaimError>> {
        Box::pin(async move {
            let control_key = keys::control(&claim.key)
                .map_err(|error| ClaimError::Store(store_key_error(&error)))?;
            let ttl_millis = i64::try_from(ttl.as_millis()).unwrap_or(i64::MAX);
            let expires = now.plus_millis(ttl_millis);
            let wire_now = translate::at(now, "now")
                .map_err(|error| ClaimError::Store(translate_error(&error)))?;
            let wire_expires = translate::at(expires, "expires")
                .map_err(|error| ClaimError::Store(translate_error(&error)))?;

            self.client()
                .update_item()
                .table_name(self.table())
                .set_key(Some(item_key(&control_key.pk, &control_key.sk)))
                .condition_expression("fence = :fence AND claimOwner = :owner")
                // Deliberately no `ADD fence`: a renewal proves nothing changed hands.
                .update_expression("SET leaseExpiresAt = :expires, updatedAt = :now")
                .expression_attribute_values(":fence", n(claim.fence.0))
                .expression_attribute_values(":owner", s(claim.owner.0.as_hyphenated().to_string()))
                .expression_attribute_values(":expires", stamp(wire_expires))
                .expression_attribute_values(":now", stamp(wire_now))
                .send()
                .await
                .map_err(|error| {
                    if is_condition(&error) {
                        ClaimError::Fenced {
                            current: Fence(claim.fence.0.saturating_add(1)),
                        }
                    } else {
                        ClaimError::Store(transport("renew", &error))
                    }
                })?;
            Ok(Claim {
                expires_at: expires,
                ..claim.clone()
            })
        })
    }

    fn release(
        &self,
        claim: Claim,
        disposition: ReleaseDisposition,
    ) -> BoxFuture<'_, Result<(), StoreError>> {
        Box::pin(async move {
            let control_key = keys::control(&claim.key).map_err(|error| store_key_error(&error))?;
            // Drain sets the expiry to zero so a surviving task claims immediately instead
            // of waiting out the whole TTL. Every other disposition simply stops renewing.
            let expiry = match disposition {
                ReleaseDisposition::Drain => 0,
                ReleaseDisposition::Committed
                | ReleaseDisposition::Parked
                | ReleaseDisposition::Abandoned => claim.expires_at.millis().max(0),
            };
            let wire_expiry = translate::at(Timestamp::from_millis(expiry), "expiry")
                .map_err(|error| translate_error(&error))?;
            match self
                .client()
                .update_item()
                .table_name(self.table())
                .set_key(Some(item_key(&control_key.pk, &control_key.sk)))
                .condition_expression("fence = :fence AND claimOwner = :owner")
                .update_expression("SET leaseExpiresAt = :expiry REMOVE claimOwner")
                .expression_attribute_values(":fence", n(claim.fence.0))
                .expression_attribute_values(":owner", s(claim.owner.0.as_hyphenated().to_string()))
                .expression_attribute_values(":expiry", stamp(wire_expiry))
                .send()
                .await
            {
                Ok(_) => Ok(()),
                // A release that loses its fence has already been superseded: the new owner
                // holds the lease, so there is nothing left to give up.
                Err(error) if is_condition(&error) => Ok(()),
                Err(error) => Err(transport("release", &error)),
            }
        })
    }
}
