# `aex-live-finance-ingest` live tests

Integration tests for the `finance-ingest` deployable land in this directory, one file
per concern. They run only in the live lane, against a real deployed plane, with
the least privileged test identity.

Primary live concerns: Stripe test-mode webhook duplicates and disorder, signature edge handoff, Aurora commit loss.

Rules: no `#[ignore]`, no `if env::var(..).is_err() { return }` self-skip, no
retry-to-green. A missing prerequisite is a failure.
