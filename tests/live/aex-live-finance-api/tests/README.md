# `aex-live-finance-api` live tests

Integration tests for the `finance-api` deployable land in this directory, one file
per concern. They run only in the live lane, against a real deployed plane, with
the least privileged test identity.

Primary live concerns: balance, statement and policy reads, provider-effect prepare/finalize/unknown, no identity mutation.

Rules: no `#[ignore]`, no `if env::var(..).is_err() { return }` self-skip, no
retry-to-green. A missing prerequisite is a failure.
