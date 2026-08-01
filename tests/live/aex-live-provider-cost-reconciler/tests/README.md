# `aex-live-provider-cost-reconciler` live tests

Integration tests for the `provider-cost-reconciler` deployable land in this directory, one file
per concern. They run only in the live lane, against a real deployed plane, with
the least privileged test identity.

Primary live concerns: sanitized provider exports and invoices, duplicates, margin alerts and no customer charge.

Rules: no `#[ignore]`, no `if env::var(..).is_err() { return }` self-skip, no
retry-to-green. A missing prerequisite is a failure.
