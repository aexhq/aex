//! Live-test companion package for the `observation-export-launcher` deployable.
//!
//! Primary live concerns: duplicate durable operation claims, unknown ECS launch outcome
//! and no observation-read grant.
//!
//! What this suite owes, and nothing else can answer:
//!
//! - **The launcher holds no observation read permission at all.** Against the
//!   deployed role, `dynamodb:Query` on the observation partitions and
//!   `s3:GetObject` on `observations/*` must both be denied, so a launcher
//!   defect cannot become a data path.
//! - **An unknown `RunTask` outcome is reconciled by identity.** With the ECS
//!   response deliberately lost, `ListTasks{startedBy}` must resolve it and
//!   exactly one task must ever exist for one export id.
//! - **A lost claim launches nothing**, observed with two launchers racing one
//!   admitted export.
//!
//! This package is `publish = false`, is never linked into a production artifact, uses the
//! least privileged test identity, and is selected only by the live lane. A test here must
//! fail on a missing prerequisite; it must never self-skip.
