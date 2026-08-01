//! `aex-model-catalog` owns the signed immutable model catalog: model, dialect, capability
//! and routing policy plus its content signature. It also owns the
//! **provider-neutral canonical result vocabulary** (`canonical`, D-CANON),
//! because that vocabulary must be visible to both the pure Brain domain and
//! the provider adapter, and this crate already sits below both.
//!
//! # Invariants
//!
//! - the catalog is immutable and content-addressed; a change is a new signed digest
//! - an unsigned or mis-signed catalog is rejected at load, never partially trusted
//! - a model absent from the catalog is not routable, whatever a request asks for
//! - an unknown field or unknown enum member anywhere in the document is a load
//!   failure, never an ignored value
//! - an `Active` entry without passing conformance evidence cannot be loaded at
//!   all, so "not admissible until proved" is a document invariant rather than a
//!   runtime check
//!
//! # Purity
//!
//! No Tokio, no `HTTP`, no `AWS`, no filesystem, no clock. Every time comparison
//! takes the caller's `now`.
//!
//! # Not this crate's job
//!
//! - calling providers (`aex-brain-provider-gateway`)
//! - prices or negotiated commercial terms; model usage is a zero-dollar `BYOK`
//!   observability fact
//! - tool policy (`aex-brain-tool-catalog`)
//! - credential storage or resolution (`aex-secret-domain` and its adapters)

pub mod canonical;
pub mod catalog;
pub mod document;
pub mod failure;
pub mod fixture;
pub mod primitives;
pub mod qualified;
pub mod receipt;
pub mod signature;
pub mod wire_pending;

pub use catalog::{Catalog, CatalogHead, CatalogLoadError};
pub use failure::{ProviderFailureClass, ProviderFailureKind};
pub use primitives::{
    Blake3Digest, BoundError, BoundedString, ModelSlug, ProviderRequestId, ToolCallId, ToolName,
};
pub use qualified::{CatalogError, QualifiedModel};
pub use wire_pending::CatalogRevision;
