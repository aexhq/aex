# `aex-live-observation-reconciler` live tests

Integration tests for the `observation-reconciler` deployable land in this directory, one file
per concern. They run only in the live lane, against a real deployed plane, with
the least privileged test identity.

Primary live concerns: missing spool, claim expiry, retention risk and ingress closure, deletion and rebuild completion.

Rules: no `#[ignore]`, no `if env::var(..).is_err() { return }` self-skip, no
retry-to-green. A missing prerequisite is a failure.
