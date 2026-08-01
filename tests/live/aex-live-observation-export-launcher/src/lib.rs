//! Live-test companion package for the `observation-export-launcher` deployable.
//!
//! Primary live concerns: duplicate durable operation claims, unknown ECS launch outcome
//! and no observation-read grant.
//!
//! This package is `publish = false`, is never linked into a production artifact, uses the
//! least privileged test identity, and is selected only by the live lane. A test here must
//! fail on a missing prerequisite; it must never self-skip.
