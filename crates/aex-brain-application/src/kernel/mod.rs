//! The synchronization kernel: every atomic and every lock in this stream.
//!
//! Concentrating them here is what makes Loom coverage a finite claim. Loom explores every
//! interleaving of the primitives it can see, so a lock that lives outside this module is a
//! lock nothing checks. The rule is therefore structural, not stylistic: if it
//! synchronizes, it is here.
//!
//! Everything is written against the [`sync`] shim so the same code runs under `std` in
//! production and under `loom`'s model checker in the concurrency lane.

pub mod cache;
pub mod drain;
pub mod permits;
pub mod registry;
pub mod renewer;
pub mod snapshot_cache;

/// The synchronization primitives, swapped for Loom's model-checking versions when the
/// `loom` feature is on.
///
/// A Cargo feature rather than a bare `--cfg loom`: the flag form needs a `check-cfg`
/// entry that only the shared workspace lint table can carry, and a feature reaches the
/// same swap without any stream editing a manifest it does not own.
pub(crate) mod sync {
    // `Arc` is deliberately **not** shimmed. Loom's `Arc` is not a valid `self` receiver,
    // which would force every kernel constructor into a shape driven by the test
    // configuration rather than by the design. Loom models the interleavings through the
    // primitives that actually synchronize — the mutexes and atomics below — and reference
    // counting adds no interleaving of its own.
    pub(crate) use std::sync::Arc;

    #[cfg(feature = "loom")]
    pub(crate) use loom::sync::{Mutex, atomic};

    #[cfg(not(feature = "loom"))]
    pub(crate) use std::sync::{Mutex, atomic};
}

pub use cache::{WarmCacheShard, WarmEntry};
pub use drain::{DrainGate, DrainPermit};
pub use permits::{PermitKind, PermitSet, PermitSetFull, Reservation};
pub use registry::{ActivationRegistry, Busy, RegistrySlot};
pub use renewer::{RenewalOutcome, RenewalState};
pub use snapshot_cache::{SnapshotBodyCache, SnapshotCacheError};
