//! Live-test companion package for the `session-stream-api` deployable.
//!
//! Primary live concerns: the admission transaction, auth assertion, idempotency, run
//! terminal barrier and `DynamoDB`/`S3` roles.
//!
//! This package is `publish = false`, is never linked into a production artifact, uses the
//! least privileged test identity, and is selected only by the live lane. A test here must
//! fail on a missing prerequisite; it must never self-skip.
