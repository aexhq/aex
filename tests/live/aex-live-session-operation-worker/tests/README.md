# `aex-live-session-operation-worker` live tests

Integration tests for the `session-operation-worker` deployable land in this directory, one file
per concern. They run only in the live lane, against a real deployed plane, with
the least privileged test identity.

Primary live concerns: every crash boundary for cursor-based export, purge and large GC, duplicate hints and scheduled due recovery.

Rules: no `#[ignore]`, no `if env::var(..).is_err() { return }` self-skip, no
retry-to-green. A missing prerequisite is a failure.
