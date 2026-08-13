//! `aex-model-vocabulary` owns the provider-neutral canonical vocabulary:
//! bounded primitives, the provider-failure kinds, the dialect classes, the
//! catalog revision identity, and the canonical result/request/receipt
//! vocabulary (D-CANON).
//!
//! It is deliberately free of catalog data: no entries, no admission, no
//! signing. `aex-model-catalog` builds its generated admit table on top of
//! this crate, and every Brain stream may depend on this crate without
//! acquiring Tokio, HTTP or AWS.
//!
//! # Purity
//!
//! No Tokio, no `HTTP`, no `AWS`, no filesystem, no clock. Every time
//! comparison takes the caller's `now`.

pub mod canonical;
pub mod dialect;
pub mod failure;
pub mod primitives;
pub mod wire_pending;

pub use dialect::DialectClass;
pub use failure::{ProviderFailureClass, ProviderFailureKind, RedactedDetail};
pub use primitives::{
    Blake3Digest, BoundError, BoundedString, ModelSlug, ProviderRequestId, ToolCallId, ToolName,
};
pub use wire_pending::CatalogRevision;
