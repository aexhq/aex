# `aex-live-usage-transfer-worker` live tests

Integration tests for the `usage-transfer-worker` deployable land in this directory, one file
per concern. They run only in the live lane, against a real deployed plane, with
the least privileged test identity.

Primary live concerns: measured versus authorized transfer, snapshot read/write facts, duplicates, disorder and category isolation.

Rules: no `#[ignore]`, no `if env::var(..).is_err() { return }` self-skip, no
retry-to-green. A missing prerequisite is a failure.
