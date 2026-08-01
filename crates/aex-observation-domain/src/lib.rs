//! `aex-observation-domain` owns the pure customer-observation authority model:
//! canonicalization, revision and order, series claims, receipts, gaps, frontiers and
//! deletion.
//!
//! # Invariants
//!
//! - a batch is hash-bound: its recorded digest covers exactly the admitted bytes
//! - a frontier advances only over a contiguous verified range
//! - a gap stays explicit until it is filled or explicitly retired; it is never skipped
//!
//! # Not this crate's job
//!
//! - `AWS`, `HTTP` or any `OTel` `SDK`
//! - the public query language (`aex-observation-query`)
//! - internal operational telemetry (`aex-platform-telemetry`)
//!
//! # The removal, restated
//!
//! There is no `ClickHouse` projection and no `Kinesis` rail. `DynamoDB` plus `S3`
//! are the authority *and* the query engine, so nothing in this crate models a
//! projection generation, a materialization lag or a replay horizon.

pub mod batch;
pub mod canonical;
pub mod frontier;
pub mod gap;
pub mod keys;
pub mod limits;
pub mod order;
pub mod series;
pub mod signal;

pub use batch::{ReceiptState, ReceiptTransition, TransitionError};
pub use canonical::{BatchBinding, CanonicalValue, batch_intent_digest, canonical_bytes};
pub use frontier::{
    AcceptedRange, DeletionState, DeletionTransitionError, Frontier, FrontierError, ScopeDeletion,
};
pub use gap::{GapError, GapLedger, GapRevision, GapState, OrdinalRange, TimeWindow};
pub use keys::{BucketHour, ObservationWakeKey, ScopeKey};
pub use order::{Direction, OrderBy, OrderTuple};
pub use series::{ClaimOutcome, SeriesClaims, SeriesError, SeriesHash};
pub use signal::{Signal, SignalSet};
