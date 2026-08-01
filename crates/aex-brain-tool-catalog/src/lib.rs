//! `aex-brain-tool-catalog` owns the signed immutable tool catalog: tool schema, route,
//! effect class, recovery, approval requirement and result bounds.
//!
//! # Invariants
//!
//! - the catalog is immutable and content-addressed; a change is a new signed digest
//! - every tool declares its effect class and result bound; an unbounded tool is not
//!   representable
//! - an approval-requiring tool cannot be routed without a recorded approval
//!
//! # Not this crate's job
//!
//! - executing tools (`aex-brain-managed-web`, `aex-brain-mcp`, `aex-brain-hands`)
//! - model policy (`aex-model-catalog`)
//! - session or approval storage

pub mod catalog;
pub mod signature;
