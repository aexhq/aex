# `aex-live-central-control-api` live tests

Integration tests for the `central-control-api` deployable land in this directory, one file
per concern. They run only in the live lane, against a real deployed plane, with
the least privileged test identity.

Primary live concerns: organization/member/workspace/key CRUD, idempotency, unknown regional effect outcome, no finance DML.

Rules: no `#[ignore]`, no `if env::var(..).is_err() { return }` self-skip, no
retry-to-green. A missing prerequisite is a failure.
