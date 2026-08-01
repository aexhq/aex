//! Live-test companion package for the `dashboard` deployable.
//!
//! Primary live concerns: staged build identity, OAuth/session/CSRF/authz negatives,
//! generated clients and no database or storage IAM.
//!
//! This package is `publish = false`, is never linked into a production artifact, uses the
//! least privileged test identity, and is selected only by the live lane. A test here must
//! fail on a missing prerequisite; it must never self-skip.
