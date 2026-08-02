//! `aex-hands-control-aws` owns the trusted Hands provider control adapter: launch, probe,
//! suspend, resume, terminate and snapshot with explicit ambiguous-outcome handling.
//!
//! # Invariants
//!
//! - every control call carries the exact generation it intends to act on
//! - an ambiguous provider outcome is recorded for reconciliation, never retried blindly
//! - a terminate is idempotent: repeating it on a gone runtime is success
//!
//! # Not this crate's job
//!
//! - the lifecycle and true-idle rules (`aex-runtime-control`)
//! - activity table rows (`aex-runtime-activity-dynamodb`)
//! - anything inside the guest

pub mod aws;
pub mod lifecycle;
pub mod provider;

pub use aws::AwsMicrovmControl;
pub use lifecycle::{
    AwaitVerdict, Fault, LAUNCH_AWAIT_MS, LAUNCH_POLL_MS, ProviderAnswer, RESUME_AWAIT_MS,
    SUSPEND_AWAIT_MS, SUSPEND_POLL_MS, await_step, classify, poll_interval_ms,
};
pub use provider::{
    AGENT_PORT, ALL_INGRESS, EndpointToken, FORBIDDEN_IAM_ACTIONS, INTERNET_EGRESS, IdlePolicy,
    LaunchError, MAX_DURATION_SECONDS, MAX_IDLE_DURATION_SECONDS, MAX_RUN_HOOK_PAYLOAD_BYTES,
    MicrovmControlApi, MicrovmDescription, MicrovmPage, RUN_HOOK_PAYLOAD_KEYS, RUNTIME_IAM_ACTIONS,
    RunHookBounds, RunHookPayload, RunRequest, SUSPENDED_DURATION_SECONDS, TOKEN_REFRESH_SECONDS,
    TOKEN_TTL_SECONDS,
};
