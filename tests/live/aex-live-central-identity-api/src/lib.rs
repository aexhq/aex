//! Live-test companion package for the `central-identity-api` deployable.
//!
//! Primary live concerns: OAuth, email and device ceremonies, replay and link collision,
//! identity database role denial.
//!
//! This package is `publish = false`, is never linked into a production artifact, uses the
//! least privileged test identity, and is selected only by the live lane. A test here must
//! fail on a missing prerequisite; it must never self-skip.
