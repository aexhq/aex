# `aex-live-usage-storage-worker` live tests

Integration tests for the `usage-storage-worker` deployable land in this directory, one file
per concern. They run only in the live lane, against a real deployed plane, with
the least privileged test identity.

Primary live concerns: duplicate and corrected storage facts, outbox and frontier, central outage and category IAM isolation.

Rules: no `#[ignore]`, no `if env::var(..).is_err() { return }` self-skip, no
retry-to-green. A missing prerequisite is a failure.
