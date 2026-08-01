//! `aex-regional-http` owns the generated finite regional `HTTP` composition: route tables,
//! region assertion extraction, body limits and the public error mapping.
//!
//! # Invariants
//!
//! - every route is generated from the contract bundle; the route set is finite and closed
//! - a request without a valid current region assertion is rejected before any handler runs
//! - the composition links no forbidden capability: a link-time check proves it
//!
//! # Not this crate's job
//!
//! - business logic: handlers delegate to application crates
//! - central routes (`aex-central-http`)
//! - the wire types themselves (`aex-wire`)

pub mod error;
pub mod router;
