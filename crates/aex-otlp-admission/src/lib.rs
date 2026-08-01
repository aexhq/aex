//! `aex-otlp-admission` owns the pinned `OTLP` decode and normalization boundary: protobuf
//! and `JSON` decoding, decompression bounds, canonical conversion and response mapping.
//!
//! # Invariants
//!
//! - decompressed size is bounded before allocation; a decompression bomb is rejected
//! - identity fields supplied by the customer are overwritten, not trusted
//! - an invalid payload maps to the standard `OTLP` partial-success response, never to a
//!   panic
//!
//! # Not this crate's job
//!
//! - operational `OTel` exporters (`aex-platform-telemetry`)
//! - storage (`aex-observation-store-aws`)
//! - quota accounting decisions

pub mod decode;
pub mod normalize;
