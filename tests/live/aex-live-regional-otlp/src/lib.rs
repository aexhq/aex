//! Live-test companion package for the `regional-otlp` deployable.
//!
//! Primary live concerns: real OTLP clients, maximum compressed and decoded
//! batches, the `DynamoDB`/`S3` receipt, and the overload gate.
//!
//! What this suite owes, and nothing else can answer:
//!
//! - **The transaction envelope, measured rather than modelled.** G7 proves the
//!   staged-commit envelope from a conservative item-size model because the SDK
//!   exposes no serialized size. Replace it with a live `TransactWriteItems`
//!   measurement at the maximum 2,000-point batch.
//! - **The allocation gate under real concurrency.** At the deployed reserved
//!   concurrency, a sustained flood of maximum batches must produce explicit
//!   retryable `503`s with a `Retry-After` and **no** OOM kill, and the decode
//!   budget must return to zero afterwards.
//! - **The ingress gate closes before evidence is at risk**, and recovery is
//!   one-way through `degraded` with its hysteresis dwell.
//! - **`partial_success` is the empty message on every success path**, observed
//!   from a real OpenTelemetry SDK exporter rather than from a fixture.
//!
//! This package is `publish = false`, is never linked into a production
//! artifact, uses the least privileged test identity, and is selected only by
//! the live lane. A test here must fail on a missing prerequisite; it must never
//! self-skip.
