//! `aex-model-catalog` owns the signed immutable model catalog: model, dialect, capability
//! and routing policy plus its content signature.
//!
//! # Invariants
//!
//! - the catalog is immutable and content-addressed; a change is a new signed digest
//! - an unsigned or mis-signed catalog is rejected at load, never partially trusted
//! - a model absent from the catalog is not routable, whatever a request asks for
//!
//! # Not this crate's job
//!
//! - calling providers (`aex-brain-provider-gateway`)
//! - prices or negotiated commercial terms
//! - tool policy (`aex-brain-tool-catalog`)

pub mod catalog;
pub mod signature;
