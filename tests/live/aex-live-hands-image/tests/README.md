# `aex-live-hands-image` live tests

Integration tests for the `hands-image` deployable land in this directory, one file
per concern. They run only in the live lane, against a real deployed plane, with
the least privileged test identity.

Primary live concerns: boot on every compute shape, credential/root/network isolation, package manifest and hostile workload.

Rules: no `#[ignore]`, no `if env::var(..).is_err() { return }` self-skip, no
retry-to-green. A missing prerequisite is a failure.
