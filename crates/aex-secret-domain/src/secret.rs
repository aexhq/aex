//! The workspace secret record.
//!
//! Generation lineage is hidden and monotone. Each `set` mints a **new**
//! [`SourceGeneration`] and bumps the revision; `delete` bumps the revision to a
//! tombstone but mints no source and does not disturb an existing session. There
//! is deliberately no public history, copy, restore or version list: a secret's
//! past values are not a feature.

use aex_wire::ids::{ResourceName, WorkspaceId};
use aex_wire::types::Timestamp;

use crate::context::EncryptionContext;
use crate::revocation::RevocationEpoch;

/// A secret name. The grammar is `aex-wire`'s resource-name grammar, so exactly
/// one name type exists in the workspace.
pub type SecretName = ResourceName;

/// The hidden, strictly increasing source generation of one `(workspace, name)`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SourceGeneration(pub u64);

impl SourceGeneration {
    /// The generation the first `set` mints.
    pub const FIRST: Self = Self(1);

    /// The next generation.
    ///
    /// # Panics
    ///
    /// Panics on `u64` overflow, which is a corrupted authority rather than a
    /// customer condition.
    #[must_use]
    pub const fn next(self) -> Self {
        match self.0.checked_add(1) {
            Some(value) => Self(value),
            None => panic!("source generation overflowed"),
        }
    }
}

/// The monotone revision of a secret record.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SecretRevision(pub u64);

impl SecretRevision {
    /// The revision the first `set` writes.
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
            None => panic!("secret revision overflowed"),
        }
    }
}

/// Where a secret record is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum SecretState {
    /// Usable.
    Ready,
    /// Revoked; every earlier admission is denied.
    Revoked,
    /// Tombstoned.
    Deleted,
}

impl SecretState {
    /// Every state, in canonical order.
    pub const ALL: [Self; 3] = [Self::Ready, Self::Revoked, Self::Deleted];
}

/// An opaque reference to stored ciphertext.
///
/// The domain never inspects it and never mints one; an adapter produces it and
/// an adapter consumes it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CiphertextRef {
    /// The key generation the value was wrapped under.
    pub key_generation: u64,
    /// The wrapped data key, as stored.
    pub wrapped_key: Vec<u8>,
    /// The nonce.
    pub nonce: Vec<u8>,
    /// The ciphertext itself.
    pub ciphertext: Vec<u8>,
}

/// One workspace secret.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceSecret {
    /// The owning workspace.
    pub workspace: WorkspaceId,
    /// Its name.
    pub name: SecretName,
    /// The current source generation.
    pub generation: SourceGeneration,
    /// The current record revision.
    pub revision: SecretRevision,
    /// Where the record is.
    pub state: SecretState,
    /// The revocation epoch in force for the name.
    pub revocation_epoch: RevocationEpoch,
    /// The stored ciphertext, when the record still has one.
    pub ciphertext: Option<CiphertextRef>,
    /// The context the ciphertext is bound to.
    pub context: EncryptionContext,
    /// When the record was created.
    pub created_at: Timestamp,
    /// When it last changed.
    pub updated_at: Timestamp,
    /// When it was revoked.
    pub revoked_at: Option<Timestamp>,
}

impl WorkspaceSecret {
    /// Whether the record may be admitted into a session's custody.
    #[must_use]
    pub const fn is_ready(&self) -> bool {
        matches!(self.state, SecretState::Ready)
    }
}

/// The record after a `set` or `delete`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SecretCommit {
    /// The record after the change.
    pub secret: WorkspaceSecret,
    /// Whether a new source generation was minted.
    pub minted_generation: bool,
}

/// Why a secret change was refused.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SecretRejection {
    /// The record is already a tombstone.
    #[error("secret `{0}` is deleted")]
    Deleted(SecretName),
    /// The change named a different workspace.
    #[error("secret `{name}` does not belong to the named workspace")]
    WrongWorkspace {
        /// The name that was asked for.
        name: SecretName,
    },
    /// The context did not name the record it is being written to.
    #[error("encryption context does not name secret `{name}` generation {generation:?}")]
    ContextMismatch {
        /// The record's name.
        name: SecretName,
        /// The generation the context should have named.
        generation: SourceGeneration,
    },
}

/// Creates or replaces a secret.
///
/// Every accepted `set` mints a new source generation and bumps the revision,
/// even when the plaintext is unchanged: the domain never sees the plaintext, so
/// it cannot claim two ciphertexts are the same value.
///
/// # Errors
///
/// Returns [`SecretRejection`] when the record is a tombstone, when the caller
/// names a different workspace, or when the supplied context does not name the
/// generation being written.
pub fn set(
    current: Option<&WorkspaceSecret>,
    workspace: WorkspaceId,
    name: &SecretName,
    ciphertext: CiphertextRef,
    context: EncryptionContext,
    now: Timestamp,
) -> Result<SecretCommit, SecretRejection> {
    let (generation, revision, created_at, epoch) = match current {
        Some(existing) => {
            if existing.workspace != workspace {
                return Err(SecretRejection::WrongWorkspace { name: name.clone() });
            }
            if existing.state == SecretState::Deleted {
                return Err(SecretRejection::Deleted(name.clone()));
            }
            (
                existing.generation.next(),
                existing.revision.next(),
                existing.created_at,
                existing.revocation_epoch,
            )
        }
        None => (
            SourceGeneration::FIRST,
            SecretRevision::FIRST,
            now,
            RevocationEpoch::INITIAL,
        ),
    };

    if context.name != *name || context.generation != generation {
        return Err(SecretRejection::ContextMismatch {
            name: name.clone(),
            generation,
        });
    }

    Ok(SecretCommit {
        secret: WorkspaceSecret {
            workspace,
            name: name.clone(),
            generation,
            revision,
            // A later `set` mints a new generation but never lowers the epoch,
            // so an existing session stays blocked until an explicit rebind.
            state: SecretState::Ready,
            revocation_epoch: epoch,
            ciphertext: Some(ciphertext),
            context,
            created_at,
            updated_at: now,
            revoked_at: None,
        },
        minted_generation: true,
    })
}

/// Tombstones a secret.
///
/// The revision advances so a concurrent editor's precondition fails, but no
/// source generation is minted and no existing session's custody is disturbed:
/// deleting the workspace record is not a revocation.
///
/// # Errors
///
/// Returns [`SecretRejection::Deleted`] when the record is already a tombstone.
pub fn delete(current: &WorkspaceSecret, now: Timestamp) -> Result<SecretCommit, SecretRejection> {
    if current.state == SecretState::Deleted {
        return Err(SecretRejection::Deleted(current.name.clone()));
    }
    Ok(SecretCommit {
        secret: WorkspaceSecret {
            revision: current.revision.next(),
            state: SecretState::Deleted,
            ciphertext: None,
            updated_at: now,
            ..current.clone()
        },
        minted_generation: false,
    })
}

#[cfg(test)]
mod tests {
    use aex_wire::ids::{OrganizationId, PrefixedId as _, Uuid7, WorkspaceId};
    use aex_wire::types::{Region, Timestamp};

    use super::{
        CiphertextRef, SecretName, SecretRejection, SecretRevision, SecretState, SourceGeneration,
        WorkspaceSecret, delete, set,
    };
    use crate::context::{EncryptionContext, Plane};

    fn workspace() -> WorkspaceId {
        WorkspaceId::from_uuid7(Uuid7::compose(1, [1; 10]))
    }

    fn moment(millis: i64) -> Timestamp {
        Timestamp::from_unix_millis(millis).expect("in range")
    }

    fn name() -> SecretName {
        SecretName::parse("openai-key").expect("valid")
    }

    fn context(generation: SourceGeneration) -> EncryptionContext {
        EncryptionContext {
            plane: Plane::Prd,
            region: Region::ALL[0],
            organization: OrganizationId::from_uuid7(Uuid7::compose(1, [2; 10])),
            workspace: workspace(),
            name: name(),
            generation,
            custody_revision: None,
        }
    }

    fn ciphertext(tag: u8) -> CiphertextRef {
        CiphertextRef {
            key_generation: 1,
            wrapped_key: vec![tag; 32],
            nonce: vec![tag; 12],
            ciphertext: vec![tag; 48],
        }
    }

    fn first() -> WorkspaceSecret {
        set(
            None,
            workspace(),
            &name(),
            ciphertext(1),
            context(SourceGeneration::FIRST),
            moment(0),
        )
        .expect("creates")
        .secret
    }

    #[test]
    fn every_set_mints_a_new_generation() {
        let one = first();
        assert_eq!(one.generation, SourceGeneration(1));
        assert_eq!(one.revision, SecretRevision(1));

        let two = set(
            Some(&one),
            workspace(),
            &name(),
            ciphertext(2),
            context(SourceGeneration(2)),
            moment(1),
        )
        .expect("replaces")
        .secret;
        assert_eq!(two.generation, SourceGeneration(2));
        assert_eq!(two.revision, SecretRevision(2));
        assert_eq!(two.created_at, one.created_at);
    }

    #[test]
    fn a_context_that_names_the_wrong_generation_is_rejected() {
        let one = first();
        assert_eq!(
            set(
                Some(&one),
                workspace(),
                &name(),
                ciphertext(2),
                context(SourceGeneration(7)),
                moment(1),
            ),
            Err(SecretRejection::ContextMismatch {
                name: name(),
                generation: SourceGeneration(2),
            })
        );
    }

    #[test]
    fn delete_tombstones_without_minting_a_generation() {
        let one = first();
        let commit = delete(&one, moment(5)).expect("deletes");
        assert!(!commit.minted_generation);
        assert_eq!(commit.secret.state, SecretState::Deleted);
        assert_eq!(commit.secret.generation, one.generation);
        assert_eq!(commit.secret.revision, one.revision.next());
        assert_eq!(commit.secret.ciphertext, None);
        assert_eq!(
            delete(&commit.secret, moment(6)),
            Err(SecretRejection::Deleted(name()))
        );
    }
}
