//! Live-test companion package for the `provider-cost-reconciler` deployable.
//!
//! Primary live concerns: sanitized provider exports and invoices, duplicates, margin
//! alerts and no customer charge.
//!
//! This package is `publish = false`, is never linked into a production artifact, uses the
//! least privileged test identity, and is selected only by the live lane. A test here must
//! fail on a missing prerequisite; it must never self-skip.
