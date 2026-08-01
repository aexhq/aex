//! `aex-usage-transfer-aws` is the `usage-transfer-authority` adapter: table expressions,
//! the row codec, the admission transaction and the frontier compare-and-set for
//! `data_transfer.egress_byte.v1` facts, plus the rating-queue producer and this
//! category's settlement receipt consumer.
//!
//! # Invariants
//!
//! - this crate can only address the transfer authority; [`CATEGORY`] is the one
//!   binding, and every codec path re-checks it on write **and** on read
//! - a fact row is written once with `attribute_not_exists`, never updated and
//!   never deleted — every row here is money evidence
//! - the admission transaction touches exactly one partition, so the frontier
//!   compare-and-set is partition-local
//! - counter-mode and receipt-mode evidence stay distinct and are never merged
//! - the crossing claim is the one-crossing-one-fact fence: a second adapter
//!   emitting the same physical crossing fails the transaction
//!
//! # Not this crate's job
//!
//! - storage, compute or memory facts (`aex-usage-storage-aws`,
//!   `aex-usage-compute-aws`); this crate depends on
//!   neither, which the isolation test asserts rather than leaving to review
//! - rating, pricing or money (`aex-usage-rating`, `aex-finance-*`)
//! - the query projection's read path (`aex-usage-query-aws`)

pub mod expressions;
pub mod outbox;

use aex_usage_domain::meter::Category;

/// The one authority this crate can address.
pub const CATEGORY: Category = Category::Transfer;

/// The environment variable naming the authority table.
///
/// Exactly one table binding exists in this crate; the source-conformance test
/// asserts that, so a second one cannot be introduced quietly.
pub const TABLE_ENV: &str = "AEX_USAGE_TRANSFER_TABLE";

/// The environment variable naming this category's settlement receipt queue.
pub const RECEIPT_QUEUE_ENV: &str = "AEX_USAGE_RECEIPT_TRANSFER_QUEUE";

/// The environment variable naming the shared central rating queue.
pub const RATING_QUEUE_ENV: &str = "AEX_USAGE_RATING_QUEUE";

pub use expressions::{
    AdmissionTransaction, FRONTIER_CAS, StoreError, TransactItem, TransferAuthority, WRITE_ONCE,
};
