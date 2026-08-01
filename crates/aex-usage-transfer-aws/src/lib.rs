//! `aex-usage-transfer-aws` owns the transfer-category `DynamoDB` and queue adapter:
//! measured and authorized transfer fact expressions, the projection frontier and its
//! queue.
//!
//! # Invariants
//!
//! - measured and authorized transfer modes stay distinct all the way to the receipt
//! - the adapter can address only `usage-transfer-authority`; sibling categories are
//!   unlinkable
//! - a fact write and its outbox row commit in one transaction
//!
//! # Not this crate's job
//!
//! - storage, compute or query tables
//! - rating policy (`aex-usage-rating`)
//! - customer `HTTP`

pub mod expressions;
pub mod outbox;
