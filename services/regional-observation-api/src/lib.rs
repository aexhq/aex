//! Shared production observation read and streaming service.
//!
//! The Lambda composition serves finite observation queries and lifecycle
//! routes. `regional-stream` serves the long-lived NDJSON subset. Both use this
//! one reader, query normalizer, cursor codec and generated server
//! implementation so replay and query semantics cannot drift by deployable.

pub mod api;
pub mod config;
pub mod counters;
pub mod frontier;
pub mod gap_watch;
pub mod ndjson;
pub mod query;
pub mod reader;
pub mod wake;

pub use api::{ObservationRequest, ObservationService};
pub use reader::ObservationReader;
