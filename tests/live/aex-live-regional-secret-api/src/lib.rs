//! Live-test companion package for the `regional-secret-api` deployable.
//!
//! Primary live concerns: immediate encryption, rotation/revocation/rebind, plaintext and
//! redaction scanning, ordinary-role denial.
//!
//! This package is `publish = false`, is never linked into a production artifact, uses the
//! least privileged test identity, and is selected only by the live lane. A test here must
//! fail on a missing prerequisite; it must never self-skip.
