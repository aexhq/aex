# `aex-live-usage-compute-worker` live tests

Integration tests for the `usage-compute-worker` deployable land in this directory, one file
per concern. They run only in the live lane, against a real deployed plane, with
the least privileged test identity.

Primary live concerns: model and runtime facts, concurrent producers, exact evidence and rating receipt, category isolation.

Rules: no `#[ignore]`, no `if env::var(..).is_err() { return }` self-skip, no
retry-to-green. A missing prerequisite is a failure.
