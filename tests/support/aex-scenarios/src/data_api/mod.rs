//! The Aurora Data `API` seam, spoken to a container instead of to AWS.
//!
//! Split three ways so the two halves that are pure data can be proved without
//! an engine at all, and only the connection handling needs one:
//!
//! - [`render`] turns a Data `API` statement into a `PostgreSQL` one;
//! - [`literal`] turns the engine's own row rendering back into Data `API`
//!   fields;
//! - [`transport`] holds the pool and the open transactions, and is the only
//!   part that needs a running database.

pub mod literal;
pub mod render;
#[cfg(feature = "integration-engines")]
pub mod transport;
