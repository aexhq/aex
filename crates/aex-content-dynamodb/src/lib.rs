//! `aex-content-dynamodb` owns the `regional-content` table adapter: descriptor rows,
//! Merkle pages, owner/root pins and the `GC` cursor.
//!
//! # Invariants
//!
//! - a root pin is re-read under the fence immediately before a delete decision
//! - descriptor rows are immutable once written; a change is a new descriptor
//! - the `GC` cursor advances only over pages it has fully evaluated
//!
//! # Not this crate's job
//!
//! - content bytes: `S3` and `KMS` belong to `aex-content-aws`
//! - the ownership-closure model (`aex-content-domain`)
//! - lifecycle scheduling or deletion policy

pub mod expressions;
pub mod store;
