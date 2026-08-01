//! `aex-workspace-domain` owns the pure workspace, named-registry and grant model:
//! immutable revisions, `ETag` semantics, root/grant pins and upload state.
//!
//! # Invariants
//!
//! - a revision is immutable; a new value is a new revision with a new `ETag`
//! - a conditional write against a stale `ETag` is rejected, never merged
//! - a pinned root or grant keeps its target reachable until the pin is released
//!
//! # Not this crate's job
//!
//! - `DynamoDB` expressions (`aex-registry-dynamodb`)
//! - content bytes or Merkle pages (`aex-content-domain`)
//! - authorization decisions about who may hold a grant

pub mod grant;
pub mod registry;
pub mod upload;
