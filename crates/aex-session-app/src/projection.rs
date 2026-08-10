//! The one rendering of a session onto the public contract.
//!
//! There is exactly one of these on purpose. A create stores the canonical 201
//! bytes on its idempotency receipt so a replay is byte-identical rather than
//! re-projected (A D-6), and that argument only holds if the bytes the winner
//! sent and the bytes a later read renders come out of the same function. A
//! second projection — one for create, one for `session_get` — would eventually
//! disagree, and the disagreement would surface as a replay that differs from
//! the original.
//!
//! So: `session_get` and `sessions_list` consume this. They must not grow their
//! own.

use aex_session_domain::{Session, SessionStatus};
use aex_wire::canonical::{CanonicalError, to_jcs_bytes};
use aex_wire::models;

/// The public status of a session head.
///
/// The domain has five statuses and the wire has four: both deletion states
/// project onto `deleting`, because a caller is told a deletion is running and
/// not which half of it.
#[must_use]
pub const fn public_status(status: SessionStatus) -> models::SessionStatus {
    match status {
        SessionStatus::Idle => models::SessionStatus::Idle,
        SessionStatus::Running => models::SessionStatus::Running,
        SessionStatus::AwaitingApproval => models::SessionStatus::AwaitingApproval,
        SessionStatus::Trashed | SessionStatus::Purging => models::SessionStatus::Deleting,
    }
}

/// Renders one session head onto the published resource.
///
/// # Errors
///
/// Returns [`CanonicalError`] when the head's sealed resolved-configuration
/// document is not the generated `ResolvedConfig` shape. That is a corrupt
/// head, never a caller input: construction of `ResolvedConfigAuthority`
/// already refused any document that is not one.
pub fn public_session(session: &Session) -> Result<models::Session, CanonicalError> {
    let resolved: models::ResolvedConfig =
        serde_json::from_value(session.resolved.document().to_value()).map_err(|error| {
            CanonicalError::Malformed {
                reason: error.to_string(),
            }
        })?;
    Ok(models::Session {
        id: session.id,
        workspace_id: session.workspace,
        status: public_status(session.status),
        revision: session.revision.0,
        continuity: models::WorkspaceContinuity {
            // A session that has never persisted has no durable root to name,
            // and rendering the all-zero root as a hash would publish a digest
            // that addresses nothing.
            root_hash: (session.persist_revision.0 > 0)
                .then(|| aex_session_domain::public_root_hash(&session.persisted_root)),
            persist_revision: session.persist_revision.0,
            last_persisted_at: session.last_persisted_at,
            // The generation that is *live*, which is `None` until a launch.
            // Never the pinned definition: publishing that would tell a caller
            // a workspace is running when H-LAZY guarantees nothing has
            // started.
            live_generation_id: session.generation,
        },
        lineage: models::SessionLineage {
            origin_session_id: session.lineage.origin.as_ref().map(|origin| origin.session),
            cloned_at_persist_revision: session
                .lineage
                .origin
                .as_ref()
                .map(|origin| origin.source_persist_revision.0),
            clone_operation_id: session
                .lineage
                .origin
                .as_ref()
                .map(|origin| origin.operation),
        },
        resolved_config: resolved,
        metadata: session
            .metadata
            .as_ref()
            .map(|metadata| serde_json::from_value(metadata.document().to_value()))
            .transpose()
            .map_err(|error| CanonicalError::Malformed {
                reason: error.to_string(),
            })?,
        created_at: session.created_at,
        updated_at: session.updated_at,
    })
}

/// The exact canonical bytes a session resource is served as.
///
/// # Errors
///
/// Returns [`CanonicalError`] as [`public_session`] does, or when the rendered
/// resource cannot be canonicalized.
pub fn canonical_session_bytes(session: &Session) -> Result<Vec<u8>, CanonicalError> {
    to_jcs_bytes(&public_session(session)?)
}
