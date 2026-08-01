# `aex-live-runtime-control-worker` live tests

Integration tests for the `runtime-control-worker` deployable land in this directory, one file
per concern. They run only in the live lane, against a real deployed plane, with
the least privileged test identity.

Primary live concerns: exact generation and fence, the busy/idle 180-second boundary, provider unknown outcomes and the eight-hour cap.

Rules: no `#[ignore]`, no `if env::var(..).is_err() { return }` self-skip, no
retry-to-green. A missing prerequisite is a failure.
