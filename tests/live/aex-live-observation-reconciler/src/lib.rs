//! Live-test companion package for the `observation-reconciler` deployable.
//!
//! Primary live concerns: missing spool, claim expiry, retention risk and ingress closure,
//! deletion and rebuild completion.
//!
//! This package is `publish = false`, is never linked into a production artifact, uses the
//! least privileged test identity, and is selected only by the live lane. A test here must
//! fail on a missing prerequisite; it must never self-skip.
