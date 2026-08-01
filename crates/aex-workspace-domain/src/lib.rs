//! `aex-workspace-domain` owns the pure workspace, named-registry, upload, grant and
//! persist model: monotone revisions, `ETag` semantics, single-use uploads,
//! grant pins and the persist mirror.
//!
//! # Invariants
//!
//! - `revision` is a strong monotone concurrency token, not a version: no route
//!   reads, lists, restores or copies a prior value;
//! - an identical `set` is `Unchanged` and writes **nothing**, so an idempotent
//!   retry cannot invalidate another editor's `If-Match`;
//! - a conditional write against a stale `ETag` is rejected, never merged and
//!   never silently retried;
//! - an upload is single-use: `Consumed` is reachable only from a registry set
//!   that pins it in the same transaction;
//! - a grant stores a token hash and a pin, never a token and never a body copy.
//!
//! # Not this crate's job
//!
//! - `DynamoDB` expressions (`aex-registry-dynamodb`);
//! - content bytes or Merkle pages (`aex-content-domain`);
//! - authorization decisions about who may hold a grant.

pub mod grant;
pub mod persist;
pub mod registry;
pub mod upload;

/// The registry name grammar, re-exported so a caller needs one import.
pub use aex_content_domain::{RegisteredName, RegistryKind, Revision};
pub use grant::{
    ByteRange, DownloadGrant, GRANT_TTL, GrantPlacement, GrantRejection, GrantSubject,
    MAX_SIGNED_RANGE_BYTES, Redemption, mint_grant, redeem,
};
pub use persist::{
    PatternAtom, PersistError, PersistPlan, PersistReceipt, PersistSelection, PersistShape,
    Selector, SelectorError, SelectorSegment, plan_persist, replay_receipt,
};
pub use registry::{
    DeleteCommit, ProposedValue, RegisteredValueRef, RegistryCommit, RegistryPointer,
    RegistryRejection, SetOutcome, ValueError, delete, etag_of, set,
};
pub use upload::{
    PART_GRANT_TTL, PART_MAX_BYTES, PART_MAX_COUNT, PART_MIN_BYTES, PartPlan, PartReceipt,
    PlannedPart, RegistrySelector, UPLOAD_GRACE, Upload, UploadCommit, UploadError, UploadState,
    VerifiedObject, abort, begin_complete, consume, expire, finish_complete, grant_parts,
    plan_parts,
};
