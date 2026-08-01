# `aex-live-observation-export-task` live tests

Integration tests for the `observation-export-task` deployable land in this directory, one file
per concern. They run only in the live lane, against a real deployed plane, with
the least privileged test identity.

Primary live concerns: multi-GB bounded memory, multipart resume, abort and hash, task loss, cancel and delete races.

Rules: no `#[ignore]`, no `if env::var(..).is_err() { return }` self-skip, no
retry-to-green. A missing prerequisite is a failure.
