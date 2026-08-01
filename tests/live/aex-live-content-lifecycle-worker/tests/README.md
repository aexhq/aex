# `aex-live-content-lifecycle-worker` live tests

Integration tests for the `content-lifecycle-worker` deployable land in this directory, one file
per concern. They run only in the live lane, against a real deployed plane, with
the least privileged test identity.

Primary live concerns: staged orphans, mark/sweep and root pins, conditional unversioned S3 keys, deletion denial and restored-data unwrap.

Rules: no `#[ignore]`, no `if env::var(..).is_err() { return }` self-skip, no
retry-to-green. A missing prerequisite is a failure.
