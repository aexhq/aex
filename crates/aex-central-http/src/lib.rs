//! `aex-central-http` owns the generated central `HTTP` composition: route tables, request
//! limits, authentication extraction and the public error mapping.
//!
//! # Invariants
//!
//! - every route is generated from the contract bundle; a hand-added route is a build failure
//! - a body larger than the declared limit is rejected before it is buffered
//! - an internal error never leaks a database or vendor message to the public wire
//!
//! # Not this crate's job
//!
//! - business logic: handlers delegate to application crates
//! - regional routes (`aex-regional-http`)
//! - the wire types themselves (`aex-wire`)

pub mod error;
pub mod router;
