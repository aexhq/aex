# `aex-live-central-identity-api` live tests

Integration tests for the `central-identity-api` deployable land in this directory, one file
per concern. They run only in the live lane, against a real deployed plane, with
the least privileged test identity.

Primary live concerns: OAuth, email and device ceremonies, replay and link collision, identity database role denial.

Rules: no `#[ignore]`, no `if env::var(..).is_err() { return }` self-skip, no
retry-to-green. A missing prerequisite is a failure.
