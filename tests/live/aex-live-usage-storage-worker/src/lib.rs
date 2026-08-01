//! Live-test companion package for the `usage-storage-worker` deployable.
//!
//! Primary live concerns: duplicate and corrected storage facts, outbox and frontier,
//! central outage and category IAM isolation.
//!
//! This package is `publish = false`, is never linked into a production artifact, uses the
//! least privileged test identity, and is selected only by the live lane. A test here must
//! fail on a missing prerequisite; it must never self-skip.
