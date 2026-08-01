# `aex-live-central-schema-admin` live tests

Integration tests for the `central-schema-admin` deployable land in this directory, one file
per concern. They run only in the live lane, against a real deployed plane, with
the least privileged test identity.

Primary live concerns: clean baseline, adjacent-head expand/contract, interrupted and resumed migration, grants and lock exclusion.

Rules: no `#[ignore]`, no `if env::var(..).is_err() { return }` self-skip, no
retry-to-green. A missing prerequisite is a failure.
