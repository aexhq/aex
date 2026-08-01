//! Live-test companion package for the `regional-otlp` deployable.
//!
//! Primary live concerns: real OTLP clients, maximum compressed and decoded batches, the
//! `DynamoDB`/`S3` receipt and the overload gate.
//!
//! This package is `publish = false`, is never linked into a production artifact, uses the
//! least privileged test identity, and is selected only by the live lane. A test here must
//! fail on a missing prerequisite; it must never self-skip.
