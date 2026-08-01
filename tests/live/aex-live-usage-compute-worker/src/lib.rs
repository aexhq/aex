//! Live-test companion package for the `usage-compute-worker` deployable.
//!
//! Primary live concerns: model and runtime facts, concurrent producers, exact evidence and
//! rating receipt, category isolation.
//!
//! This package is `publish = false`, is never linked into a production artifact, uses the
//! least privileged test identity, and is selected only by the live lane. A test here must
//! fail on a missing prerequisite; it must never self-skip.
