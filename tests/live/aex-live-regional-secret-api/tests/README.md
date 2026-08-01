# `aex-live-regional-secret-api` live tests

Integration tests for the `regional-secret-api` deployable land in this directory, one file
per concern. They run only in the live lane, against a real deployed plane, with
the least privileged test identity.

Primary live concerns: immediate encryption, rotation/revocation/rebind, plaintext and redaction scanning, ordinary-role denial.

Rules: no `#[ignore]`, no `if env::var(..).is_err() { return }` self-skip, no
retry-to-green. A missing prerequisite is a failure.
