//! `aex-usage-query-dynamodb` owns the read-only `usage-query-projection` adapter: the pure
//! query plan, bounded coarse and per-tuple reads, and the copied coverage vector.
//!
//! # Invariants
//!
//! - the adapter exposes no write capability at all; the type system prevents one
//! - every query is bounded by page size and scanned range
//! - the copied coverage vector is returned with every answer
//!
//! # Not this crate's job
//!
//! - fact or outbox writes (`aex-usage-*-aws`)
//! - rating policy (`aex-usage-rating`)
//! - customer `HTTP` or authorization
//! - **the cursor codec.** This crate deliberately holds no cursor module. The
//!   regional plane has exactly one signed cursor codec, in `aex-regional-http`,
//!   and its key material is loaded by the serving unit at cold start. A codec
//!   here would give this crate a key and a policy, and `tests/write_incapability.rs`
//!   would stop being a fact about its own sources. The serving unit owns the
//!   *binding* — route, principal scope, region, workspace, normalized query and
//!   the pinned generation — exactly as every regional query adapter does, and this
//!   crate hands it the plan position to carry.

pub mod expressions;
pub mod plan;
pub mod store;

pub use expressions::{
    AggregatePage, AggregateRequest, AggregateRow, CoarsePage, CoarseRequest, CoarseRow,
    CoverageRow, Generation, Grain, Granularity, MAX_PAGE_ROWS, ProjectionKey, ProjectionKeyError,
    ProjectionKeys, ProjectionReads, QueryError,
};
pub use plan::{
    Bucket, MAX_RANGE_DAYS, PlanRequest, PlannedPartition, QueryPlan, UsagePlanError, plan,
};
pub use store::{AggregatePageRows, CoarsePageRows, UsageProjectionReads, UsageQueryStore};
