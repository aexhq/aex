# `aex-live-regional-observation-api` live tests

Integration tests for the `regional-observation-api` deployable land in this directory, one file
per concern. They run only in the live lane, against a real deployed plane, with
the least privileged test identity.

Primary live concerns: bounded authority queries, cursor generation and expiry, gaps and exports, read-only grants.

Rules: no `#[ignore]`, no `if env::var(..).is_err() { return }` self-skip, no
retry-to-green. A missing prerequisite is a failure.
