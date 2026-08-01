# `aex-live-brain-mux` live tests

Integration tests for the `brain-mux` deployable land in this directory, one file
per concern. They run only in the live lane, against a real deployed plane, with
the least privileged test identity.

Primary live concerns: the provider, MCP and Hands paths, swarms, long context/stream/tool waits, pressure, drain/failover and a 12/24-hour soak.

Rules: no `#[ignore]`, no `if env::var(..).is_err() { return }` self-skip, no
retry-to-green. A missing prerequisite is a failure.
