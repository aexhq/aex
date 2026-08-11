//! The finite half: session, run, operation, registry, upload,
//! content-metadata, approval, secret-metadata and usage routes.
//!
//! Everything reachable from here is unary and bounded. This is also the only
//! half of the process that holds a write handle, and [`stores::Stores`] is the
//! complete statement of what that handle may touch.

pub mod admission;
pub mod app_ports;
pub mod create_readiness;
pub mod create_route;
pub mod handlers;
pub mod live_composition;
pub mod live_files;
pub mod live_transfer;
pub mod operation_worker;
pub mod registry;
pub mod registry_download;
pub mod routes;
pub mod stores;
pub mod uploads;
pub mod usage;
pub mod wire_pending;

pub use handlers::{Dispatcher, Routes, Shared};
pub use stores::{ObjectBinding, Stores};
