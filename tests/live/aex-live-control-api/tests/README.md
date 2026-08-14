# `aex-live-control-api` live tests

Integration tests for the `central-api` deployable land in this directory, one file
per concern. They run only in the live lane, against a real deployed plane, with
the least privileged test identity.

Primary live concerns: in-process credential verification behind the ALB (a forged
secret against a real row is refused; an unreachable authority answers `503`, never
`401`), a caller-supplied `aex.*` context map gaining nothing, the merged
control/auth/billing surface answering from one listener, and readiness going `503`
before the listener stops on drain.

Rules: no `#[ignore]`, no `if env::var(..).is_err() { return }` self-skip, no
retry-to-green. A missing prerequisite is a failure.
