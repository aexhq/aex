//! Live-test companion package for the `central-authz` deployable.
//!
//! Primary live concerns: key/token/assertion binding, 30-second expiry, epoch revocation,
//! KMS rotation and a 100/500 admission spike.
//!
//! This package is `publish = false`, is never linked into a production artifact, uses the
//! least privileged test identity, and is selected only by the live lane. A test here must
//! fail on a missing prerequisite; it must never self-skip.
