//! Live-test companion package for the `usage-transfer-worker` deployable.
//!
//! Primary live concerns: measured versus authorized transfer, snapshot read/write facts,
//! duplicates, disorder and category isolation.
//!
//! This package is `publish = false`, is never linked into a production artifact, uses the
//! least privileged test identity, and is selected only by the live lane. A test here must
//! fail on a missing prerequisite; it must never self-skip.
