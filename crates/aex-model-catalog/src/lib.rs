//! `aex-model-catalog` owns the immutable model catalog: the generated admit
//! table, model, dialect, capability and routing policy, and the
//! pre-dispatch qualification that admits a `(provider, model)` pair.
//!
//! The **provider-neutral canonical result vocabulary** lives in
//! `aex-model-vocabulary` and is re-exported through the modules below, so
//! existing `aex_model_catalog::canonical::*` paths resolve unchanged.
//!
//! # Invariants
//!
//! - the catalog is immutable and content-addressed; a change is a new digest
//! - a model absent from the catalog is not routable, whatever a request asks for
//! - an unknown field or unknown enum member anywhere in the document is a load
//!   failure, never an ignored value
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
pub mod document;
pub mod failure;
pub mod fixture;
pub mod primitives;
pub mod qualified;
pub mod receipt;
pub mod wire_pending;

pub use failure::{ProviderFailureClass, ProviderFailureKind, RedactedDetail};
pub use primitives::{
    Blake3Digest, BoundError, BoundedString, ModelSlug, ProviderRequestId, ToolCallId, ToolName,
};
pub use qualified::{CatalogError, QualifiedModel};
pub use wire_pending::CatalogRevision;
