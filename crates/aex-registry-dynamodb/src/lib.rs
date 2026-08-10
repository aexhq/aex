//! `aex-registry-dynamodb` owns the `regional-registry` table adapter: the
//! current `(workspace, kind, name)` pointer with its revision and `ETag`,
//! multipart upload staging, and idempotency receipts.
//!
//! # Invariants
//!
//! - a pointer write is conditional on the revision the caller observed, which
//!   is what makes the public `If-Match` mean something: the `ETag` is a pure
//!   function of `(kind, revision, digest)`
//! - the stored `ETag` is recomputed and compared on every read, so a tag that
//!   no longer describes its own row is corruption rather than a value
//! - an upload transition names exactly the state it moves from, so two callers
//!   cannot both complete
//! - an upload row carries **no TTL**: reclaiming it on a timer would orphan the
//!   live multipart upload it names
//! - there is no index and no stream on this table (D-22)
//!
//! # Not this crate's job
//!
//! - the registry and upload model itself (`aex-workspace-domain`)
//! - content storage (`aex-content-dynamodb`, `aex-content-aws`)
//! - authorization, and the object-store side of a multipart completion

pub mod application_plan;
pub mod codec;
pub mod expressions;
pub mod keys;
pub mod store;

pub use store::{PointerPage, RegistryDynamoStore, RegistryStore};
