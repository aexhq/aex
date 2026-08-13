//! Finite regional session API.
//!
//! The package serves session, run, operation, registry, upload,
//! content-metadata, approval, secret-metadata, and usage routes. Provider
//! credential plaintext admission stays isolated behind the narrow
//! [`session::secret_registration::ProviderCredentialRegistration`] port.

pub mod capability;
pub mod config;
pub mod frontier;
pub mod release_catalog;
pub mod session;

pub use config::Config;
