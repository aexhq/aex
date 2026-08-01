//! Live-test companion package for the `finance-ingest` deployable.
//!
//! Primary live concerns: Stripe test-mode webhook duplicates and disorder, signature edge
//! handoff, Aurora commit loss.
//!
//! This package is `publish = false`, is never linked into a production artifact, uses the
//! least privileged test identity, and is selected only by the live lane. A test here must
//! fail on a missing prerequisite; it must never self-skip.
