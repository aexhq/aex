# `aex-live-finance-reconcile` live tests

Integration tests for the `finance-reconcile` deployable land in this directory, one file
per concern. They run only in the live lane, against a real deployed plane, with
the least privileged test identity.

Primary live concerns: unknown payment lookup, dispute, refund and reversal, statement and outbox repair.

Rules: no `#[ignore]`, no `if env::var(..).is_err() { return }` self-skip, no
retry-to-green. A missing prerequisite is a failure.
