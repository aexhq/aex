//! The port traits every Brain peer stream implements.
//!
//! These signatures are the cross-stream contract. `aex-brain-provider-gateway`,
//! `aex-model-catalog`, `aex-brain-tool-catalog`, `aex-brain-managed-web`, `aex-brain-mcp`,
//! `aex-brain-hands` and `aex-brain-store-dynamodb` each implement one or more of them; nothing
//! in this crate knows a vendor.
//!
//! # Why every future is boxed
//!
//! Boxed futures make every port dyn-compatible, so the composition root selects adapters
//! at runtime instead of threading a generic parameter through thirteen parallel streams.
//! One allocation per call is immaterial next to the HTTP request it wraps, and the
//! alternative — a generic parameter per port — would make `brain-mux` a single
//! monomorphized type that no two streams could change independently.
//!
//! # Ordering enforced by types
//!
//! Two capability tokens carry the split-phase rule into the type system.
//!
//! - [`FenceGuard`] is required by every store write. Holding one is the proof that the
//!   caller claimed the agent and has not been fenced out.
//! - [`DispatchTicket`] is minted only from claimed [`SessionAuthority`], a `FenceGuard` and
//!   the effect it belongs to, and [`ProviderPort::dispatch`] will not accept anything else.
//!   A caller that tries to send a byte before the guarded `dispatch_started` transaction
//!   does not compile.

pub mod catalog;
pub mod hands;
pub mod proof;
pub mod provider;
pub mod store;
pub mod tool;

/// A boxed, `Send` future — the return shape of every asynchronous port method.
pub type BoxFuture<'a, T> = core::pin::Pin<Box<dyn core::future::Future<Output = T> + Send + 'a>>;

pub use catalog::{CatalogDigest, CatalogError, CatalogPort, ClockPort, IdPort, SteadyInstant};
pub use hands::{
    HandsAccepted, HandsEndpoint, HandsError, HandsOperationStart, HandsOperationStatus, HandsPort,
    HandsResult, ResultBounds,
};
pub use proof::{
    CancelToken, DispatchTicket, FenceGuard, NullPreviewSink, PreviewSink, StreamBudget,
    TicketMismatch,
};
pub use provider::{
    ProviderDispatchError, ProviderFailureClass, ProviderFailureKind, ProviderOutcome,
    ProviderPort, RedactedDetail, UnknownResolution,
};
pub use store::{
    AgentHead, Claim, ClaimError, CommitError, CommitReceipt, ConditionFailure, DecisionContext,
    DueRowIsolation, DueRowIsolationReason, DueScanCursor, DueScanPage, DurableWake, EffectStore,
    JournalCursor, JournalPage, JournalStore, LeaseStore, MAX_DUE_ROW_ISOLATIONS,
    MalformedWakeDelivery, MalformedWakeReason, ReadBudget, ReleaseDisposition, SessionAuthority,
    StoreError, WakeBatch, WakeDelivery, WakeOrigin, WakeQueue, WakeState,
};
pub use tool::{
    ControlStateView, DetachedStatus, PreparedToolCall, ToolAdvertisement, ToolDispatchError,
    ToolOutcome, ToolPort, ToolResultBody, ToolRoute, ToolRoutingError,
};
