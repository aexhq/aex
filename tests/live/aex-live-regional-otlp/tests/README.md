# `aex-live-regional-otlp` live tests

Integration tests for the `regional-otlp` deployable land in this directory, one file
per concern. They run only in the live lane, against a real deployed plane, with
the least privileged test identity.

Primary live concerns: real OTLP clients, maximum compressed and decoded batches, the `DynamoDB`/`S3` receipt and the overload gate.

Rules: no `#[ignore]`, no `if env::var(..).is_err() { return }` self-skip, no
retry-to-green. A missing prerequisite is a failure.
