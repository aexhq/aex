# `aex-live-session-api` live tests

Integration tests for the `session-api` release unit land in this directory, one file
per concern. They run only in the live lane, against a real deployed plane, with
the least privileged test identity.

Primary live concerns: the admission transaction, auth assertion, idempotency, run terminal barrier and `DynamoDB`/`S3` roles.

The `e2e` target is the first release-runnable slice. It proves that the exact
deployed regional API rejects an anonymous registry inventory request, admits
the protected API key, returns the closed inventory shape, and emits
zero-residue/zero-spend hygiene evidence. It does not claim session
creation, terminal processing, or registry mutation/lifecycle coverage.

Rules: no `#[ignore]`, no `if env::var(..).is_err() { return }` self-skip, no
retry-to-green. A missing prerequisite is a failure.
