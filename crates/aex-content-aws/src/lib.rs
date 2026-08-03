//! `aex-content-aws` owns the S3 content object adapter: workspace-scoped
//! addressing, conditional create, multipart upload and completion, presigned
//! download grants, the fenced delete, and the error mapping that turns an S3
//! condition into a typed product outcome.
//!
//! # Invariants
//!
//! - an object key is a pure function of `(workspace, digest)`, so a create is
//!   idempotent and a delete provably targets the exact body (D-15)
//! - a create is conditional and an existing key is compared, never overwritten;
//!   a mismatch is a hard `DigestCollision`, and substituted bytes are never
//!   returned
//! - a delete names the `ETag` the sweep decided on, and the bucket policy
//!   refuses one that does not (D-13/D-14)
//! - a presigned URL is a bearer credential and cannot print itself
//! - an ambiguous completion is resolved by `HeadObject`, **never** by
//!   re-issuing the completion or by aborting the upload
//!
//! # Not this crate's job
//!
//! - content metadata rows (`aex-content-dynamodb`)
//! - the descriptor and Merkle model (`aex-content-domain`)
//! - deletion policy (`content-lifecycle-worker`)
//! - application-layer encryption (`aex-secret-aws`); this crate binds the
//!   encryption context to SSE-KMS and never holds key material

pub mod errors;
pub mod multipart;
pub mod object_key;
pub mod object_store;
pub mod policy;
pub mod redacted;

pub use errors::ContentObjectError;
pub use multipart::{CompletionManifest, MultipartHandle, PartPlan, ProviderPart};
pub use object_key::{MAX_SIGNATURE_AGE_MILLIS, ObjectKey, PRESIGN_EXPIRY};
pub use object_store::{
    BoundedObject, BucketBinding, ContentObjectStore, FencedDeleteOutcome, ObjectCommit,
    S3ContentObjects,
};
pub use redacted::RedactedUrl;
