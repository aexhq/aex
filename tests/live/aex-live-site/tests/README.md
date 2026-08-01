# `aex-live-site` live tests

Integration tests for the `site` deployable land in this directory, one file
per concern. They run only in the live lane, against a real deployed plane, with
the least privileged test identity.

Primary live concerns: static health, generated docs/API/SDK/CLI identity, links, search, accessibility and independent availability.

Rules: no `#[ignore]`, no `if env::var(..).is_err() { return }` self-skip, no
retry-to-green. A missing prerequisite is a failure.
