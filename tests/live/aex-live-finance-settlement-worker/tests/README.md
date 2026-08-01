# `aex-live-finance-settlement-worker` live tests

Integration tests for the `finance-settlement-worker` deployable land in this directory, one file
per concern. They run only in the live lane, against a real deployed plane, with
the least privileged test identity.

Primary live concerns: concurrent inbox, rating, reservation and settlement, balanced postings and central outage catch-up.

Rules: no `#[ignore]`, no `if env::var(..).is_err() { return }` self-skip, no
retry-to-green. A missing prerequisite is a failure.
