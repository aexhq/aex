//! Lineage and clone.
//!
//! `clone` is the accepted public name (D-11); `fork` is not implemented and has
//! no alias. Clone lineage and subagent parentage are different relations: a
//! clone descends from a source **session**, while a subagent belongs to the
//! session it was spawned in and always goes with it.

use aex_content_domain::ContentRoot;
use aex_secret_domain::CloneCredentials;
use aex_wire::ids::{OperationId, SessionId};
use aex_wire::types::Timestamp;

use crate::ids::PersistRevision;

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

/// The clone edge itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Origin {
    /// The source session.
    pub session: SessionId,
    /// The operation that made the clone.
    pub operation: OperationId,
    /// The source persist revision captured by the clone.
    pub source_persist_revision: PersistRevision,
    /// Which files the clone took.
    pub files: CloneFiles,
    /// When it was made.
    pub cloned_at: Timestamp,
}

/// Which file root a clone adopts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum CloneFiles {
    /// The exact live tree, captured into a child-owned root without advancing
    /// the source's persist revision.
    Current,
    /// The source's `initial_root`.
    Initial,
    /// An empty root.
    None,
}

impl CloneFiles {
    /// Every mode, in canonical order.
    pub const ALL: [Self; 3] = [Self::Current, Self::Initial, Self::None];

    /// Whether the mode has to read the live workspace, and therefore whether it
    /// needs the true-idle guard at all. Only `Current` does.
    #[must_use]
    pub const fn reads_live_workspace(self) -> bool {
        matches!(self, Self::Current)
    }
}

/// What a clone asks for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CloneRequest {
    /// The source session.
    pub source: SessionId,
    /// The child's identity.
    pub target: SessionId,
    /// The operation making the clone.
    pub operation: OperationId,
    /// The source persist revision captured with the request.
    pub source_persist_revision: PersistRevision,
    /// Which files.
    pub files: CloneFiles,
    /// Which credentials.
    pub credentials: CloneCredentials,
}

/// The lineage and root a clone results in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CloneOutcome {
    /// The child's lineage.
    pub lineage: Lineage,
    /// The root the child adopts.
    pub root: ContentRoot,
    /// Whether the source's live workspace had to be read.
    pub read_live_workspace: bool,
}

/// Resolves what a clone adopts, without touching the source.
///
/// The source is passed by reference and never returned, which is the structural
/// half of clone independence: there is no code path by which cloning writes to
/// its source.
#[must_use]
pub fn plan_clone(
    request: &CloneRequest,
    initial_root: ContentRoot,
    live_root: ContentRoot,
    empty_root: ContentRoot,
    now: Timestamp,
) -> CloneOutcome {
    let root = match request.files {
        CloneFiles::Current => live_root,
        CloneFiles::Initial => initial_root,
        CloneFiles::None => empty_root,
    };
    CloneOutcome {
        lineage: Lineage {
            origin: Some(Origin {
                session: request.source,
                operation: request.operation,
                source_persist_revision: request.source_persist_revision,
                files: request.files,
                cloned_at: now,
            }),
        },
        root,
        read_live_workspace: request.files.reads_live_workspace(),
    }
}

/// How a purge treats a session's lineage descendants.
///
/// No public route mints a descendant today: `session_clone` was withdrawn from
/// the launch contract, and this domain machinery is what a later clone route
/// would be built on. The distinction survives because a purge over a lineage
/// that *does* exist has two genuinely different meanings, and a boolean cannot
/// carry which one was asked for.
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
    use aex_content_domain::ContentRoot;
    use aex_secret_domain::CloneCredentials;
    use aex_wire::ids::{OperationId, PrefixedId as _, SessionId, Uuid7};
    use aex_wire::types::Timestamp;

    use super::{CloneFiles, CloneRequest, Lineage, detach, plan_clone};

    fn root(tag: u8, entries: u64) -> ContentRoot {
        ContentRoot {
            digest: [tag; 32],
            entries,
            logical_bytes: u64::from(tag) * 10,
        }
    }

    fn request(files: CloneFiles) -> CloneRequest {
        CloneRequest {
            source: SessionId::from_uuid7(Uuid7::compose(1, [1; 10])),
            target: SessionId::from_uuid7(Uuid7::compose(1, [2; 10])),
            operation: OperationId::from_uuid7(Uuid7::compose(1, [3; 10])),
            source_persist_revision: crate::PersistRevision(7),
            files,
            credentials: CloneCredentials::Copy,
        }
    }

    #[test]
    fn each_mode_selects_its_own_root_and_only_current_reads_live() {
        let now = Timestamp::from_unix_millis(0).expect("in range");
        let cases = [
            (CloneFiles::Current, root(3, 3), true),
            (CloneFiles::Initial, root(1, 1), false),
            (CloneFiles::None, root(0, 0), false),
        ];
        for (files, expected, reads_live) in cases {
            let outcome = plan_clone(&request(files), root(1, 1), root(3, 3), root(0, 0), now);
            assert_eq!(outcome.root, expected);
            assert_eq!(outcome.read_live_workspace, reads_live);
            assert!(outcome.lineage.is_clone());
        }
    }

    #[test]
    fn detaching_clears_the_origin_and_nothing_else() {
        let now = Timestamp::from_unix_millis(0).expect("in range");
        let outcome = plan_clone(
            &request(CloneFiles::Initial),
            root(1, 1),
            root(3, 3),
            root(0, 0),
            now,
        );
        assert_eq!(detach(outcome.lineage), Lineage::ROOT);
        assert!(!detach(outcome.lineage).is_clone());
    }
}
