//! `aex-observation-query` owns the bounded public observation query language: `AST`
//! normalization, the typed field policy, exact ordering, cursor binding and coverage
//! calculation.
//!
//! # Invariants
//!
//! - every query is bounded before execution: page size, scanned range and result bytes
//! - a cursor is bound to the generation it was issued against; a stale cursor is rejected
//! - coverage is reported explicitly so a partial answer is never presented as complete
//!
//! # Not this crate's job
//!
//! - network, credentials or provider retry
//! - admission or authority mutation (`aex-observation-application`)
//! - customer authorization

pub mod ast;
pub mod coverage;
pub mod cursor;
