//! `aex-content-dynamodb` owns the content row family within the unified
//! `regional-file-authority` table: body descriptors, inline ciphertext bodies,
//! pins, download grants, the garbage-collection epoch and its candidates.
//!
//! # Invariants
//!
//! - **every customer-content partition is workspace scoped.** The system this
//!   replaces addressed a body without a workspace component, so identical
//!   bytes in two tenants shared one physical row and one deletion fate; that is
//!   a cross-tenant equality side channel and it is fixed here, not ported
//!   (D-15). Maintenance-authority rows are explicitly system/shard scoped and
//!   are not customer-addressable
//! - **pins live on roots.** A binding over 10,000 files writes one pin, not
//!   10,000, and only a loose body carries a direct pin (D-10)
//! - a download grant holds a content reference and a pin, **never a body copy**
//!   (D-11) — the type has nowhere to put one
//! - the collector re-reads the partition under strong consistency immediately
//!   before it decides, and any lost condition keeps the body
//! - `TTL` is never a fence: every expiring row also carries the explicit
//!   `expiresAt` the reader checks (D-24)
//!
//! # Not this crate's job
//!
//! - content bytes: `S3` and `KMS` belong to `aex-content-aws`
//! - the ownership-closure and Merkle model (`aex-content-domain`)
//! - lifecycle scheduling or deletion policy (`content-lifecycle-worker`)

pub mod application_plan;
pub mod codec;
pub mod expressions;
pub mod keys;
pub mod store;
pub mod wire_pending;

pub use codec::{ContentDescriptor, ContentPin, DownloadGrant, GcCandidate, GcEpoch};
pub use store::{ContentMetadataStore, ContentStore, Reachability};
pub use wire_pending::{GcSweepPlan, GrantPlan, InlineBody, PinOwner};
