//! Durable regional capacity-limit authority.
//!
//! The canonical default document is compiled into this crate, but it is not
//! an admission fallback. [`CapacityStore`] materializes it into one durable
//! workspace authority record and atomically publishes the complete serving
//! projection. All readers fail closed until that transaction exists.

pub mod defaults;
pub mod model;
pub mod store;

pub use defaults::{CapacityDefaults, DefaultsError, canonical_defaults};
pub use model::{
    CapacityCommand, CapacityError, CapacityState, OverrideChange, plan_capacity_change,
};
pub use store::{
    ALL_WORKSPACES_INDEX, ALL_WORKSPACES_PARTITION, AppliedCapacity, CapacityStore,
    CapacityStoreError,
};
