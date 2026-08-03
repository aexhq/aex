//! The `DynamoDB`-backed [`LeaseStore`].
//!
//! Two rules are carried here rather than by review.
//!
//! - **A claim returns the agent head in the conditional write, then reads the session
//!   head strongly consistently.** The second read is required: workspace, organization
//!   and deletion epoch are session facts and cannot be fixed at process composition.
//! - **A renewal never moves the fence.** Extending a lease proves nothing changed hands,
//!   and advancing the fence would fence out the very owner doing the renewing.

use aex_brain_application::ports::{
    BoxFuture, Claim, ClaimError, LeaseStore, ReleaseDisposition, SessionAuthority, StoreError,
};
use aex_brain_domain::ids::{AgentKey, Fence, OwnerToken, Timestamp};
use aex_session_dynamodb::attr::{Row, n, s, stamp};
use aex_session_dynamodb::plan::key as item_key;
use aex_wire::ids::{SessionId, WorkspaceId};
use aws_sdk_dynamodb::operation::update_item::UpdateItemError;
use aws_sdk_dynamodb::types::ReturnValue;

use crate::journal::{BrainStore, store_key_error, translate_error, transport};
use crate::{control, keys, translate};

const RENEW_ORDER: &[aex_session_dynamodb::plan::Participant] = &[
    aex_session_dynamodb::plan::Participant::SESSION_HEAD_GUARD,
    aex_session_dynamodb::plan::Participant::AGENT_CONTROL,
];

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

async fn load_session_authority(
    store: &BrainStore,
    key: &AgentKey,
) -> Result<SessionAuthority, ClaimError> {
    let session = translate::session(key.session)
        .map_err(|error| ClaimError::Store(translate_error(&error)))?;
    let head_key = aex_session_dynamodb::keys::head(session);
    let output = store
        .client()
        .get_item()
        .table_name(store.table())
        .set_key(Some(item_key(&head_key.pk, &head_key.sk)))
        .consistent_read(true)
        .send()
        .await
        .map_err(|error| ClaimError::Store(transport("claim session authority", &error)))?;
    session_authority_from_item(output.item.as_ref(), session)
}

fn session_authority_from_item(
    item: Option<&aex_session_dynamodb::attr::Item>,
    expected_session: SessionId,
) -> Result<SessionAuthority, ClaimError> {
    // A durable wake or agent row may outlive the purged session that owned it.
    // That stale delivery is terminal work, not malformed storage to retry.
    let Some(item) = item else {
        return Err(ClaimError::Terminal);
    };
    decode_session_authority(item, expected_session)
}

fn decode_session_authority(
    item: &aex_session_dynamodb::attr::Item,
    expected_session: SessionId,
) -> Result<SessionAuthority, ClaimError> {
    let row = Row::bind(item, aex_session_dynamodb::codec::SESSION_HEAD).map_err(|error| {
        ClaimError::Store(StoreError::Undecodable {
            location: "session head".to_owned(),
            reason: error.to_string(),
        })
    })?;
    let workspace = row.id::<WorkspaceId>("workspaceId").map_err(|error| {
        ClaimError::Store(StoreError::Undecodable {
            location: "session head".to_owned(),
            reason: error.to_string(),
        })
    })?;
    let session = aex_session_dynamodb::authority_codec::decode_session(item, workspace).map_err(
        |error| {
            ClaimError::Store(StoreError::Undecodable {
                location: "session head".to_owned(),
                reason: error.to_string(),
            })
        },
    )?;
    if session.id != expected_session {
        return Err(ClaimError::Store(StoreError::Undecodable {
            location: "session head".to_owned(),
            reason: "the row session does not match the claimed agent".to_owned(),
        }));
    }
    if !session.deletion.state.admits_work() {
        return Err(ClaimError::Terminal);
    }
    Ok(SessionAuthority {
        workspace: session.workspace,
        organization: session.organization,
        deletion_epoch: session.deletion.epoch.0,
    })
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
                    "attribute_exists(pk) \
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
            let authority = load_session_authority(self, key).await?;
            Ok(Claim {
                key: *key,
                owner,
                fence: head.fence,
                expires_at: expires,
                authority,
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
            let session = translate::session(claim.key.session)
                .map_err(|error| ClaimError::Store(translate_error(&error)))?;
            let ttl_millis = i64::try_from(ttl.as_millis()).unwrap_or(i64::MAX);
            let expires = now.plus_millis(ttl_millis);
            let wire_now = translate::at(now, "now")
                .map_err(|error| ClaimError::Store(translate_error(&error)))?;
            let wire_expires = translate::at(expires, "expires")
                .map_err(|error| ClaimError::Store(translate_error(&error)))?;

            let material = format!(
                "{}:{}:{}:{}:{}:{}",
                claim.key.session.0.as_hyphenated(),
                claim.key.agent.0.as_hyphenated(),
                claim.owner.0.as_hyphenated(),
                claim.fence.0,
                claim.head.cancel_epoch.0,
                expires.millis(),
            );
            let digest = blake3::hash(material.as_bytes()).to_hex().to_string();
            let mut plan = aex_session_dynamodb::plan::TransactionPlan::new(format!(
                "brain-renew-{}",
                &digest[..24]
            ));
            plan.condition_check(
                aex_session_dynamodb::plan::Participant::SESSION_HEAD_GUARD,
                crate::plan::session_head_guard(
                    self.table(),
                    session,
                    claim.head.cancel_epoch,
                    &claim.authority,
                ),
            )
            .map_err(|error| renew_plan_error(&error))?;
            plan.update(
                aex_session_dynamodb::plan::Participant::AGENT_CONTROL,
                aws_sdk_dynamodb::types::Update::builder()
                    .table_name(self.table())
                    .set_key(Some(item_key(&control_key.pk, &control_key.sk)))
                    .condition_expression(
                        "fence = :fence AND claimOwner = :owner AND cancelEpoch = :cancelEpoch",
                    )
                    // Deliberately no `ADD fence`: a renewal proves nothing changed hands.
                    .update_expression("SET leaseExpiresAt = :expires, updatedAt = :now")
                    .expression_attribute_values(":fence", n(claim.fence.0))
                    .expression_attribute_values(
                        ":owner",
                        s(claim.owner.0.as_hyphenated().to_string()),
                    )
                    .expression_attribute_values(":cancelEpoch", n(claim.head.cancel_epoch.0))
                    .expression_attribute_values(":expires", stamp(wire_expires))
                    .expression_attribute_values(":now", stamp(wire_now)),
            )
            .map_err(|error| renew_plan_error(&error))?;
            debug_assert_eq!(plan.participants(), RENEW_ORDER);
            let participants = plan.participants().to_vec();
            plan.compile(self.client())
                .map_err(|error| renew_plan_error(&error))?
                .send()
                .await
                .map_err(|error| renewal_transaction_error(claim, &participants, &error))?;
            Ok(Claim {
                expires_at: expires,
                ..claim.clone()
            })
        })
    }

    fn release(
        &self,
        claim: Claim,
        _disposition: ReleaseDisposition,
    ) -> BoxFuture<'_, Result<(), StoreError>> {
        Box::pin(async move {
            let control_key = keys::control(&claim.key).map_err(|error| store_key_error(&error))?;
            // Every disposition ends this ownership scope. Removing `claimOwner` while
            // retaining a future expiry creates an ownerless interval in which the claim
            // condition still rejects every successor. The exact old fence/owner condition
            // makes immediate expiry safe: a delayed predecessor cannot clear a successor.
            match self
                .client()
                .update_item()
                .table_name(self.table())
                .set_key(Some(item_key(&control_key.pk, &control_key.sk)))
                .condition_expression("fence = :fence AND claimOwner = :owner")
                .update_expression("REMOVE claimOwner, leaseExpiresAt")
                .expression_attribute_values(":fence", n(claim.fence.0))
                .expression_attribute_values(":owner", s(claim.owner.0.as_hyphenated().to_string()))
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

fn renew_plan_error(error: &aex_session_dynamodb::error::StoreError) -> ClaimError {
    ClaimError::Store(StoreError::Transport {
        reason: error.to_string(),
        retryable: error.retryable(),
    })
}

fn renewal_transaction_error<R>(
    claim: &Claim,
    participants: &[aex_session_dynamodb::plan::Participant],
    error: &aws_sdk_dynamodb::error::SdkError<
        aws_sdk_dynamodb::operation::transact_write_items::TransactWriteItemsError,
        R,
    >,
) -> ClaimError {
    let mapped = match error.as_service_error() {
        Some(service) => aex_session_dynamodb::error::decode_cancellation(service, participants),
        None => aex_session_dynamodb::error::classify(
            error,
            aex_session_dynamodb::error::Idempotence::Write(
                aex_session_dynamodb::error::Resolution::TargetItem,
            ),
        ),
    };
    match mapped {
        aex_session_dynamodb::error::StoreError::PreconditionFailed {
            participant: aex_session_dynamodb::plan::Participant::SESSION_HEAD_GUARD,
            ..
        } => ClaimError::Terminal,
        aex_session_dynamodb::error::StoreError::PreconditionFailed {
            participant: aex_session_dynamodb::plan::Participant::AGENT_CONTROL,
            ..
        } => ClaimError::Fenced {
            current: Fence(claim.fence.0.saturating_add(1)),
        },
        other => renew_plan_error(&other),
    }
}

#[cfg(test)]
mod tests {
    use aex_brain_application::ports::{ClaimError, StoreError};
    use aex_session_domain::DeletionState;
    use aex_session_domain::testing::{id, session_fixture};
    use aex_wire::ids::{SessionId, WorkspaceId};

    use super::{decode_session_authority, session_authority_from_item};

    #[test]
    fn an_absent_parent_is_a_terminal_stale_claim() {
        assert_eq!(
            session_authority_from_item(None, id::<SessionId>(99)),
            Err(ClaimError::Terminal)
        );
    }

    #[test]
    fn canonical_live_session_supplies_the_claim_authority() {
        let session = session_fixture();
        let item = aex_session_dynamodb::authority_codec::encode_session(&session)
            .expect("canonical session row");

        let authority = decode_session_authority(&item, session.id).expect("live authority");

        assert_eq!(authority.workspace, session.workspace);
        assert_eq!(authority.organization, session.organization);
        assert_eq!(authority.deletion_epoch, session.deletion.epoch.0);
    }

    #[test]
    fn every_non_live_deletion_state_is_terminal_to_a_claim() {
        for state in [
            DeletionState::Trashed,
            DeletionState::Purging,
            DeletionState::Purged,
        ] {
            let mut session = session_fixture();
            session.deletion.state = state;
            let item = aex_session_dynamodb::authority_codec::encode_session(&session)
                .expect("canonical session row");

            assert_eq!(
                decode_session_authority(&item, session.id),
                Err(ClaimError::Terminal),
                "{state:?}"
            );
        }
    }

    #[test]
    fn a_claim_cannot_accept_an_authority_row_for_another_session() {
        let session = session_fixture();
        let item = aex_session_dynamodb::authority_codec::encode_session(&session)
            .expect("canonical session row");

        let error = decode_session_authority(&item, id::<SessionId>(99))
            .expect_err("session identity mismatch");

        assert!(
            matches!(error, ClaimError::Store(StoreError::Undecodable { .. })),
            "{error:?}"
        );
    }

    #[test]
    fn a_claim_refuses_workspace_drift_inside_the_canonical_row() {
        let session = session_fixture();
        let mut item = aex_session_dynamodb::authority_codec::encode_session(&session)
            .expect("canonical session row");
        item.insert(
            "workspaceId".to_owned(),
            aex_session_dynamodb::attr::s(id::<WorkspaceId>(98).to_string()),
        );

        let error = decode_session_authority(&item, session.id)
            .expect_err("the checked projection and document disagree");

        assert!(
            matches!(error, ClaimError::Store(StoreError::Undecodable { .. })),
            "{error:?}"
        );
    }
}
