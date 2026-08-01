# `aex-live-model-catalog` live tests

Integration tests for the `model-catalog` deployable land in this directory, one file
per concern. They run only in the live lane, against a real deployed plane, with
the least privileged test identity.

Primary live concerns: the real provider model qualification matrix plus disable and change probes.

Rules: no `#[ignore]`, no `if env::var(..).is_err() { return }` self-skip, no
retry-to-green. A missing prerequisite is a failure.
