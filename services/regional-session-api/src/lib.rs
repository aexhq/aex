//! Finite regional session API composition policy.

pub mod admission;
pub mod config;
pub mod handlers;
pub mod routes;
pub mod session_projection;
pub mod stores;
pub mod wire_pending;

pub use config::Config;
pub use handlers::{Routes, Shared};
pub use stores::{ObjectBinding, Stores};
