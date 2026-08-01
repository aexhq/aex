//! `aex-content-domain` owns the pure content descriptor, Merkle page, ownership-closure
//! and deletion model.
//!
//! # Invariants
//!
//! - a descriptor is content-addressed: equal bytes produce an equal identity;
//! - a Merkle root is history independent — the same entry set always yields the
//!   same root, whatever order the entries were written in;
//! - the owner/root closure is computed, never asserted; an unreferenced object
//!   is provably orphaned;
//! - a deletion-denial epoch beats a concurrent delete decision, and a metadata
//!   restore cannot outrun the denial ledger.
//!
//! # Not this crate's job
//!
//! - `S3` or `KMS` calls (`aex-content-aws`);
//! - table expressions (`aex-content-dynamodb`);
//! - lifecycle scheduling (`content-lifecycle-worker`).
//!
//! Every canonical byte encoding here is hand-written, fixed-width and
//! length-prefixed. Nothing hashed by this crate passes through `serde`, so a
//! serialization bump can never move a persisted digest.

pub mod denial;
pub mod descriptor;
pub mod digest;
pub mod gc;
pub mod identity;
pub mod missing;
pub mod path;
pub mod pin;
pub mod placement;
pub mod tree;

pub use denial::{
    DeletionDenial, DenialEpoch, DenialProjection, DenialSubject, UnwrapDenied, unwrap_allowed,
};
pub use descriptor::{CiphertextIdentity, ContentDescriptor, MediaType, MediaTypeError};
pub use digest::{ContentDigest, Crc32c, PageDigest, PageDigestError};
pub use gc::{
    GcCondition, GcEpoch, ReachableFrom, RetainReason, STAGED_ORPHAN_GRACE, STAGED_ORPHAN_GRACE_MS,
    SweepCandidate, SweepDecision, sweep_decision,
};
pub use identity::{CursorId, GrantId, RegistryKind, Revision};
pub use missing::{ContentMissing, ContentOutcome, MissingReason, Remediation};
pub use path::{
    FileMode, MAX_PATH_BYTES, MAX_SEGMENT_BYTES, NormalizedPath, PathError, RelativeInternalPath,
};
pub use pin::{OwnerEdge, Pin, PinSet, PinSubject, RegisteredName, RootKind};
pub use placement::{
    APPLICATION_ITEM_MAX_BYTES, ContentObjectKey, INLINE_PLACEMENT_MAX_BYTES, ObjectKeyError,
    Placement, PlacementClass, TREE_PAGE_TARGET_BYTES, placement_for,
};
pub use tree::{
    BranchChild, BranchPage, BuiltTree, ContentRoot, EntryNode, LeafPage, NodeKind, Reference,
    SPLIT_THRESHOLD, TreeDelta, TreeEntry, TreeError, TreeMutation, TreeNode, TreeView,
    apply_mutations, build_tree, canonical_page_bytes, clone_root, empty_root, is_boundary,
    page_digest, references, root_of, verify_root,
};
