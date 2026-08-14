//! Finite regional session API.
//!
//! The package serves session, run, operation, registry, upload,
//! content-metadata, approval, and usage routes. Session provider-key
//! plaintext admission stays isolated behind the narrow
//! [`session::provider_key::SessionProviderKeys`] adapter.

pub mod capability;
pub mod config;
pub mod frontier;
pub mod release_catalog;
pub mod session;

pub use config::Config;
