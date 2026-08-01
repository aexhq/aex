//! Live-test companion package for the `stripe-command-edge` deployable.
//!
//! Primary live concerns: official Stripe test-mode version, idempotency, error and timeout
//! paths; no database credential.
//!
//! This package is `publish = false`, is never linked into a production artifact, uses the
//! least privileged test identity, and is selected only by the live lane. A test here must
//! fail on a missing prerequisite; it must never self-skip.
