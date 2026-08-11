//! Deriving a session's generation rows from the definition its head pinned.
//!
//! Session creation pins an immutable [`HandsGeneration`] on the session head
//! but makes no provider call. The first path that needs runtime activity
//! derives the generation row and `CURRENT` pointer from that pin. Concurrent
//! derivations of the same pin are benign; disagreement is never overwritten.

use aex_hands_protocol::rpc::Fence;
use aex_runtime_control::generation::{GenerationState, HandsGeneration, Revision};
use aex_session_dynamodb::error::StoreError;
use aex_wire::types::Timestamp;

use crate::codec::GenerationRow;
use crate::store::RuntimeActivityDynamoStore;

/// Why derivation did not establish the runtime rows.
#[derive(Debug, thiserror::Error)]
pub enum DeriveError {
    /// Existing runtime rows disagree with the immutable session-head pin.
    #[error("session {session} pinned generation {pinned} but its runtime rows name {stored}")]
    Conflict {
        /// The session.
        session: aex_wire::ids::SessionId,
        /// The immutable head pin.
        pinned: aex_wire::ids::GenerationId,
        /// The existing runtime row or pointer.
        stored: aex_wire::ids::GenerationId,
    },
    /// The runtime-activity store refused or could not answer.
    #[error(transparent)]
    Store(#[from] StoreError),
}

/// Produces the generation row a pinned definition has before its first launch.
#[must_use]
pub fn derived_generation(definition: &HandsGeneration, now: Timestamp) -> GenerationRow {
    GenerationRow {
        session: definition.session,
        workspace: definition.workspace,
        organization: definition.organization,
        generation: definition.generation,
        size: definition.size,
        definition: definition.clone(),
        state: GenerationState::Requested,
        fence: Fence(0),
        revision: Revision::ZERO,
        provider_vm_id: None,
        open_operations: 0,
        last_busy_at: now,
        idle_since: Some(now),
        keepalive_lease_until: None,
        provider_lifetime_expires_at: None,
        microvm: None,
        lifetime: None,
        accounted_from: now,
        open_intent: None,
        suspended_at: None,
        snapshot_ordinal: 0,
        snapshot_bytes: 0,
        suspend_lock_expires_at: None,
        keepalive_lease: None,
        transport_mode: None,
        next_evaluate_at: now,
        updated_at: now,
    }
}

/// Establishes both runtime rows from one immutable session-head pin.
///
/// # Errors
///
/// Returns [`DeriveError::Conflict`] when an existing row names a different
/// generation, or [`DeriveError::Store`] when the authority cannot decide.
pub async fn derive_generation_rows(
    store: &RuntimeActivityDynamoStore,
    definition: &HandsGeneration,
    now: Timestamp,
) -> Result<GenerationRow, DeriveError> {
    let row = derived_generation(definition, now);

    match store.create_generation(&row).await {
        Ok(()) => {}
        Err(StoreError::PreconditionFailed { .. }) => {
            let stored = store
                .load_generation(
                    definition.workspace,
                    definition.session,
                    definition.generation,
                )
                .await?
                .ok_or_else(|| {
                    DeriveError::Store(StoreError::Invalid {
                        detail: "generation create lost an absence condition but read back absent"
                            .to_owned(),
                    })
                })?;
            if stored.definition != row.definition {
                return Err(DeriveError::Conflict {
                    session: definition.session,
                    pinned: definition.generation,
                    stored: stored.generation,
                });
            }
        }
        Err(error) => return Err(error.into()),
    }

    match store
        .point_current(
            definition.session,
            definition.generation,
            Fence(0),
            None,
            now,
        )
        .await
    {
        Ok(()) => Ok(row),
        Err(StoreError::PreconditionFailed { .. }) => {
            let current = store
                .load_current(definition.session)
                .await?
                .ok_or_else(|| {
                    DeriveError::Store(StoreError::Invalid {
                        detail: "current pointer lost an absence condition but read back absent"
                            .to_owned(),
                    })
                })?;
            if current.generation == definition.generation {
                Ok(row)
            } else {
                Err(DeriveError::Conflict {
                    session: definition.session,
                    pinned: definition.generation,
                    stored: current.generation,
                })
            }
        }
        Err(error) => Err(error.into()),
    }
}

#[cfg(test)]
mod tests {
    use aex_runtime_control::generation::{
        GenerationState, HandsGeneration, ImageIdentifier, ImagePin, ImageVersion, LimitsRevision,
        NetworkPolicy, guest_root,
    };
    use aex_wire::ids::{
        GenerationId, OrganizationId, PrefixedId as _, SessionId, Uuid7, WorkspaceId,
    };
    use aex_wire::types::{ComputeSize, Timestamp};

    use super::derived_generation;

    fn definition() -> HandsGeneration {
        HandsGeneration {
            generation: GenerationId::from_uuid7(Uuid7::compose(1, [7; 10])),
            session: SessionId::from_uuid7(Uuid7::compose(1, [1; 10])),
            workspace: WorkspaceId::from_uuid7(Uuid7::compose(1, [2; 10])),
            organization: OrganizationId::from_uuid7(Uuid7::compose(1, [3; 10])),
            size: ComputeSize::Gb1,
            image: ImagePin {
                identifier: ImageIdentifier("aex-hands-1gb".to_owned()),
                version: ImageVersion("1".to_owned()),
                artifact_digest: aex_wire::ids::ContentHash::from_bytes([7; 32]),
                capabilities: Vec::new(),
            },
            network: NetworkPolicy::None,
            protocol_version: aex_internal_contracts::SchemaVersion::V1,
            limits_revision: LimitsRevision(1),
            root: guest_root(),
        }
    }

    fn moment(millis: i64) -> Timestamp {
        Timestamp::from_unix_millis(millis).expect("in range")
    }

    #[test]
    fn derived_rows_claim_no_provider_launch() {
        let pinned = definition();
        let row = derived_generation(&pinned, moment(0));
        assert_eq!(row.definition, pinned);
        assert_eq!(row.state, GenerationState::Requested);
        assert!(row.microvm.is_none());
        assert!(row.provider_vm_id.is_none());
        assert!(row.open_intent.is_none());
        assert_eq!(row.open_operations, 0);
    }

    #[test]
    fn lifecycle_instants_do_not_change_the_derived_identity() {
        let pinned = definition();
        let early = derived_generation(&pinned, moment(0));
        let late = derived_generation(&pinned, moment(60_000));
        assert_eq!(early.session, late.session);
        assert_eq!(early.generation, late.generation);
        assert_eq!(early.definition, late.definition);
        assert_eq!(early.state, late.state);
        assert_eq!(early.fence, late.fence);
        assert_eq!(early.revision, late.revision);
    }
}
