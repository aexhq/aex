//! `aex-observation-export` owns the deterministic streaming export engine: format
//! encoders, `NDJSON` and Parquet streaming state, the multipart manifest and hash
//! invariants.
//!
//! # Invariants
//!
//! - memory use is bounded by the configured buffer, independent of export size
//! - the manifest hash covers exactly the emitted bytes
//! - a checkpoint is sufficient to resume: no in-memory-only state is required
//!
//! # Not this crate's job
//!
//! - launching tasks (`observation-export-launcher`)
//! - public authorization
//! - reading the authority (`aex-observation-store-aws`)

pub mod checkpoint;
pub mod encoder;
pub mod manifest;

pub use checkpoint::{ExportCheckpoint, PartRecord, Publication, PublishFence, ResumeError};
pub use encoder::{EncodeError, Format, GzipPins, MemberEncoder, NdjsonEncoder, ZipPins};
pub use manifest::{Completeness, ExportManifest, ExportMember, ManifestError};
