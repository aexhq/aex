//! Reachability authorities that keep registered content bodies alive.
//!
//! Session filesystem roots, persistence cursors, and operation roots are not
//! content authorities. Registered pointers, temporary download grants, and a
//! collector's own examination fence are the complete retained set.

use std::collections::BTreeSet;

use aex_wire::ids::{ResourceName, WorkspaceId};
use aex_wire::types::Timestamp;

use crate::digest::ContentDigest;
use crate::gc::GcEpoch;
use crate::identity::{GrantId, RegistryKind, Revision};

/// A registry name. The registry grammar is `aex-wire`'s resource-name grammar.
pub type RegisteredName = ResourceName;

/// One reachability authority.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Pin {
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
            Self::Registry { .. } => crate::gc::RetainReason::RegistryPin,
            Self::Grant { .. } => crate::gc::RetainReason::GrantPin,
            Self::Gc { .. } => crate::gc::RetainReason::GcPin,
        }
    }

    /// The body this pin holds directly, when it holds one.
    #[must_use]
    pub const fn body(&self) -> Option<&ContentDigest> {
        match self {
            Self::Grant { digest, .. } | Self::Gc { digest, .. } => Some(digest),
            Self::Registry { .. } => None,
        }
    }

    /// Whether the pin is still live at `now`.
    #[must_use]
    pub fn is_live_at(&self, now: Timestamp) -> bool {
        match self {
            Self::Grant { expires_at, .. } => now.unix_millis() < expires_at.unix_millis(),
            Self::Registry { .. } | Self::Gc { .. } => true,
        }
    }
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
    use aex_wire::ids::Uuid7;
    use aex_wire::types::Timestamp;

    use super::{Pin, PinSet};

    fn moment(millis: i64) -> Timestamp {
        Timestamp::from_unix_millis(millis).expect("in range")
    }

    #[test]
    fn a_pin_set_is_a_set() {
        let pin = Pin::Grant {
            grant: crate::GrantId(Uuid7::compose(1, [1; 10])),
            digest: crate::ContentDigest::of(b"body"),
            expires_at: moment(10),
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
            grant: crate::GrantId(Uuid7::compose(1, [2; 10])),
            digest: crate::ContentDigest::of(b"body"),
            expires_at: moment(10),
        };
        assert!(pin.is_live_at(moment(9)));
        assert!(!pin.is_live_at(moment(10)));
    }
}
