//! Live-test companion package for the `central-control-api` deployable.
//!
//! Primary live concerns: organization/member/workspace/key CRUD, idempotency, unknown
//! regional effect outcome, no finance DML.
//!
//! This package is `publish = false`, is never linked into a production artifact, uses the
//! least privileged test identity, and is selected only by the live lane. A test here must
//! fail on a missing prerequisite; it must never self-skip.
