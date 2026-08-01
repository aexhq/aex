# `aex-live-regional-stream` live tests

Integration tests for the `regional-stream` deployable land in this directory, one file
per concern. They run only in the live lane, against a real deployed plane, with
the least privileged test identity.

Primary live concerns: 100+ sockets, slow and disconnected clients, durable cursor reconnect, task drain and replay lag.

Rules: no `#[ignore]`, no `if env::var(..).is_err() { return }` self-skip, no
retry-to-green. A missing prerequisite is a failure.
