//! Live-test companion package for the `finance-api` deployable.
//!
//! Primary live concerns: balance, statement and policy reads, provider-effect
//! prepare/finalize/unknown, no identity mutation.
//!
//! This package is `publish = false`, is never linked into a production artifact, uses the
//! least privileged test identity, and is selected only by the live lane. A test here must
//! fail on a missing prerequisite; it must never self-skip.
