//! Live-test companion package for the `central-schema-admin` deployable.
//!
//! Primary live concerns: clean baseline, adjacent-head expand/contract, interrupted and
//! resumed migration, grants and lock exclusion.
//!
//! This package is `publish = false`, is never linked into a production artifact, uses the
//! least privileged test identity, and is selected only by the live lane. A test here must
//! fail on a missing prerequisite; it must never self-skip.
