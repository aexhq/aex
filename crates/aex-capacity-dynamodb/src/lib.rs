//! Durable regional capacity-limit authority.
//!
//! The canonical default document is compiled into this crate. [`CapacityStore`]
//! can materialize it into one durable workspace authority record and
//! atomically publish the complete capacity projection, but the session MVP has
//! no capacity-controller deployable and does not use that projection for
//! request admission. Its request ceilings come from validated deployable
//! configuration.

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
