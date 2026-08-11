//! `aex-observation-store-dynamodb` owns the observation `DynamoDB` and `S3` adapter: admission
//! transactions, immutable bodies, the durable spool, outbox, claims and the ingress gate.
//!
//! # Invariants
//!
//! - a receipt row and its body reference commit together or not at all
//! - the ingress gate closes before retained evidence can be lost
//! - spool entries are replayable: duplicate delivery converges on the same state
//! - **no `Scan` exists anywhere in this stream**, asserted by a link-graph test:
//!   a scan is how a bounded query engine silently becomes an unbounded one
//!
//! # Not this crate's job
//!
//! - public query semantics (`aex-observation-query`)
//! - finance or usage authority
//! - export encoding (`aex-observation-export`)
//!
//! # The removal, restated
//!
//! There is no `ClickHouse` projection and no `Kinesis` rail. This table is both
//! the authority and the query engine, and its `KEYS_ONLY` stream is the only
//! wake `regional-stream` consumes.

pub mod composition;
pub mod export_pair;
pub mod expressions;
pub mod gap;
pub mod gap_hint;
pub mod health;
pub mod segments;
pub mod spool;
pub mod store;

pub use composition::{Capability, CapabilityViolation, Role, assert_grant};
pub use export_pair::{
    ExportAdmissionCommit, ExportAdmissionOutcome, ExportPairError, ExportPairStore, ExportPublish,
    ExportSettlement, ExportStart,
};
pub use expressions::{DENSE_INDEX_PROJECTION, ExpressionBuilder, Index, is_safe_expression};
pub use gap::{GAP_ITEM_TYPE, GapCodecError, GapStore, GapStoreError, append_action};
pub use gap_hint::{GAP_HINT_ITEM_TYPE, GapAppendCount, hint_update, hint_update_action};
pub use health::{HEALTHZ, Probe, READYZ, Readiness, readiness};
pub use segments::{Segment, SegmentDirectory};
pub use spool::{GateEvidence, GateState, Pending, SpoolChunk};
pub use store::{
    AdmissionPlan, PageSpan, StagedRecord, StoreError, TransactionEnvelope, pack_pages,
};

/// The `DynamoDB` Streams wake contract `regional-stream` consumes.
///
/// A wake is a hint and carries no payload: `StreamViewType = KEYS_ONLY` makes
/// RS-06 structural, because there are no bytes to emit even by mistake. Frame
/// bytes always come from a strongly consistent base-table range read, so a
/// missed, duplicated or reordered wake changes latency only.
pub mod keys {
    pub use aex_observation_domain::keys::{BucketHour, ObservationWakeKey, ScopeKey};

    /// Classifies one `observation-authority` stream record from its partition
    /// key alone.
    ///
    /// Returns `Some` for exactly the observation revision family and `None` for
    /// every other item family. That is the whole filter; `regional-stream`
    /// needs no schema knowledge.
    #[must_use]
    pub fn parse_observation_pk(pk: &str) -> Option<ObservationWakeKey> {
        aex_observation_domain::keys::parse_observation_pk(pk)
    }

    /// The stream view type the table declares.
    pub const STREAM_VIEW_TYPE: &str = "KEYS_ONLY";
}
