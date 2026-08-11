//! `aex-content-domain` owns registered content descriptors, reachability, and deletion.
//!
//! # Invariants
//!
//! - a descriptor is content-addressed: equal bytes produce an equal identity;
//! - registered pointers and grants are the only customer-facing body pins;
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

pub use denial::{DeletionDenial, DenialEpoch, DenialProjection, UnwrapDenied, unwrap_allowed};
pub use descriptor::{CiphertextIdentity, ContentDescriptor, MediaType, MediaTypeError};
pub use digest::{ContentDigest, Crc32c};
pub use gc::{
    GcCondition, GcEpoch, ReachableFrom, RetainReason, STAGED_ORPHAN_GRACE, STAGED_ORPHAN_GRACE_MS,
    SweepCandidate, SweepDecision, sweep_decision,
};
pub use identity::{GrantId, RegistryKind, Revision};
pub use missing::{ContentMissing, ContentOutcome, MissingReason, Remediation};
pub use path::{
    FileMode, MAX_PATH_BYTES, MAX_SEGMENT_BYTES, NormalizedPath, PathError, RelativeInternalPath,
};
pub use pin::{Pin, PinSet, RegisteredName};
pub use placement::{
    APPLICATION_ITEM_MAX_BYTES, ContentObjectKey, INLINE_PLACEMENT_MAX_BYTES, ObjectKeyError,
    Placement, PlacementClass, placement_for,
};
