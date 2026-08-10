//! The session head.
//!
//! Two guards live here. `WorkAdmission` says whether new work may start at all
//! and moves alongside a session-wide cancellation; the **mutation guard** is a
//! single-holder exclusion that a whole-session command — persist, clone,
//! discard, rebind — takes so two of them cannot interleave.

use aex_content_domain::ContentRoot;
use aex_operation_domain::{DeletionGuard, DeletionState, OperationKind};
use aex_secret_domain::CustodyRevision;
use aex_wire::CanonicalJson;
use aex_wire::ids::{
    AgentId, ContentHash, GenerationId, OperationId, OrganizationId, RunId, SessionId, WorkspaceId,
};
use aex_wire::provider::ProviderId;
use aex_wire::types::Timestamp;
use sha2::{Digest as _, Sha256};

use crate::ids::{CancellationEpoch, PersistRevision, SessionRevision};
use crate::lineage::Lineage;

/// Where the session is, as the public wire sees it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum SessionStatus {
    /// At rest.
    Idle,
    /// A run owns the session.
    Running,
    /// A run is blocked on an approval.
    AwaitingApproval,
    /// In the recovery window.
    Trashed,
    /// Past the destructive fence.
    Purging,
}

impl SessionStatus {
    /// Every status, in lifecycle order.
    pub const ALL: [Self; 5] = [
        Self::Idle,
        Self::Running,
        Self::AwaitingApproval,
        Self::Trashed,
        Self::Purging,
    ];

    /// Whether a run currently owns the session.
    #[must_use]
    pub const fn has_active_run(self) -> bool {
        matches!(self, Self::Running | Self::AwaitingApproval)
    }
}

/// Whether new work may be admitted, and why not.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum WorkAdmission {
    /// New work is admitted.
    Open,
    /// The account is paused.
    Paused,
    /// The session is being trashed.
    Trashing,
    /// The session is being purged.
    Purging,
    /// The workspace generation the session depended on is gone.
    ContinuityLost,
}

impl WorkAdmission {
    /// Every admission state, in canonical order.
    pub const ALL: [Self; 5] = [
        Self::Open,
        Self::Paused,
        Self::Trashing,
        Self::Purging,
        Self::ContinuityLost,
    ];

    /// Whether new work may be admitted.
    #[must_use]
    pub const fn is_open(self) -> bool {
        matches!(self, Self::Open)
    }
}

/// The single-holder exclusion a whole-session command takes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MutationGuard {
    /// The operation holding it.
    pub holder: OperationId,
    /// What that operation does.
    pub kind: OperationKind,
    /// When it was taken.
    pub acquired_at: Timestamp,
}

/// The digest of the exact configuration a session resolved at creation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ResolvedConfigDigest(pub [u8; 32]);

/// The content-addressed authority for the exact configuration a session uses.
///
/// The canonical document is the whole generated `ResolvedConfig`, not a
/// provider/model shortcut. Provider and model are carried beside it only as
/// checked projections for the workspace session index; construction refuses
/// a projection that disagrees with the document.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedConfigAuthority {
    /// SHA-256 of the canonical document bytes.
    digest: ResolvedConfigDigest,
    /// The whole resolved configuration, in canonical JSON form.
    document: CanonicalJson,
    /// The checked provider projection.
    provider: ProviderId,
    /// The checked provider-native model projection.
    model: String,
}

impl ResolvedConfigAuthority {
    /// Builds a content-addressed configuration authority.
    ///
    /// # Errors
    ///
    /// Returns [`ResolvedConfigError`] unless the canonical document is an
    /// object whose `provider` and `model` exactly match the projections.
    pub fn new(
        document: CanonicalJson,
        provider: ProviderId,
        model: String,
    ) -> Result<Self, ResolvedConfigError> {
        let value = document.to_value();
        let resolved: aex_wire::models::ResolvedConfig =
            serde_json::from_value(value).map_err(|_| ResolvedConfigError::MalformedDocument)?;
        if resolved.provider != provider {
            return Err(ResolvedConfigError::ProviderProjection);
        }
        if resolved.model != model {
            return Err(ResolvedConfigError::ModelProjection);
        }
        let mut hasher = Sha256::new();
        hasher.update(document.as_bytes());
        Ok(Self {
            digest: ResolvedConfigDigest(hasher.finalize().into()),
            document,
            provider,
            model,
        })
    }

    /// SHA-256 of the canonical document bytes.
    #[must_use]
    pub const fn digest(&self) -> ResolvedConfigDigest {
        self.digest
    }

    /// The whole resolved configuration document.
    #[must_use]
    pub const fn document(&self) -> &CanonicalJson {
        &self.document
    }

    /// The checked provider projection.
    #[must_use]
    pub const fn provider(&self) -> ProviderId {
        self.provider
    }

    /// The checked provider-native model projection.
    #[must_use]
    pub fn model(&self) -> &str {
        &self.model
    }
}

/// Why a resolved configuration authority was rejected.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum ResolvedConfigError {
    /// The canonical document is not the generated strict-v1 resolved schema.
    #[error("resolved configuration document is not a strict-v1 ResolvedConfig")]
    MalformedDocument,
    /// The provider projection does not match the canonical document.
    #[error("resolved provider projection does not match its canonical document")]
    ProviderProjection,
    /// The model projection does not match the canonical document.
    #[error("resolved model projection does not match its canonical document")]
    ModelProjection,
}

/// Caller-defined session labels in the exact closed wire shape.
///
/// Keeping canonical JSON here preserves equality without weakening the public
/// `MetadataValue` vocabulary to an arbitrary nested document. Construction
/// proves the document is an object of JSON scalars, so projection is total.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionMetadata(CanonicalJson);

impl SessionMetadata {
    /// Validates one canonical metadata object.
    ///
    /// # Errors
    ///
    /// Returns [`SessionMetadataError`] for a non-object, nested value, or
    /// number that cannot enter the wire's finite `f64` representation.
    pub fn new(document: CanonicalJson) -> Result<Self, SessionMetadataError> {
        let value = document.to_value();
        let Some(object) = value.as_object() else {
            return Err(SessionMetadataError::NotObject);
        };
        for value in object.values() {
            if !(value.is_null()
                || value.is_boolean()
                || value.is_string()
                || value.as_f64().is_some())
            {
                return Err(SessionMetadataError::NonScalar);
            }
        }
        Ok(Self(document))
    }

    /// The canonical metadata document.
    #[must_use]
    pub const fn document(&self) -> &CanonicalJson {
        &self.0
    }
}

/// Why caller metadata was rejected.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum SessionMetadataError {
    /// Metadata must be a JSON object.
    #[error("session metadata must be a JSON object")]
    NotObject,
    /// Metadata values must be wire scalars.
    #[error("session metadata values must be finite JSON scalars")]
    NonScalar,
}

/// Computes the public SHA-256 identity of a canonical BLAKE3 tree root.
///
/// The storage root remains the lossless BLAKE3 tuple. The public contract uses
/// `ContentHash`, so its value is SHA-256 over a domain tag plus that complete
/// tuple; it is never a mislabeled copy of the BLAKE3 digest.
#[must_use]
pub fn public_root_hash(root: &ContentRoot) -> ContentHash {
    let mut hasher = Sha256::new();
    hasher.update(b"aex.session.public-root.v1\0");
    hasher.update(root.digest);
    hasher.update(root.entries.to_be_bytes());
    hasher.update(root.logical_bytes.to_be_bytes());
    ContentHash::from_bytes(hasher.finalize().into())
}

/// One session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Session {
    /// Its identity.
    pub id: SessionId,
    /// The owning workspace.
    pub workspace: WorkspaceId,
    /// The owning organization.
    pub organization: OrganizationId,
    /// Where it is.
    pub status: SessionStatus,
    /// The head's concurrency token.
    pub revision: SessionRevision,
    /// The run that owns it, when one does.
    pub active_run: Option<RunId>,
    /// Whether new work may be admitted.
    pub work_admission: WorkAdmission,
    /// How many session-wide cancellations have happened.
    pub cancellation: CancellationEpoch,
    /// How far into deletion it is.
    pub deletion: DeletionGuard,
    /// The whole-session command holding the exclusion, when one does.
    pub mutation_guard: Option<MutationGuard>,
    /// The root agent.
    pub root_agent: AgentId,
    /// The live workspace generation, when one is running.
    pub generation: Option<GenerationId>,
    /// The root the session was created with.
    pub initial_root: ContentRoot,
    /// The durable root it last persisted.
    pub persisted_root: ContentRoot,
    /// How many times the durable root has advanced.
    pub persist_revision: PersistRevision,
    /// When it last persisted.
    pub last_persisted_at: Option<Timestamp>,
    /// The custody revision its credentials are at.
    pub custody_revision: CustodyRevision,
    /// Where it came from.
    pub lineage: Lineage,
    /// The exact configuration it resolved.
    pub resolved: ResolvedConfigAuthority,
    /// Caller-defined metadata as one canonical JSON object, when present.
    pub metadata: Option<SessionMetadata>,
    /// When it was created.
    pub created_at: Timestamp,
    /// When the head last changed.
    pub updated_at: Timestamp,
}

impl Session {
    /// Whether the session is genuinely at rest for a whole-session command.
    ///
    /// Three things must hold: no run owns it, no whole-session command holds
    /// the exclusion, and it is still `Live`.
    #[must_use]
    pub const fn is_command_idle(&self) -> bool {
        self.active_run.is_none()
            && self.mutation_guard.is_none()
            && matches!(self.deletion.state, DeletionState::Live)
    }
}

/// Why a session-head change was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum SessionError {
    /// Another whole-session command holds the exclusion.
    #[error("session is held by operation {holder}")]
    MutationGuardHeld {
        /// The holder.
        holder: OperationId,
    },
    /// The caller is not the holder.
    #[error("operation {presented} does not hold the session guard")]
    NotGuardHolder {
        /// Who holds it, when anyone does.
        holder: Option<OperationId>,
        /// Who asked.
        presented: OperationId,
    },
    /// The session is not admitting work.
    #[error("session admission is {0:?}")]
    AdmissionClosed(WorkAdmission),
    /// A run owns the session.
    #[error("session is not idle: run {run} owns it")]
    NotIdle {
        /// The owning run.
        run: RunId,
    },
    /// The session is being or has been deleted.
    #[error("session deletion state is {0:?}")]
    Deleted(DeletionState),
}

/// Takes the whole-session exclusion.
///
/// # Errors
///
/// Returns [`SessionError`] when another operation holds it, when the session is
/// not `Live`, or when a run owns the session. Re-acquiring by the same holder
/// is idempotent and returns the existing guard unchanged.
pub fn acquire_mutation_guard(
    session: &Session,
    holder: OperationId,
    kind: OperationKind,
    now: Timestamp,
) -> Result<MutationGuard, SessionError> {
    if session.deletion.state != DeletionState::Live {
        return Err(SessionError::Deleted(session.deletion.state));
    }
    if let Some(existing) = session.mutation_guard {
        if existing.holder == holder {
            return Ok(existing);
        }
        return Err(SessionError::MutationGuardHeld {
            holder: existing.holder,
        });
    }
    if let Some(run) = session.active_run {
        return Err(SessionError::NotIdle { run });
    }
    Ok(MutationGuard {
        holder,
        kind,
        acquired_at: now,
    })
}

/// Releases the whole-session exclusion.
///
/// # Errors
///
/// Returns [`SessionError::NotGuardHolder`] when the caller does not hold it.
/// Releasing a guard that is already free is **not** silently accepted: a
/// double release means two commands believe they owned the session.
pub fn release_mutation_guard(session: &Session, holder: OperationId) -> Result<(), SessionError> {
    match session.mutation_guard {
        Some(existing) if existing.holder == holder => Ok(()),
        Some(existing) => Err(SessionError::NotGuardHolder {
            holder: Some(existing.holder),
            presented: holder,
        }),
        None => Err(SessionError::NotGuardHolder {
            holder: None,
            presented: holder,
        }),
    }
}

#[cfg(test)]
mod tests {
    use aex_operation_domain::OperationKind;
    use aex_wire::CanonicalJson;
    use aex_wire::ids::{OperationId, PrefixedId as _, RunId, Uuid7};
    use aex_wire::provider::ProviderId;

    use super::{
        ResolvedConfigAuthority, ResolvedConfigError, SessionError, SessionMetadata,
        SessionMetadataError, acquire_mutation_guard, release_mutation_guard,
    };
    use crate::testing::session_fixture;

    fn operation(tag: u8) -> OperationId {
        OperationId::from_uuid7(Uuid7::compose(1, [tag; 10]))
    }

    fn moment(millis: i64) -> aex_wire::types::Timestamp {
        aex_wire::types::Timestamp::from_unix_millis(millis).expect("in range")
    }

    #[test]
    fn resolved_config_rejects_a_drifting_index_projection() {
        let fixture = crate::testing::session_fixture()
            .resolved
            .document()
            .clone();
        assert_eq!(
            ResolvedConfigAuthority::new(
                fixture.clone(),
                ProviderId::Anthropic,
                "gpt-test".to_owned(),
            ),
            Err(ResolvedConfigError::ProviderProjection)
        );
        assert_eq!(
            ResolvedConfigAuthority::new(
                fixture,
                ProviderId::Openai,
                "a-different-model".to_owned(),
            ),
            Err(ResolvedConfigError::ModelProjection)
        );
    }

    #[test]
    fn metadata_refuses_a_nested_document() {
        let nested = CanonicalJson::parse(r#"{"nested":{"value":1}}"#).expect("canonical");
        assert_eq!(
            SessionMetadata::new(nested),
            Err(SessionMetadataError::NonScalar)
        );
    }

    #[test]
    fn the_guard_admits_one_holder_and_is_idempotent_for_it() {
        let mut session = session_fixture();
        let guard = acquire_mutation_guard(
            &session,
            operation(1),
            OperationKind::SessionPersist,
            moment(1),
        )
        .expect("acquires");
        session.mutation_guard = Some(guard);

        assert_eq!(
            acquire_mutation_guard(
                &session,
                operation(1),
                OperationKind::SessionPersist,
                moment(2)
            ),
            Ok(guard)
        );
        assert_eq!(
            acquire_mutation_guard(
                &session,
                operation(2),
                OperationKind::SessionTrash,
                moment(2)
            ),
            Err(SessionError::MutationGuardHeld {
                holder: operation(1)
            })
        );
    }

    #[test]
    fn releasing_by_a_non_holder_is_rejected() {
        let mut session = session_fixture();
        assert_eq!(
            release_mutation_guard(&session, operation(1)),
            Err(SessionError::NotGuardHolder {
                holder: None,
                presented: operation(1)
            })
        );
        session.mutation_guard = Some(
            acquire_mutation_guard(
                &session,
                operation(1),
                OperationKind::SessionPersist,
                moment(1),
            )
            .expect("acquires"),
        );
        assert_eq!(release_mutation_guard(&session, operation(1)), Ok(()));
        assert_eq!(
            release_mutation_guard(&session, operation(2)),
            Err(SessionError::NotGuardHolder {
                holder: Some(operation(1)),
                presented: operation(2)
            })
        );
    }

    #[test]
    fn a_running_session_refuses_the_guard() {
        let mut session = session_fixture();
        let run = RunId::from_uuid7(Uuid7::compose(1, [9; 10]));
        session.active_run = Some(run);
        assert_eq!(
            acquire_mutation_guard(
                &session,
                operation(1),
                OperationKind::SessionPersist,
                moment(1)
            ),
            Err(SessionError::NotIdle { run })
        );
    }
}
