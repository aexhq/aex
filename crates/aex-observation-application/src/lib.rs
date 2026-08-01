//! `aex-observation-application` owns the observation stage, admit, abort, reconcile,
//! rebuild and delete use cases and their ports.
//!
//! # Invariants
//!
//! - staging and admission are separate steps so a crash between them is recoverable
//! - a rebuild is deterministic: the same authority contents rebuild to the same projection
//! - deletion completes or stays claimed; it never half-applies
//!
//! # Not this crate's job
//!
//! - concrete clients or `SQL` strings (`aex-observation-store-aws`)
//! - the authority invariants themselves (`aex-observation-domain`)
//! - customer `HTTP` admission (`regional-otlp`)

pub mod ports;
pub mod use_cases;

pub use ports::{
    CommitReceipt, CommitRequest, EventPage, EventPageRequest, ObservationAuthority, PortError,
    SecretManifestSource, SemanticEvent, SemanticEventSource,
};
pub use use_cases::{
    AdmissionError, AdmitBatch, GapOnFailure, SemanticAdmission, SemanticAdmissionRequest,
};
