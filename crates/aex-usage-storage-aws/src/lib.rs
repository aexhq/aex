//! `aex-usage-storage-aws` owns the storage-category `DynamoDB` and queue adapter: storage
//! fact and outbox expressions, the projection frontier and the storage-category queue.
//!
//! # Invariants
//!
//! - the adapter can address only `usage-storage-authority`; sibling categories are
//!   unlinkable
//! - a fact write and its outbox row commit in one transaction
//! - the frontier advances only after the outbox row is durable
//!
//! # Not this crate's job
//!
//! - compute, transfer or query tables
//! - rating policy (`aex-usage-rating`)
//! - customer `HTTP`

pub mod expressions;
pub mod outbox;
