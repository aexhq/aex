//! `aex-usage-storage-aws` is the `usage-storage-authority` adapter: table expressions,
//! the row codec, the admission transaction and the frontier compare-and-set for
//! `storage.byte_min.v1` facts, plus the rating-queue producer and this
//! category's settlement receipt consumer.
//!
//! # Invariants
//!
//! - this crate can only address the storage authority; [`CATEGORY`] is the one
//!   binding, and every codec path re-checks it on write **and** on read
//! - a fact row is written once with `attribute_not_exists`, never updated and
//!   never deleted — every row here is money evidence
//! - the admission transaction touches exactly one partition, so the frontier
//!   compare-and-set is partition-local
//! - the minute cursor is the only mutable row in the table, and it is fenced
//!   by an explicit revision
//!
//! # Not this crate's job
//!
//! - compute, memory or transfer facts (`aex-usage-compute-aws`,
//!   `aex-usage-transfer-aws`); this crate depends on
//!   neither, which the isolation test asserts rather than leaving to review
//! - rating, pricing or money (`aex-usage-rating`, `aex-finance-*`)
//! - the query projection's read path (`aex-usage-query-aws`)

pub mod expressions;
pub mod outbox;

use aex_usage_domain::meter::Category;

/// The one authority this crate can address.
pub const CATEGORY: Category = Category::Storage;

/// The environment variable naming the authority table.
///
/// Exactly one table binding exists in this crate; the source-conformance test
/// asserts that, so a second one cannot be introduced quietly.
pub const TABLE_ENV: &str = "AEX_USAGE_STORAGE_TABLE";

/// The environment variable naming this category's settlement receipt queue.
pub const RECEIPT_QUEUE_ENV: &str = "AEX_USAGE_RECEIPT_STORAGE_QUEUE";

/// The environment variable naming the shared central rating queue.
pub const RATING_QUEUE_ENV: &str = "AEX_USAGE_RATING_QUEUE";

pub use expressions::{
    AdmissionTransaction, FRONTIER_CAS, StorageAuthority, StoreError, TransactItem, WRITE_ONCE,
};
