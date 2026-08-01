//! `aex-registry-dynamodb` owns the `regional-registry` table adapter: current named
//! pointers, revisions, `ETag` conditions, uploads and idempotency receipts.
//!
//! # Invariants
//!
//! - a pointer update is conditional on the observed revision, so concurrent overwrites
//!   cannot interleave
//! - an idempotency receipt is written in the same transaction as the effect it records
//! - an expired upload is unreadable, not merely marked
//!
//! # Not this crate's job
//!
//! - the registry model itself (`aex-workspace-domain`)
//! - content storage (`aex-content-dynamodb`, `aex-content-aws`)
//! - authorization

pub mod expressions;
pub mod store;
