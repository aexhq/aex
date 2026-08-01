//! Reachability: the closed set of authorities that keep a body alive.
//!
//! There are no synchronous reference counts. A body is retained exactly when
//! some [`Pin`] reaches it, and the pin variants below are the whole
//! vocabulary — an object listing is repair evidence, never an authority, so
//! this module exposes no constructor that turns a listing into an
//! [`OwnerEdge`].

use std::collections::BTreeSet;

use aex_wire::ids::{OperationId, ResourceName, SessionId, WorkspaceId};
use aex_wire::types::Timestamp;

use crate::digest::ContentDigest;
use crate::gc::GcEpoch;
use crate::identity::{CursorId, GrantId, RegistryKind, Revision};
use crate::tree::ContentRoot;

/// A registry name. The registry grammar is `aex-wire`'s resource-name grammar,
/// so exactly one name type exists in the workspace.
pub type RegisteredName = ResourceName;

/// Which role a session root plays.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum RootKind {
    /// The root the session was created with.
    Initial,
    /// The durable root the session last persisted.
    Persisted,
    /// A root staged by an in-flight persist.
    StagedPersist,
    /// A root a clone adopted from its source.
    CloneSource,
    /// A root an in-flight operation holds.
    OperationRoot,
}

impl RootKind {
    /// Every kind, in canonical order.
    pub const ALL: [Self; 5] = [
        Self::Initial,
        Self::Persisted,
        Self::StagedPersist,
        Self::CloneSource,
        Self::OperationRoot,
    ];

    /// Whether the kind is subject to the staged-orphan grace window.
    #[must_use]
    pub const fn is_staged(self) -> bool {
        matches!(self, Self::StagedPersist)
    }
}

/// One reachability authority.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Pin {
    /// A session holds a root.
    Root {
        /// The owning session.
        session: SessionId,
        /// Which role the root plays.
        kind: RootKind,
        /// The pinned root.
        root: ContentRoot,
    },
    /// A registry pointer holds its current value.
    Registry {
        /// The owning workspace.
        workspace: WorkspaceId,
        /// Which registry.
        kind: RegistryKind,
        /// Which name.
        name: RegisteredName,
        /// Which revision.
        revision: Revision,
    },
    /// An unexpired download grant holds a body.
    Grant {
        /// The grant.
        grant: GrantId,
        /// The pinned body.
        digest: ContentDigest,
        /// When the pin lapses.
        expires_at: Timestamp,
    },
    /// An unexpired query cursor holds a root.
    Cursor {
        /// The cursor.
        cursor: CursorId,
        /// The pinned root.
        root: ContentRoot,
        /// When the pin lapses.
        expires_at: Timestamp,
    },
    /// An in-flight operation holds a root.
    Operation {
        /// The operation.
        operation: OperationId,
        /// The pinned root.
        root: ContentRoot,
    },
    /// A garbage-collection epoch holds a body under examination.
    Gc {
        /// The epoch.
        epoch: GcEpoch,
        /// The pinned body.
        digest: ContentDigest,
    },
}

impl Pin {
    /// Which retain reason this pin supplies.
    #[must_use]
    pub const fn retain_reason(&self) -> crate::gc::RetainReason {
        match self {
            Self::Root { .. } => crate::gc::RetainReason::RootPin,
            Self::Registry { .. } => crate::gc::RetainReason::RegistryPin,
            Self::Grant { .. } => crate::gc::RetainReason::GrantPin,
            Self::Cursor { .. } => crate::gc::RetainReason::CursorPin,
            Self::Operation { .. } => crate::gc::RetainReason::OperationPin,
            Self::Gc { .. } => crate::gc::RetainReason::GcPin,
        }
    }

    /// The root this pin holds, when it holds one.
    #[must_use]
    pub const fn root(&self) -> Option<&ContentRoot> {
        match self {
            Self::Root { root, .. } | Self::Cursor { root, .. } | Self::Operation { root, .. } => {
                Some(root)
            }
            Self::Registry { .. } | Self::Grant { .. } | Self::Gc { .. } => None,
        }
    }

    /// The body this pin holds directly, when it holds one.
    #[must_use]
    pub const fn body(&self) -> Option<&ContentDigest> {
        match self {
            Self::Grant { digest, .. } | Self::Gc { digest, .. } => Some(digest),
            Self::Root { .. }
            | Self::Registry { .. }
            | Self::Cursor { .. }
            | Self::Operation { .. } => None,
        }
    }

    /// Whether the pin is still live at `now`. A pin with no expiry is always
    /// live; expiry is inclusive of its instant.
    #[must_use]
    pub fn is_live_at(&self, now: Timestamp) -> bool {
        match self {
            Self::Grant { expires_at, .. } | Self::Cursor { expires_at, .. } => {
                now.unix_millis() < expires_at.unix_millis()
            }
            Self::Root { .. }
            | Self::Registry { .. }
            | Self::Operation { .. }
            | Self::Gc { .. } => true,
        }
    }
}

/// What a pin is attached to, from the owner's side.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum PinSubject {
    /// A session owns the pin.
    Session(SessionId),
    /// A workspace owns the pin.
    Workspace(WorkspaceId),
    /// An operation owns the pin.
    Operation(OperationId),
}

/// A durable owner-to-pin edge.
///
/// The edge is the only thing an unwrap may be authorized against; there is no
/// path from a bucket listing to one.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct OwnerEdge {
    /// The owning workspace.
    pub workspace: WorkspaceId,
    /// Who owns the pin.
    pub subject: PinSubject,
    /// The pin itself.
    pub pin: Pin,
}

/// The pins currently in force.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PinSet(BTreeSet<Pin>);

impl PinSet {
    /// An empty set.
    #[must_use]
    pub fn new() -> Self {
        Self(BTreeSet::new())
    }

    /// Adds a pin, reporting whether it was new.
    pub fn insert(&mut self, pin: Pin) -> bool {
        self.0.insert(pin)
    }

    /// Removes a pin, reporting whether it was present.
    pub fn remove(&mut self, pin: &Pin) -> bool {
        self.0.remove(pin)
    }

    /// How many pins are held.
    #[must_use]
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// Whether no pin is held.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Iterates the pins in canonical order.
    pub fn iter(&self) -> impl Iterator<Item = &Pin> {
        self.0.iter()
    }
}

impl FromIterator<Pin> for PinSet {
    fn from_iter<I: IntoIterator<Item = Pin>>(iter: I) -> Self {
        Self(iter.into_iter().collect())
    }
}

#[cfg(test)]
mod tests {
    use aex_wire::ids::{PrefixedId as _, SessionId, Uuid7};
    use aex_wire::types::Timestamp;

    use super::{Pin, PinSet, RootKind};
    use crate::digest::ContentDigest;
    use crate::identity::GrantId;
    use crate::tree::ContentRoot;

    fn moment(millis: i64) -> Timestamp {
        Timestamp::from_unix_millis(millis).expect("in range")
    }

    #[test]
    fn a_pin_set_is_a_set() {
        let pin = Pin::Root {
            session: SessionId::from_uuid7(Uuid7::compose(1, [1; 10])),
            kind: RootKind::Persisted,
            root: ContentRoot {
                digest: [0; 32],
                entries: 0,
                logical_bytes: 0,
            },
        };
        let mut set = PinSet::new();
        assert!(set.insert(pin.clone()));
        assert!(!set.insert(pin.clone()));
        assert_eq!(set.len(), 1);
        assert!(set.remove(&pin));
        assert!(set.is_empty());
    }

    #[test]
    fn expiry_is_exclusive_of_its_own_instant() {
        let pin = Pin::Grant {
            grant: GrantId(Uuid7::compose(1, [2; 10])),
            digest: ContentDigest::of(b"body"),
            expires_at: moment(1_000),
        };
        assert!(pin.is_live_at(moment(999)));
        assert!(!pin.is_live_at(moment(1_000)));
        assert!(!pin.is_live_at(moment(1_001)));
    }
}
