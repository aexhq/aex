use core::future::Future;
use core::pin::Pin;
use std::collections::BTreeMap;

use aex_hands_protocol::operation::GuestPath;
use aex_hands_protocol::rpc::HandsOperationId;
use aex_runtime_control::HandId;
use aex_wire::ids::{ContentHash, GenerationId, ResourceName, SessionId};

use crate::contract::{
    ExecutorOutput, ReadyHand, RetainedResult, SandboxConfig, ToolCallIdentity, ToolHandle,
    ToolTarget,
};
use crate::telemetry::PreparationProgress;

/// Sendable boxed future used by every Tool Mux port.
pub type ToolMuxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// Runtime Control boundary. It owns provider lifecycle and durable waiters.
pub trait RuntimePort: Send + Sync + 'static {
    /// Starts the eager prepare/materialize/qualify/suspend path once.
    fn eager_prepare<'a>(
        &'a self,
        session: SessionId,
        sandbox: SandboxConfig,
    ) -> ToolMuxFuture<'a, Result<Vec<PreparationProgress>, String>>;

    /// Adds a durable waiter and resolves only after the exact generation is ready.
    fn wait_ready<'a>(
        &'a self,
        session: SessionId,
        hand: HandId,
        generation: GenerationId,
        call: &'a ToolCallIdentity,
    ) -> ToolMuxFuture<'a, Result<ReadyHand, String>>;

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
    ) -> ToolMuxFuture<'a, Result<Result<ExecutorOutput, HandsOperationId>, String>>;

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
        operation: HandsOperationId,
    ) -> ToolMuxFuture<'a, Result<(), String>>;
}

/// Qualified Streamable HTTP MCP boundary.
pub trait McpPort: Send + Sync + 'static {
    /// Calls one frozen remote tool. This path never asks Runtime Control for a Hand.
    fn call_remote<'a>(
        &'a self,
        endpoint: &'a str,
        headers: &'a BTreeMap<String, ResourceName>,
        server: &'a ResourceName,
        tool: &'a str,
        arguments: &'a serde_json::Value,
        call: &'a ToolCallIdentity,
    ) -> ToolMuxFuture<'a, Result<ExecutorOutput, String>>;
}

/// Narrow latest-only workspace persistence port implemented by file authority.
pub trait StoragePersistPort: Send + Sync + 'static {
    /// Streams one exact sandbox file under a single-purpose grant, verifies it,
    /// and replaces the logical workspace name idempotently.
    fn persist<'a>(
        &'a self,
        ready: ReadyHand,
        source: &'a GuestPath,
        logical_name: &'a str,
        media_type: Option<&'a str>,
        call: &'a ToolCallIdentity,
    ) -> ToolMuxFuture<'a, Result<ExecutorOutput, String>>;
}

/// Trusted full-result retention boundary.
pub trait ResultRetentionPort: Send + Sync + 'static {
    /// Retains a complete small result.
    fn retain_inline<'a>(
        &'a self,
        call: &'a ToolCallIdentity,
        body: &'a [u8],
    ) -> ToolMuxFuture<'a, Result<RetainedResult, String>>;

    /// Streams a full sandbox result to session S3 without guest credentials.
    fn retain_sandbox_file<'a>(
        &'a self,
        call: &'a ToolCallIdentity,
        ready: ReadyHand,
        path: &'a GuestPath,
        bytes: u64,
        hash: ContentHash,
    ) -> ToolMuxFuture<'a, Result<RetainedResult, String>>;

    /// Reads or cancels a detached result already represented by a durable handle.
    fn read_handle<'a>(
        &'a self,
        handle: &'a ToolHandle,
    ) -> ToolMuxFuture<'a, Result<Option<ExecutorOutput>, String>>;
}
