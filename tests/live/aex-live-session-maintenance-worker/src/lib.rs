//! Live-test companion package for the `session-maintenance-worker` deployable.
//!
//! Primary live concerns: every crash boundary for cursor-based export, purge and large GC,
//! duplicate hints and scheduled due recovery.
//!
//! This package is `publish = false`, is never linked into a production artifact, uses the
//! least privileged test identity, and is selected only by the live lane. A test here must
//! fail on a missing prerequisite; it must never self-skip.
