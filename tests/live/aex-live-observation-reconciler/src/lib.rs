//! Live-test companion package for the `observation-reconciler` deployable.
//!
//! Primary live concerns: missing spool, claim expiry, retention risk and ingress closure,
//! deletion and rebuild completion.
//!
//! What this suite owes, and nothing else can answer:
//!
//! - **The duty split is a deployment fact.** Exactly one deployment may carry
//!   the `deletion.execute` duty and its `s3:DeleteObject` grant; every other
//!   duty's role must be observed to lack it against the real IAM closure.
//! - **A partial batch is reported per item.** Under a poisoned record the
//!   handler must answer with the failed identifiers and never by throwing, so
//!   the healthy remainder of the batch is not redelivered.
//! - **The ingress gate closes before retained evidence is at risk**, and its
//!   one-way recovery through `degraded` holds its hysteresis dwell against a
//!   flapping dependency.
//!
//! This package is `publish = false`, is never linked into a production artifact, uses the
//! least privileged test identity, and is selected only by the live lane. A test here must
//! fail on a missing prerequisite; it must never self-skip.
