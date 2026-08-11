//! Session lineage.
//!
//! Clone is not part of the launch contract. The retained edge vocabulary exists only so
//! deletion can detach or cascade over historical descendants without preserving a workspace
//! root or persistence revision.

use aex_wire::ids::{OperationId, SessionId};
use aex_wire::types::Timestamp;

/// Where a session came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Lineage {
    /// The session it was cloned from, when it was.
    pub origin: Option<Origin>,
}

impl Lineage {
    /// The lineage of a session created from nothing.
    pub const ROOT: Self = Self { origin: None };

    /// Whether the session descends from another.
    #[must_use]
    pub const fn is_clone(&self) -> bool {
        self.origin.is_some()
    }
}

/// The historical clone edge itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Origin {
    /// The source session.
    pub session: SessionId,
    /// The operation that made the clone.
    pub operation: OperationId,
    /// When it was made.
    pub cloned_at: Timestamp,
}

/// How a purge treats a session's lineage descendants.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum PurgeCascade {
    /// Clear each descendant's origin link and leave the descendant alive.
    DetachDescendants,
    /// Purge the whole lineage-descendant closure.
    PurgeClosure,
}

/// Clears a clone edge without touching anything else about the child.
#[must_use]
pub const fn detach(lineage: Lineage) -> Lineage {
    let _ = lineage;
    Lineage::ROOT
}

#[cfg(test)]
mod tests {
    use aex_wire::ids::{OperationId, PrefixedId as _, SessionId, Uuid7};
    use aex_wire::types::Timestamp;

    use super::{Lineage, Origin, detach};

    #[test]
    fn detaching_clears_the_origin() {
        let lineage = Lineage {
            origin: Some(Origin {
                session: SessionId::from_uuid7(Uuid7::compose(1, [1; 10])),
                operation: OperationId::from_uuid7(Uuid7::compose(1, [2; 10])),
                cloned_at: Timestamp::from_unix_millis(0).expect("in range"),
            }),
        };
        assert_eq!(detach(lineage), Lineage::ROOT);
        assert!(!detach(lineage).is_clone());
    }
}
