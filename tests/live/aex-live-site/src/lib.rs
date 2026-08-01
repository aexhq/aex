//! Live-test companion package for the `site` deployable.
//!
//! Primary live concerns: static health, generated docs/API/SDK/CLI identity, links,
//! search, accessibility and independent availability.
//!
//! This package is `publish = false`, is never linked into a production artifact, uses the
//! least privileged test identity, and is selected only by the live lane. A test here must
//! fail on a missing prerequisite; it must never self-skip.
