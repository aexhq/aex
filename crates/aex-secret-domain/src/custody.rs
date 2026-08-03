//! Session credential custody.
//!
//! A session admits a snapshot of the workspace secrets it is allowed to use:
//! each [`CustodyEntry`] records the source generation **and** the revocation
//! epoch in force at admission. Use is then a pure comparison, so a revocation
//! anywhere in the lineage denies every holder without rewriting a single
//! custody row.
//!
//! `rebind` is explicit, idle-only and atomic: it rewraps from the **workspace**
//! ciphertext rather than the old session ciphertext, mints a new owner key edge
//! and increments the custody revision by exactly one. The prior edge is emitted
//! for destruction strictly after commit and before any fact publication, so
//! every authorization at the old revision dies immediately.

use aex_wire::ids::{SessionId, Uuid7, WorkspaceId};
use aex_wire::types::Timestamp;

use crate::revocation::RevocationEpoch;
use crate::secret::{
    CiphertextRef, SecretName, SecretRevision, SecretState, SourceGeneration, WorkspaceSecret,
};

/// The monotone revision of one session's custody.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct CustodyRevision(pub u64);

impl CustodyRevision {
    /// The revision a session with no custody row reports.
    pub const NONE: Self = Self(0);

    /// The revision the first admission writes.
    pub const FIRST: Self = Self(1);

    /// The next revision.
    ///
    /// # Panics
    ///
    /// Panics on `u64` overflow.
    #[must_use]
    pub const fn next(self) -> Self {
        match self.0.checked_add(1) {
            Some(value) => Self(value),
            None => panic!("custody revision overflowed"),
        }
    }
}

/// One owner key edge. Destroying it invalidates every authorization derived
/// from it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct OwnerKeyEdgeId(pub Uuid7);

/// Where a custody row is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum CustodyState {
    /// Usable.
    Active,
    /// Tombstoned.
    Deleted,
}

/// One admitted secret inside a session's custody.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CustodyEntry {
    /// Which name.
    pub name: SecretName,
    /// The source generation admitted.
    pub source_generation: SourceGeneration,
    /// The source metadata revision admitted. Managed-call authorization uses
    /// this immutable fence against `revokedThroughRevision`.
    pub source_revision: SecretRevision,
    /// The revocation epoch in force at admission.
    pub epoch_at_admission: RevocationEpoch,
    /// The session-scoped ciphertext.
    pub ciphertext: CiphertextRef,
}

/// One session's credential custody.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionCustody {
    /// The owning session.
    pub session: SessionId,
    /// The owning workspace.
    pub workspace: WorkspaceId,
    /// The monotone revision.
    pub revision: CustodyRevision,
    /// The key edge every authorization at this revision derives from.
    pub owner_key_edge: OwnerKeyEdgeId,
    /// Where the row is.
    pub state: CustodyState,
    /// The admitted entries, in name order.
    pub entries: Vec<CustodyEntry>,
    /// When the row last changed.
    pub updated_at: Timestamp,
}

/// What custody looks like from outside the domain.
///
/// Names and a revision, nothing else: source generations, ciphertext and key
/// edges never leave.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CustodyDescription {
    /// The admitted names, in order.
    pub names: Vec<SecretName>,
    /// The current revision.
    pub revision: CustodyRevision,
}

impl SessionCustody {
    /// The outward projection.
    #[must_use]
    pub fn describe(&self) -> CustodyDescription {
        CustodyDescription {
            names: self
                .entries
                .iter()
                .map(|entry| entry.name.clone())
                .collect(),
            revision: self.revision,
        }
    }

    /// The entry for a name, when one is bound.
    #[must_use]
    pub fn entry(&self, name: &SecretName) -> Option<&CustodyEntry> {
        self.entries.iter().find(|entry| entry.name == *name)
    }
}

/// Why a generation is not genuinely idle.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum TrueIdleViolation {
    /// Brain has admitted work that has not settled.
    WorkAdmitted,
    /// Work is queued.
    WorkQueued,
    /// A connection is open.
    ConnectionOpen,
    /// A keepalive lease holds the generation warm.
    KeepaliveHeld,
}

/// The idle evidence a rebind consumes.
///
/// `aex-runtime-control` owns the predicate and the exact idle window; this
/// crate consumes only its verdict, so the two cannot disagree about what
/// "idle" means by each computing it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TrueIdle {
    /// When the evidence was taken.
    pub observed_at: Timestamp,
    /// Why it is not idle, when it is not.
    pub violation: Option<TrueIdleViolation>,
}

impl TrueIdle {
    /// Idle evidence.
    #[must_use]
    pub const fn idle(observed_at: Timestamp) -> Self {
        Self {
            observed_at,
            violation: None,
        }
    }

    /// Whether the session is genuinely idle.
    #[must_use]
    pub const fn is_idle(&self) -> bool {
        self.violation.is_none()
    }
}

/// Which credentials a clone inherits.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum CloneCredentials {
    /// Preserve the source generations and epochs under a distinct key edge and
    /// distinct ciphertext.
    Copy,
    /// Write no custody row at all.
    None,
}

/// A custody row after a change.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CustodyCommit {
    /// The row after the change.
    pub custody: SessionCustody,
    /// Key edges to destroy strictly after commit and before fact publication.
    pub destroy_key_edges: Vec<OwnerKeyEdgeId>,
}

/// A rebind.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RebindCommit {
    /// The row after the rebind.
    pub custody: SessionCustody,
    /// The prior edge, emitted for destruction after commit.
    pub destroy_key_edges: Vec<OwnerKeyEdgeId>,
}

/// A clone's custody.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CloneCustodyCommit {
    /// The child's row, when one was written.
    pub custody: Option<SessionCustody>,
    /// The revision the child reports.
    pub revision: CustodyRevision,
    /// Key edges to destroy after commit. Empty for a successful clone; present
    /// only when a partially built edge must be released.
    pub destroy_key_edges: Vec<OwnerKeyEdgeId>,
}

/// Why a custody change was refused.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CustodyRejection {
    /// The session was not genuinely idle.
    #[error("session is not true idle: {0:?}")]
    NotTrueIdle(TrueIdleViolation),
    /// The session already has custody.
    #[error("session already has custody")]
    AlreadyAdmitted,
    /// The session is gone.
    #[error("session is deleted")]
    SessionDeleted,
    /// A selected secret is not `Ready`.
    #[error("secret `{0}` is not ready")]
    SecretNotReady(SecretName),
    /// A selected secret is revoked.
    #[error("secret `{0}` is revoked")]
    SecretRevoked(SecretName),
    /// The selection named the same secret twice.
    #[error("secret `{0}` is selected twice")]
    DuplicateName(SecretName),
}

/// Why a managed use was denied.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum UseDenied {
    /// The name was revoked after this session admitted it.
    #[error("secret revoked after admission (admitted {admitted:?}, current {current:?})")]
    RevokedAfterAdmission {
        /// The epoch at admission.
        admitted: RevocationEpoch,
        /// The epoch now.
        current: RevocationEpoch,
    },
    /// The workspace record is a tombstone.
    #[error("secret is deleted")]
    SecretDeleted,
    /// The session never admitted the name.
    #[error("secret `{0}` is not bound to this session")]
    SecretNotBound(SecretName),
    /// The caller presented a different custody revision.
    #[error("custody revision mismatch (current {current:?}, presented {presented:?})")]
    CustodyRevisionMismatch {
        /// The row's revision.
        current: CustodyRevision,
        /// What the caller presented.
        presented: CustodyRevision,
    },
    /// The custody row is a tombstone.
    #[error("custody is deleted")]
    CustodyDeleted,
}

fn check_selection(selection: &[WorkspaceSecret]) -> Result<(), CustodyRejection> {
    let mut seen: Vec<&SecretName> = Vec::with_capacity(selection.len());
    for secret in selection {
        if seen.contains(&&secret.name) {
            return Err(CustodyRejection::DuplicateName(secret.name.clone()));
        }
        seen.push(&secret.name);
        match secret.state {
            SecretState::Ready => {}
            SecretState::Revoked => {
                return Err(CustodyRejection::SecretRevoked(secret.name.clone()));
            }
            SecretState::Deleted => {
                return Err(CustodyRejection::SecretNotReady(secret.name.clone()));
            }
        }
    }
    Ok(())
}

fn entries_from(selection: &[WorkspaceSecret]) -> Result<Vec<CustodyEntry>, CustodyRejection> {
    let mut entries: Vec<CustodyEntry> = selection
        .iter()
        .map(|secret| {
            Ok(CustodyEntry {
                name: secret.name.clone(),
                source_generation: secret.generation,
                source_revision: secret.revision,
                epoch_at_admission: secret.revocation_epoch,
                ciphertext: secret
                    .ciphertext
                    .clone()
                    .ok_or_else(|| CustodyRejection::SecretNotReady(secret.name.clone()))?,
            })
        })
        .collect::<Result<Vec<_>, CustodyRejection>>()?;
    entries.sort_by(|left, right| {
        left.name
            .as_str()
            .as_bytes()
            .cmp(right.name.as_str().as_bytes())
    });
    Ok(entries)
}

/// Admits a session's credentials for the first time.
///
/// # Errors
///
/// Returns [`CustodyRejection`] when the session already has custody or when the
/// selection contains a duplicate, revoked or unready name.
pub fn admit_custody(
    session: SessionId,
    workspace: WorkspaceId,
    existing: Option<&SessionCustody>,
    selection: &[WorkspaceSecret],
    edge: OwnerKeyEdgeId,
    now: Timestamp,
) -> Result<SessionCustody, CustodyRejection> {
    if let Some(existing) = existing {
        if existing.state == CustodyState::Deleted {
            return Err(CustodyRejection::SessionDeleted);
        }
        return Err(CustodyRejection::AlreadyAdmitted);
    }
    check_selection(selection)?;
    Ok(SessionCustody {
        session,
        workspace,
        revision: CustodyRevision::FIRST,
        owner_key_edge: edge,
        state: CustodyState::Active,
        entries: entries_from(selection)?,
        updated_at: now,
    })
}

/// Rebinds a session's credentials from the current workspace ciphertext.
///
/// # Errors
///
/// Returns [`CustodyRejection`] when the session is not genuinely idle, when the
/// row is a tombstone, or when the selection contains a duplicate, revoked or
/// unready name.
pub fn rebind(
    current: &SessionCustody,
    selection: &[WorkspaceSecret],
    idle: &TrueIdle,
    edge: OwnerKeyEdgeId,
    now: Timestamp,
) -> Result<RebindCommit, CustodyRejection> {
    if let Some(violation) = idle.violation {
        return Err(CustodyRejection::NotTrueIdle(violation));
    }
    if current.state == CustodyState::Deleted {
        return Err(CustodyRejection::SessionDeleted);
    }
    check_selection(selection)?;
    Ok(RebindCommit {
        custody: SessionCustody {
            revision: current.revision.next(),
            owner_key_edge: edge,
            entries: entries_from(selection)?,
            updated_at: now,
            ..current.clone()
        },
        destroy_key_edges: vec![current.owner_key_edge],
    })
}

/// Builds a clone's custody.
///
/// `Copy` preserves the source generations and epochs under a **distinct** edge
/// and distinct ciphertext, so revoking a name still denies both sessions;
/// `None` writes no row and reports revision 0.
///
/// # Errors
///
/// Returns [`CustodyRejection::SessionDeleted`] when the source row is a
/// tombstone.
pub fn clone_custody(
    source: Option<&SessionCustody>,
    target: SessionId,
    mode: CloneCredentials,
    edge: Option<OwnerKeyEdgeId>,
    now: Timestamp,
) -> Result<CloneCustodyCommit, CustodyRejection> {
    match (mode, source, edge) {
        // Nothing to copy: either the caller asked for no credentials or the
        // source has none. Any edge that was minted in anticipation is released.
        (CloneCredentials::None, _, unused) | (CloneCredentials::Copy, None, unused) => {
            Ok(CloneCustodyCommit {
                custody: None,
                revision: CustodyRevision::NONE,
                destroy_key_edges: unused.into_iter().collect(),
            })
        }
        (CloneCredentials::Copy, Some(source), Some(edge)) => {
            if source.state == CustodyState::Deleted {
                return Err(CustodyRejection::SessionDeleted);
            }
            Ok(CloneCustodyCommit {
                custody: Some(SessionCustody {
                    session: target,
                    workspace: source.workspace,
                    revision: CustodyRevision::FIRST,
                    owner_key_edge: edge,
                    state: CustodyState::Active,
                    entries: source.entries.clone(),
                    updated_at: now,
                }),
                revision: CustodyRevision::FIRST,
                destroy_key_edges: Vec::new(),
            })
        }
        (CloneCredentials::Copy, Some(_), None) => Err(CustodyRejection::SessionDeleted),
    }
}

/// Tombstones a session's custody.
#[must_use]
pub fn delete_custody(current: &SessionCustody, now: Timestamp) -> CustodyCommit {
    CustodyCommit {
        custody: SessionCustody {
            state: CustodyState::Deleted,
            revision: current.revision.next(),
            entries: Vec::new(),
            updated_at: now,
            ..current.clone()
        },
        destroy_key_edges: vec![current.owner_key_edge],
    }
}

/// Whether a managed use of one admitted name is allowed.
///
/// Every clause must hold: the custody row is active, the caller presented the
/// exact revision, the name is bound, the workspace record still exists, and the
/// revocation epoch has not moved since admission.
///
/// # Errors
///
/// Returns the [`UseDenied`] naming the first failing clause.
pub fn managed_use_allowed(
    entry: &CustodyEntry,
    current: &WorkspaceSecret,
    presented: CustodyRevision,
    custody: &SessionCustody,
) -> Result<(), UseDenied> {
    if custody.state == CustodyState::Deleted {
        return Err(UseDenied::CustodyDeleted);
    }
    if custody.revision != presented {
        return Err(UseDenied::CustodyRevisionMismatch {
            current: custody.revision,
            presented,
        });
    }
    if custody.entry(&entry.name).is_none() {
        return Err(UseDenied::SecretNotBound(entry.name.clone()));
    }
    if current.state == SecretState::Deleted {
        return Err(UseDenied::SecretDeleted);
    }
    if entry.epoch_at_admission < current.revocation_epoch {
        return Err(UseDenied::RevokedAfterAdmission {
            admitted: entry.epoch_at_admission,
            current: current.revocation_epoch,
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use aex_wire::ids::{OrganizationId, PrefixedId as _, SessionId, Uuid7, WorkspaceId};
    use aex_wire::types::{Region, Timestamp};

    use super::{
        CloneCredentials, CustodyRejection, CustodyRevision, CustodyState, OwnerKeyEdgeId,
        SessionCustody, TrueIdle, TrueIdleViolation, UseDenied, admit_custody, clone_custody,
        delete_custody, managed_use_allowed, rebind,
    };
    use crate::context::{EncryptionContext, Plane};
    use crate::revocation::revoke;
    use crate::secret::{CiphertextRef, SecretName, SourceGeneration, WorkspaceSecret, set};

    fn workspace() -> WorkspaceId {
        WorkspaceId::from_uuid7(Uuid7::compose(1, [1; 10]))
    }

    fn session(tag: u8) -> SessionId {
        SessionId::from_uuid7(Uuid7::compose(1, [tag; 10]))
    }

    fn edge(tag: u8) -> OwnerKeyEdgeId {
        OwnerKeyEdgeId(Uuid7::compose(1, [tag; 10]))
    }

    fn moment(millis: i64) -> Timestamp {
        Timestamp::from_unix_millis(millis).expect("in range")
    }

    fn secret(name: &str) -> WorkspaceSecret {
        let parsed = SecretName::parse(name).expect("valid");
        set(
            None,
            workspace(),
            &parsed,
            CiphertextRef {
                key_generation: 1,
                wrapped_key: vec![1; 32],
                nonce: vec![1; 12],
                ciphertext: vec![1; 48],
            },
            EncryptionContext {
                plane: Plane::Prd,
                region: Region::ALL[0],
                organization: OrganizationId::from_uuid7(Uuid7::compose(1, [2; 10])),
                workspace: workspace(),
                name: parsed.clone(),
                generation: SourceGeneration::FIRST,
                custody_revision: None,
            },
            moment(0),
        )
        .expect("creates")
        .secret
    }

    fn custody() -> SessionCustody {
        admit_custody(
            session(3),
            workspace(),
            None,
            &[secret("beta"), secret("alpha")],
            edge(4),
            moment(1),
        )
        .expect("admits")
    }

    #[test]
    fn admission_orders_entries_and_reports_names_only() {
        let row = custody();
        assert_eq!(row.revision, CustodyRevision::FIRST);
        let described = row.describe();
        assert_eq!(
            described
                .names
                .iter()
                .map(|name| name.as_str().to_owned())
                .collect::<Vec<_>>(),
            vec!["alpha".to_owned(), "beta".to_owned()]
        );
        assert_eq!(described.revision, CustodyRevision::FIRST);
    }

    #[test]
    fn a_second_admission_is_rejected() {
        let row = custody();
        assert_eq!(
            admit_custody(
                session(3),
                workspace(),
                Some(&row),
                &[secret("alpha")],
                edge(5),
                moment(2)
            ),
            Err(CustodyRejection::AlreadyAdmitted)
        );
    }

    #[test]
    fn rebind_is_idle_only_and_destroys_the_prior_edge() {
        let row = custody();
        assert_eq!(
            rebind(
                &row,
                &[secret("alpha")],
                &TrueIdle {
                    observed_at: moment(2),
                    violation: Some(TrueIdleViolation::WorkQueued)
                },
                edge(5),
                moment(2)
            ),
            Err(CustodyRejection::NotTrueIdle(TrueIdleViolation::WorkQueued))
        );

        let commit = rebind(
            &row,
            &[secret("alpha")],
            &TrueIdle::idle(moment(2)),
            edge(5),
            moment(2),
        )
        .expect("rebinds");
        assert_eq!(commit.custody.revision, CustodyRevision(2));
        assert_eq!(commit.custody.owner_key_edge, edge(5));
        assert_eq!(commit.destroy_key_edges, vec![edge(4)]);
    }

    #[test]
    fn a_revocation_denies_a_previously_admitted_entry() {
        let row = custody();
        let alpha = secret("alpha");
        let entry = row
            .entry(&SecretName::parse("alpha").expect("valid"))
            .expect("bound")
            .clone();
        assert_eq!(
            managed_use_allowed(&entry, &alpha, row.revision, &row),
            Ok(())
        );

        let revoked = revoke(&alpha, moment(3)).expect("revokes").secret;
        assert_eq!(
            managed_use_allowed(&entry, &revoked, row.revision, &row),
            Err(UseDenied::RevokedAfterAdmission {
                admitted: crate::revocation::RevocationEpoch::INITIAL,
                current: crate::revocation::RevocationEpoch(1),
            })
        );
    }

    #[test]
    fn use_requires_the_exact_custody_revision() {
        let row = custody();
        let alpha = secret("alpha");
        let entry = row
            .entry(&SecretName::parse("alpha").expect("valid"))
            .expect("bound")
            .clone();
        assert_eq!(
            managed_use_allowed(&entry, &alpha, CustodyRevision(99), &row),
            Err(UseDenied::CustodyRevisionMismatch {
                current: CustodyRevision::FIRST,
                presented: CustodyRevision(99),
            })
        );

        let deleted = delete_custody(&row, moment(4));
        assert_eq!(deleted.custody.state, CustodyState::Deleted);
        assert_eq!(deleted.destroy_key_edges, vec![edge(4)]);
        assert_eq!(
            managed_use_allowed(&entry, &alpha, deleted.custody.revision, &deleted.custody),
            Err(UseDenied::CustodyDeleted)
        );
    }

    #[test]
    fn clone_copy_preserves_lineage_and_none_writes_nothing() {
        let row = custody();
        let copied = clone_custody(
            Some(&row),
            session(7),
            CloneCredentials::Copy,
            Some(edge(8)),
            moment(5),
        )
        .expect("clones");
        let child = copied.custody.expect("written");
        assert_eq!(child.entries, row.entries);
        assert_ne!(child.owner_key_edge, row.owner_key_edge);
        assert_eq!(copied.revision, CustodyRevision::FIRST);

        let none = clone_custody(
            Some(&row),
            session(9),
            CloneCredentials::None,
            None,
            moment(5),
        )
        .expect("clones");
        assert_eq!(none.custody, None);
        assert_eq!(none.revision, CustodyRevision::NONE);
    }
}
