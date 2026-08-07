//! `aex-observation-query` owns the bounded public observation query language: `AST`
//! normalization, the typed field policy, exact ordering, cursor binding and coverage
//! calculation.
//!
//! # Invariants
//!
//! - every query is bounded before execution: page size, scanned range and result bytes
//! - **no `Scan` API call exists anywhere in this crate**: a scan is how a bounded
//!   query engine silently becomes an unbounded one
//! - a budget-exhausted page returns a short page **with a cursor**; only a first
//!   segment that cannot yield one item returns `telemetry_query_budget_exhausted`
//! - a cursor is bound to the generation it was issued against; a stale cursor is rejected
//! - coverage is reported explicitly so a partial answer is never presented as complete
//!
//! # Not this crate's job
//!
//! - network, credentials or provider retry
//! - admission or authority mutation (`aex-observation-app`)
//! - customer authorization

pub mod aggregate;
pub mod ast;
pub mod coverage;
pub mod cursor;
pub mod plan;

pub use aggregate::{Aggregation, Calculation, TDigest};
pub use ast::{CmpOp, FieldRef, FilterRow, MapRow, Predicate, QueryError, TextOp, resolve};
pub use coverage::{Consistency, Coverage, Snapshot};
pub use cursor::{
    CursorError, ObservationCursorBinding, ObservationResume, ResumeKey, ResumeTuple,
    SegmentPosition, SegmentResume, SegmentState, TraceRevisionMode,
};
pub use plan::{
    Access, Budget, Dimension, NormalizedQuery, PageOutcome, Plan, ScopeAxis, Spend, Walk,
    classify, plan,
};
