//! Live-test companion package for the `central-control-worker` deployable.
//!
//! Primary live concerns: asynchronous Lambda retry and failure delivery, email and provider
//! loss, regional outage, backlog and reconciliation.
//!
//! This package is `publish = false`, is never linked into a production artifact, uses the
//! least privileged test identity, and is selected only by the live lane. A test here must
//! fail on a missing prerequisite; it must never self-skip.
