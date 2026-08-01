//! `aex-runtime-activity-dynamodb` owns the `runtime-activity` table adapter: exact Hands
//! generations, lifecycle intents and receipts, true-idle heartbeats and snapshot activity.
//!
//! # Invariants
//!
//! - every lifecycle write is conditional on the exact fence **and** revision, so
//!   a late reply from a superseded provider call loses rather than reviving a
//!   generation the system has moved past
//! - an intent and its receipt are matched by identity, never by ordering, and a
//!   receipt is immutable: a second settlement loses rather than overwriting
//!   lifecycle evidence a usage fact already references
//! - a terminal transition removes the due index attributes in the same write,
//!   so the reaper never sees a generation that is already gone
//! - the only TTL on this table is on idle probes; receipts have none
//! - a dedicated due index rather than a work item per evaluation, because the
//!   180-second true-idle timer would otherwise cost one work-item rewrite per
//!   generation per evaluation
//!
//! # Not this crate's job
//!
//! - the lifecycle and true-idle rules (`aex-runtime-control`)
//! - the compute provider (`aex-hands-control-aws`)
//! - queue delivery (`aex-runtime-control-aws`)

pub mod codec;
pub mod expressions;
pub mod keys;
pub mod store;

pub use codec::{CurrentGeneration, GenerationRow, IdleProbe, LifecycleIntent, LifecycleReceipt};
pub use store::{DueGeneration, RuntimeActivityDynamoStore, RuntimeActivityStore};
