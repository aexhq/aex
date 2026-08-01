# `aex-live-dashboard` live tests

Integration tests for the `dashboard` deployable land in this directory, one file
per concern. They run only in the live lane, against a real deployed plane, with
the least privileged test identity.

Primary live concerns: staged build identity, OAuth/session/CSRF/authz negatives, generated clients and no database or storage IAM.

Rules: no `#[ignore]`, no `if env::var(..).is_err() { return }` self-skip, no
retry-to-green. A missing prerequisite is a failure.
