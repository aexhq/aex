//! `aex-observation-store-aws` owns the observation `DynamoDB` and `S3` adapter: admission
//! transactions, immutable bodies, the durable spool, outbox, claims and the ingress gate.
//!
//! # Invariants
//!
//! - a receipt row and its body reference commit together or not at all
//! - the ingress gate closes before retained evidence can be lost
//! - spool entries are replayable: duplicate delivery converges on the same state
//!
//! # Not this crate's job
//!
//! - public query semantics (`aex-observation-query`)
//! - finance or usage authority
//! - export encoding (`aex-observation-export`)

pub mod expressions;
pub mod spool;
pub mod store;
