# `aex-live-hands-agent` live tests

Integration tests for the `hands-agent` deployable land in this directory, one file
per concern. They run only in the live lane, against a real deployed plane, with
the least privileged test identity.

Primary live concerns: hostile root workloads, reconnect, cancel and crash observation against a real guest.

Rules: no `#[ignore]`, no `if env::var(..).is_err() { return }` self-skip, no
retry-to-green. A missing prerequisite is a failure.
