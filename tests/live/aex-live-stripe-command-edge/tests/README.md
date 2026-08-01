# `aex-live-stripe-command-edge` live tests

Integration tests for the `stripe-command-edge` deployable land in this directory, one file
per concern. They run only in the live lane, against a real deployed plane, with
the least privileged test identity.

Primary live concerns: official Stripe test-mode version, idempotency, error and timeout paths; no database credential.

Rules: no `#[ignore]`, no `if env::var(..).is_err() { return }` self-skip, no
retry-to-green. A missing prerequisite is a failure.
