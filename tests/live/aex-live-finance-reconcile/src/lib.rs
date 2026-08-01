//! Live-test companion package for the `finance-reconcile` deployable.
//!
//! Primary live concerns: unknown payment lookup, dispute, refund and reversal, statement
//! and outbox repair.
//!
//! This package is `publish = false`, is never linked into a production artifact, uses the
//! least privileged test identity, and is selected only by the live lane. A test here must
//! fail on a missing prerequisite; it must never self-skip.
