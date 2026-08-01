# `aex-live-usage-receipt-dispatcher` live tests

Integration tests for the `usage-receipt-dispatcher` deployable land in this directory, one file
per concern. They run only in the live lane, against a real deployed plane, with
the least privileged test identity.

Primary live concerns: queue duplication and loss, regional outage, receipt replay and frontier convergence.

Rules: no `#[ignore]`, no `if env::var(..).is_err() { return }` self-skip, no
retry-to-green. A missing prerequisite is a failure.
