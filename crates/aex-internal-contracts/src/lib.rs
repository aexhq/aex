//! `aex-internal-contracts` owns the envelopes that cross a boundary between two
//! independently deployed AEX processes and never appear on the public wire.
//!
//! Every top-level envelope carries a `schema_version`, so a mixed-version plane
//! during a staged deploy is a typed condition rather than undefined behaviour.
//!
//! # Invariants
//!
//! - no type here appears in a public HTTP response, asserted by
//!   `tests/boundaries.rs`;
//! - every decoder rejects unknown members;
//! - money is integer micro-USD; the crate contains no `f32` and no `f64`, and a
//!   source-scanning test proves it.
//!
//! # Not this crate's job
//!
//! - public customer-facing wire types (`aex-wire`);
//! - queue, stream or database transport;
//! - any authority decision about the values it carries.

pub mod assertion;
pub mod catalog;
pub mod control;
pub mod journal;
pub mod money;
pub mod observation;
pub mod outbox;
pub mod release;
pub mod usage;
pub mod wake;

use serde::{Deserialize, Serialize};

/// The version discriminator every internal envelope carries.
///
/// It is an explicit field rather than an implicit assumption because a staged
/// deploy always has two versions live at once; the only question is whether the
/// receiver notices.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SchemaVersion(pub u32);

impl SchemaVersion {
    /// The one prelaunch version.
    pub const V1: Self = Self(1);
}

impl Default for SchemaVersion {
    fn default() -> Self {
        Self::V1
    }
}

/// A monotonic revision counter, used for revocation, key and account epochs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Epoch(pub u64);

/// A FIFO ordering key.
///
/// Ordered settlement is a correctness property, so the group is always the
/// account the money belongs to and never anything finer.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct FifoGroup(pub String);

/// A producer-derived deduplication key.
///
/// A hint that arrives twice must not become two units of work.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct DedupeKey(pub String);

/// The opaque identity of the rate book a fact was priced against.
///
/// No AEX price enters this crate: the value is a version, not an amount.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct PricingVersion(pub String);
