//! `aex-usage-compute-aws` owns the compute-category `DynamoDB` and queue adapter: compute,
//! memory, model and invocation fact expressions, the projection frontier and its queue.
//!
//! # Invariants
//!
//! - the adapter can address only `usage-compute-authority`; sibling categories are
//!   unlinkable
//! - compute and memory are discriminated facts inside one authority, not two tables
//! - a fact write and its outbox row commit in one transaction
//!
//! # Not this crate's job
//!
//! - storage, transfer or query tables
//! - rating policy (`aex-usage-rating`)
//! - customer `HTTP`

pub mod expressions;
pub mod outbox;
