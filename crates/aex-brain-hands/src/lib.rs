//! `aex-brain-hands` owns the Brain-side exact-generation Hands adapter: operation
//! dispatch, frame handling, fence checks, cancellation and result incorporation.
//!
//! # Invariants
//!
//! - every operation is addressed to an exact generation; a generation mismatch is refused
//! - guest loss is observable as guest loss, not as a failed operation result
//! - a cancelled operation is never incorporated into the fold
//!
//! # Not this crate's job
//!
//! - the wire protocol definition (`aex-hands-protocol`) and its frame codec
//!   (`aex_hands_agent::wire`, consumed here so there is one codec and not two)
//! - guest-side execution (`aex-hands-agent`, `aex-hands-tools`)
//! - runtime lifecycle (`aex-runtime-control`, `aex-hands-control-aws`)

pub mod adapter;
pub mod backend;
pub mod guest;
pub mod operation;
pub mod port;

pub use backend::ProductionHandsBackend;
pub use port::{HandsAdapter, HandsBackend};

pub use guest::{AuthenticatedGuestEndpoint, GuestReply, HttpGuestTransport};

pub use adapter::{
    AdmitPlan, Alpn, CONNECTION_IDLE_MS, CONTEXT_TOOL_RESULT_BYTES, HandsError,
    LAUNCH_POLL_BASE_MS, LAUNCH_POLL_MAX_MS, MAX_FRAME_BYTES, MAX_RESULT_BODY_BYTES,
    MaterializeStep, RESERVED_CONNECTIONS, SettlePlan, admit, launch_backoff_ms, materialize_step,
    max_in_flight, pool_size, settle, transport_mode,
};
pub use operation::{
    CODE_RUN_MAX_WALL_MS, CODE_RUN_WALL_MS, CodeLanguage, ConstructError, ConstructedCommand,
    DETACHED_WALL_MS, EXEC_WALL_MS, IncorporateError, MAX_CODE_BYTES, MAX_PACKAGES, PackageManager,
    ResultAssembly, TOOLCHAIN_WALL_MS, code_run, git, package_install,
};

/// The frame codec both sides of the Hands wire share.
///
/// Re-exported rather than reimplemented: two codecs that can disagree is exactly
/// the failure this stream is meant to remove.
pub use aex_hands_agent::wire;
