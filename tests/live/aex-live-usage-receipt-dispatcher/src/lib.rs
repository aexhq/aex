//! Live-test companion package for the `usage-receipt-dispatcher` deployable.
//!
//! Primary live concerns: queue duplication and loss, regional outage, receipt replay and
//! frontier convergence.
//!
//! This package is `publish = false`, is never linked into a production artifact, uses the
//! least privileged test identity, and is selected only by the live lane. A test here must
//! fail on a missing prerequisite; it must never self-skip.
