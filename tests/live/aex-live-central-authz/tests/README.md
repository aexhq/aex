# `aex-live-central-authz` live tests

Integration tests for the `central-authz` deployable land in this directory, one file
per concern. They run only in the live lane, against a real deployed plane, with
the least privileged test identity.

Primary live concerns: key/token/assertion binding, 30-second expiry, epoch revocation, KMS rotation and a 100/500 admission spike.

Rules: no `#[ignore]`, no `if env::var(..).is_err() { return }` self-skip, no
retry-to-green. A missing prerequisite is a failure.
