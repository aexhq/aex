//! Live-test companion package for the `regional-secret-key-admin` deployable.
//!
//! Primary live concerns: hierarchical key create, rotate and replay, KMS grants and
//! application-role administration denial.
//!
//! This package is `publish = false`, is never linked into a production artifact, uses the
//! least privileged test identity, and is selected only by the live lane. A test here must
//! fail on a missing prerequisite; it must never self-skip.
