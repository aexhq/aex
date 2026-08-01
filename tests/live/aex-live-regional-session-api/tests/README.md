# `aex-live-regional-session-api` live tests

Integration tests for the `regional-session-api` deployable land in this directory, one file
per concern. They run only in the live lane, against a real deployed plane, with
the least privileged test identity.

Primary live concerns: the admission transaction, auth assertion, idempotency, run terminal barrier and `DynamoDB`/`S3` roles.

Rules: no `#[ignore]`, no `if env::var(..).is_err() { return }` self-skip, no
retry-to-green. A missing prerequisite is a failure.
