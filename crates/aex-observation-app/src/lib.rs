//! `aex-observation-app` owns the observation stage, admit, abort, reconcile,
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
//! - concrete clients or `SQL` strings (`aex-observation-store-dynamodb`)
//! - the authority invariants themselves (`aex-observation-domain`)
//! - customer `HTTP` admission (`regional-otlp`)

pub mod export;
pub mod ports;
pub mod use_cases;

pub use export::{
    EXPORT_RETENTION_MILLIS, ExportAdmission, ExportPlan, ExportPlanError, ExportRow, ExportScope,
    derive_export_id,
};
pub use ports::{
    CommitReceipt, CommitRequest, EventPage, EventPageRequest, GapSink, ObservationAuthority,
    PortError, SemanticEvent, SemanticEventSource,
};
pub use use_cases::{
    AdmissionError, AdmitBatch, GapOnFailure, SemanticAdmission, SemanticAdmissionRequest,
};
