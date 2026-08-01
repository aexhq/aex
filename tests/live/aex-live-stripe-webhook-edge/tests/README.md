# `aex-live-stripe-webhook-edge` live tests

Integration tests for the `stripe-webhook-edge` deployable land in this directory, one file
per concern. They run only in the live lane, against a real deployed plane, with
the least privileged test identity.

Primary live concerns: raw-body signature and version corpus, malformed events, durable handoff before 2xx.

Rules: no `#[ignore]`, no `if env::var(..).is_err() { return }` self-skip, no
retry-to-green. A missing prerequisite is a failure.
