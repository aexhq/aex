//! Live-test companion package for the `billing-worker` deployable.
//!
//! Primary live concerns: Stripe test-mode webhook duplicates and disorder, signature edge
//! handoff, Aurora commit loss.
//!
//! This package is `publish = false`, is never linked into a production artifact, uses the
//! least privileged test identity, and is selected only by the live lane. A test here must
//! fail on a missing prerequisite; it must never self-skip.

/// Direct-invoke identity of the deployed billing-worker Lambda.
pub const FUNCTION_ENV: &str = "AEX_LIVE_FINANCE_INGEST_FUNCTION";
/// Authenticated central public API base URL used to observe the projection.
pub const API_URL_ENV: &str = "AEX_LIVE_CENTRAL_API_URL";
/// Short-lived API credential carrying `billing:read`.
pub const API_BEARER_ENV: &str = "AEX_LIVE_BILLING_READ_BEARER";
/// Fresh attached-event request JSON for the run.
pub const ATTACHED_ENV: &str = "AEX_LIVE_PAYMENT_METHOD_ATTACHED_REQUEST";
/// A newer updated-event request JSON for the same method.
pub const UPDATED_ENV: &str = "AEX_LIVE_PAYMENT_METHOD_UPDATED_REQUEST";
/// A newer detached-event request JSON for the same method.
pub const DETACHED_ENV: &str = "AEX_LIVE_PAYMENT_METHOD_DETACHED_REQUEST";
/// A distinct event id carrying attached state older than the detach.
pub const STALE_ATTACHED_ENV: &str = "AEX_LIVE_PAYMENT_METHOD_STALE_ATTACHED_REQUEST";
/// An attached request whose provider customer has no billing owner.
pub const MISSING_OWNER_ENV: &str = "AEX_LIVE_PAYMENT_METHOD_MISSING_OWNER_REQUEST";
