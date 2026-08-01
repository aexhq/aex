# `aex-live-observation-export-launcher` live tests

Integration tests for the `observation-export-launcher` deployable land in this directory, one file
per concern. They run only in the live lane, against a real deployed plane, with
the least privileged test identity.

Primary live concerns: duplicate durable operation claims, unknown ECS launch outcome and no observation-read grant.

Rules: no `#[ignore]`, no `if env::var(..).is_err() { return }` self-skip, no
retry-to-green. A missing prerequisite is a failure.
