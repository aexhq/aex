# `aex-live-regional-secret-key-admin` live tests

Integration tests for the `regional-secret-key-admin` deployable land in this directory, one file
per concern. They run only in the live lane, against a real deployed plane, with
the least privileged test identity.

Primary live concerns: hierarchical key create, rotate and replay, KMS grants and application-role administration denial.

Rules: no `#[ignore]`, no `if env::var(..).is_err() { return }` self-skip, no
retry-to-green. A missing prerequisite is a failure.
