//! `aex-work-dynamodb` owns the `regional-work` table: wake, due, claim, lease,
//! fence, attempt, dedupe claims and per-shard reconciliation cursors over one
//! sharded due index.
//!
//! # Invariants
//!
//! - the due index is sharded 64 ways and there is no literal single due
//!   partition anywhere; priority is a lead on the due time, so priority
//!   ordering and bounded ageing come from one sort key and no band can starve
//! - a work payload is typed, bounded and checked against a per-kind schema on
//!   both encode and decode, which is what makes the `NEW_IMAGE` stream safe
//!   rather than merely intended
//! - every post-claim write is fenced by the exact claim, and a lost fence makes
//!   the worker discard its prepared effect rather than retry
//! - the due index attributes are removed the moment work settles, so the index
//!   holds only outstanding work
//! - `TTL` reclaims a retired record 24 hours later and is never a fence: both
//!   the inside-window and outside-window arms are correct
//!
//! # Not this crate's job
//!
//! - what the work means, or when it should be scheduled (`aex-session-app`,
//!   `aex-operation-domain`)
//! - the queue, the pipe or the stream consumer (`regional-stream`, the workers)
//! - session, content, secret or runtime rows

pub mod application_plan;
pub mod claim;
pub mod codec;
pub mod keys;
pub mod store;

pub use application_plan::WorkApplicationCompiler;

pub use claim::WorkClaim;
pub use codec::{Payload, PayloadError, WorkRecord};
pub use keys::{DUE_SHARDS, PRIORITY_LEAD_SECONDS};
