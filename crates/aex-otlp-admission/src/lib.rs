//! `aex-otlp-admission` owns the pinned `OTLP` decode and normalization boundary: protobuf
//! and `JSON` decoding, decompression bounds, canonical conversion and response mapping.
//!
//! # Invariants
//!
//! - decompressed size is bounded before allocation; a decompression bomb is rejected
//! - identity fields supplied by the customer are overwritten, not trusted
//! - an invalid record rejects the **whole** batch; there is no OTLP partial success
//! - an untrusted payload never panics, which the declared fuzz target proves
//!
//! # Not this crate's job
//!
//! - operational `OTel` exporters (`aex-platform-telemetry`)
//! - storage (`aex-observation-store-aws`)
//! - quota accounting decisions
//!
//! # The pinned protocol
//!
//! The OTLP wire format is an *input format* here, not a telemetry library. The
//! `.proto` tree is vendored and digest-pinned (see [`proto_pin`]); no
//! `opentelemetry` SDK crate is a dependency, and `tonic` is deliberately
//! absent because OTLP reaches AEX over HTTP/protobuf only.

pub mod proto;

pub mod decode;
pub mod error;
pub mod json;
pub mod limits;
pub mod memory;
pub mod normalize;
pub mod proto_pin;
pub mod redact;
pub mod response;
pub mod wire_pending;

pub use decode::{ContentCoding, DecodeRequest, DecodedBatch, OtlpEncoding, decode};
pub use error::{AdmissionCode, OtlpError, OtlpSignal, RecordPointer};
pub use limits::OtlpLimits;
pub use memory::{MemoryBudget, MemoryLease, reservation_for};
pub use normalize::{
    AuthenticatedScope, NormalizedBatch, NormalizedObservation, OverwriteReport, normalize,
};
pub use redact::{
    DigestRedactor, ManagedSecretRedactor, NoManagedSecrets, RedactionReport, SecretDigest,
    SecretDigestManifest,
};
