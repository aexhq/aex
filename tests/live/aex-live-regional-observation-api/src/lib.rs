//! Live-test companion package for the `regional-observation-api` deployable.
//!
//! Primary live concerns: the bounded page walk against a real
//! `observation-authority` table, the measured secondary-index settle
//! distribution, the signed cursor across a real 24-hour boundary, and the
//! NDJSON frame stream against a real client.
//!
//! What this suite owes, and nothing else can answer:
//!
//! - **`AEX_OBS_INDEX_SETTLE_MS = 2000` is an estimate, not a measurement.**
//!   Measure the p99.99 GSI propagation distribution per region and re-pin it.
//!   Everything at or below the pinned snapshot is only complete *because* the
//!   settle window dominates propagation and admission clock skew.
//! - **`metric.aggregate_scan = 2_000_000` is an unmeasured cap** on the one
//!   capability that genuinely shrank when the projection was removed. Measure a
//!   dashboard-shaped workload against the API deadline.
//! - **The deletion fence.** A query issued before a scope's `deletionEpoch`
//!   advances must answer `410` after it, and its cursor must become
//!   `invalid_cursor`.
//! - **`503 observability_unavailable` means the authority is unreachable**, and
//!   never an empty `200`.
//!
//! This package is `publish = false`, is never linked into a production
//! artifact, uses the least privileged test identity, and is selected only by
//! the live lane. A test here must fail on a missing prerequisite; it must never
//! self-skip.
