//! Pure interval arithmetic.
//!
//! Every closing rule that turns a raw observation into a billable quantity
//! lives here, with no dependency beyond this crate's own model. That is what
//! lets the storage minute cursor, the CPU physical cap and the byte-millisecond
//! integral be property-tested with zero I/O, and it keeps `tokio`, `rustix` and
//! cgroup reading out of every worker that never probes.

pub mod cpu;
pub mod memory;
pub mod storage;

use crate::measurement::MeasurementError;
use crate::quantity::QuantityError;

pub use cpu::{CpuAllocation, reconcile_cpu};
pub use memory::byte_ms;
pub use storage::{
    StorageAccrualOutcome, StorageClose, StorageCursor, StorageOwner, StorageOwnerKind,
    StorageSource, StorageTransition, accrue_storage,
};

/// Why an interval could not be closed.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum IntervalError {
    /// A transition arrived before the cursor's charged-through instant.
    #[error("transition at {at} precedes the cursor's {charged_through} charged-through instant")]
    Backwards {
        /// The transition instant that was refused.
        at: String,
        /// How far the cursor has already charged.
        charged_through: String,
    },
    /// A transition arrived for a residence that was never opened.
    #[error("transition `{transition}` requires an open residence")]
    NoResidence {
        /// The transition that was refused.
        transition: &'static str,
    },
    /// A `Put` arrived for a residence that is already open.
    #[error("residence is already open; a byte change is a `Resize`")]
    AlreadyOpen,
    /// Any transition arrived after the residence was sealed.
    #[error("residence is sealed; a hard delete is terminal")]
    Sealed,
    /// The physically observed CPU total went backwards or was inconsistent.
    #[error("attributed {attributed_us} us cannot be reconciled against {physical_us} us")]
    Unreconcilable {
        /// Microseconds the cgroup reported.
        physical_us: u64,
        /// Microseconds the meters attributed.
        attributed_us: u128,
    },
    /// A derived quantity did not fit.
    #[error(transparent)]
    Quantity(#[from] QuantityError),
    /// The closed measurement failed its own construction rules.
    #[error(transparent)]
    Measurement(#[from] MeasurementError),
}
