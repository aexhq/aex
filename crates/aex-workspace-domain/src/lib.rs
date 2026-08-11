//! `aex-workspace-domain` owns the pure workspace, named-registry, upload, grant and
//! registry and content model: monotone revisions, `ETag` semantics,
//! single-use uploads, and grant pins.
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
//! - an upload names its provider multipart handle and its object key as
//!   non-optional facts, so a row that cannot be aborted or swept is
//!   unconstructible;
//! - a grant is a presigned object range that always arrives with its pin — never
//!   a redeemable token, because there is no redemption route.
//!
//! # Not this crate's job
//!
//! - `DynamoDB` expressions (`aex-registry-dynamodb`);
//! - content bytes or Merkle pages (`aex-content-domain`);
//! - authorization decisions about who may hold a grant.

pub mod grant;
pub mod registry;
pub mod upload;

/// The registry name grammar, re-exported so a caller needs one import.
pub use aex_content_domain::{RegisteredName, RegistryKind, Revision};
pub use grant::{
    ByteRange, ContentObjectLocation, DownloadGrant, GRANT_TTL, GrantContentDescriptor,
    GrantPlacement, GrantRejection, GrantSubject, MAX_SIGNED_RANGE_BYTES, ObjectChecksum,
    mint_grant,
};
pub use registry::{
    DeleteCommit, ProposedValue, RegisteredValueRef, RegistryCommit, RegistryPointer,
    RegistryRejection, RegistryRow, SetOutcome, ValueDocument, ValueError, delete, etag_of, set,
};
pub use upload::{
    AmbiguityResolution, CompletionEvidence, ExpiryOutcome, HeadOracle, PART_GRANT_MAX_PER_CALL,
    PART_GRANT_TTL, PART_MAX_BYTES, PART_MAX_COUNT, PART_MIN_BYTES, PartGrantRequest, PartPlan,
    PartReceipt, PlannedPart, RegistrySelector, SubmittedPart, UPLOAD_GRACE, Upload, UploadCommit,
    UploadError, UploadState, VerifiedObject, abort, begin_complete, consume, expire,
    finish_complete, grant_parts, plan_parts, resolve_completing,
};
