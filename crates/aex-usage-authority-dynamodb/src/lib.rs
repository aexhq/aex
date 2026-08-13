//! Shared, category-parametric `DynamoDB` machinery for the usage authorities.
//!
//! This crate owns no table binding, environment variable, IAM grant, or worker.
//! The compute, storage, and transfer adapters retain those authority boundaries
//! and select one [`AuthorityBinding`] at their public edge. The mechanisms here
//! deliberately remain strict and manual because every row is money evidence.

pub mod attribute;
pub mod clock;
pub mod codec;
pub mod expressions;
pub mod fault;
pub mod outbox;
pub mod projection;
pub mod queue;
pub mod store;
pub mod stream;

use aex_usage_domain::meter::Category;

/// Selects the one category an owning adapter may address.
///
/// Implementations belong in the three authority facade crates. Keeping the
/// binding there means this shared leaf cannot select a table or authority by
/// itself.
pub trait AuthorityBinding: Copy + std::fmt::Debug + Send + Sync + 'static {
    /// The category owned by the facade.
    const CATEGORY: Category;
}

#[cfg(test)]
#[derive(Debug, Clone, Copy)]
pub(crate) struct TestBinding;

#[cfg(test)]
impl AuthorityBinding for TestBinding {
    const CATEGORY: Category = Category::Compute;
}

/// The two secondary indexes every authority table declares.
pub mod gsi {
    /// Resolves a fact identity back to its workspace partition.
    pub const FACT_ID: &str = "gsi_fact_id";
    /// The sharded, enqueue-ordered view of undelivered outbox rows.
    pub const OUTBOX_DUE: &str = "gsi_outbox_due";
    /// The partition attribute `gsi_outbox_due` is keyed on.
    pub const OUTBOX_DUE_PARTITION: &str = "outDuePk";
    /// The sort attribute `gsi_outbox_due` is keyed on.
    pub const OUTBOX_DUE_SORT: &str = "outDueSk";
}
