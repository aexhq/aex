# `aex-live-control-projection-worker` live tests

Integration tests for the `central-control-worker` deployable land in this directory, one file
per concern. They run only in the live lane, against a real deployed plane, with
the least privileged test identity.

Primary live concerns: FIFO duplicates and disorder, email and provider loss, regional outage, backlog and reconciliation.

Rules: no `#[ignore]`, no `if env::var(..).is_err() { return }` self-skip, no
retry-to-green. A missing prerequisite is a failure.
