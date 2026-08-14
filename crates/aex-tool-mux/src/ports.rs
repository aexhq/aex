use core::future::Future;
use core::pin::Pin;

use aex_hands_protocol::operation::GuestPath;
use aex_hands_protocol::rpc::HandsOperationId;
use aex_runtime_control::HandId;
use aex_wire::ids::{GenerationId, SessionId};

use crate::contract::{ExecutorOutput, ReadyHand, ToolCallIdentity, ToolTarget};

/// Sendable boxed future used by every Tool Mux port.
pub type ToolMuxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// Runtime Control boundary. It owns provider lifecycle and durable waiters.
pub trait RuntimePort: Send + Sync + 'static {
    /// Starts exact-generation preparation behind a durable waiter and returns promptly.
    fn start_waiter<'a>(
        &'a self,
        session: SessionId,
        hand: HandId,
        generation: GenerationId,
        call: &'a ToolCallIdentity,
    ) -> ToolMuxFuture<'a, Result<(), String>>;

    /// Polls preparation/readiness without starting another waiter.
    fn poll_waiter<'a>(
        &'a self,
        session: SessionId,
        hand: HandId,
        generation: GenerationId,
        call: &'a ToolCallIdentity,
    ) -> ToolMuxFuture<'a, Result<Option<ReadyHand>, String>>;

    /// Cancels an original pre-dispatch waiter, or recovers exact-generation
    /// readiness after a process restart so the deterministic guest operation
    /// can be cancelled directly.
    fn cancel_waiter<'a>(
        &'a self,
        generation: GenerationId,
        call: &'a ToolCallIdentity,
    ) -> ToolMuxFuture<'a, Result<Option<ReadyHand>, String>>;

    /// Settles the waiter after terminal execution; Runtime Control may suspend.
    fn settle_waiter<'a>(
        &'a self,
        ready: ReadyHand,
        call: &'a ToolCallIdentity,
    ) -> ToolMuxFuture<'a, Result<(), String>>;
}

/// Credential-free exact-generation Hands bridge.
pub trait GuestPort: Send + Sync + 'static {
    /// Performs `/hello` and validates the response against `ready`.
    fn hello<'a>(&'a self, ready: ReadyHand) -> ToolMuxFuture<'a, Result<(), String>>;

    /// Starts an official or sandbox-MCP call on the exact generation.
    fn start<'a>(
        &'a self,
        ready: ReadyHand,
        target: &'a ToolTarget,
        arguments: &'a serde_json::Value,
        call: &'a ToolCallIdentity,
        deadline_ms: i64,
        max_result_bytes: usize,
        timeout_ms: u32,
    ) -> ToolMuxFuture<'a, Result<HandsOperationId, String>>;

    /// Reads an already accepted guest operation without starting another.
    fn read<'a>(
        &'a self,
        ready: ReadyHand,
        operation: HandsOperationId,
        max_result_bytes: usize,
        timeout_ms: u32,
    ) -> ToolMuxFuture<'a, Result<Option<ExecutorOutput>, String>>;

    /// Best-effort cancellation on the exact generation.
    fn cancel<'a>(
        &'a self,
        ready: ReadyHand,
        call: &'a ToolCallIdentity,
    ) -> ToolMuxFuture<'a, Result<(), String>>;
}

/// Narrow latest-only workspace persistence port implemented by file authority.
pub trait StoragePersistPort: Send + Sync + 'static {
    /// Starts a latest-only persistence operation behind a detached handle.
    fn start_persist<'a>(
        &'a self,
        ready: ReadyHand,
        source: &'a GuestPath,
        logical_name: &'a str,
        media_type: Option<&'a str>,
        call: &'a ToolCallIdentity,
    ) -> ToolMuxFuture<'a, Result<(), String>>;

    /// Reads an already-started persistence operation.
    fn read_persist<'a>(
        &'a self,
        ready: ReadyHand,
        call: &'a ToolCallIdentity,
    ) -> ToolMuxFuture<'a, Result<Option<ExecutorOutput>, String>>;

    /// Best-effort cancellation of one persistence operation.
    fn cancel_persist<'a>(
        &'a self,
        ready: ReadyHand,
        call: &'a ToolCallIdentity,
    ) -> ToolMuxFuture<'a, Result<(), String>>;
}
