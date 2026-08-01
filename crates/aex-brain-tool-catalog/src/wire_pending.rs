//! Temporary peer-owned type declarations.
//!
//! Items are added here only when the owning Brain stream has not yet landed
//! the generated/domain type required by this crate.

use serde::{Deserialize, Serialize};

/// How replay affects an already-started tool effect.
// TODO(cross-stream): replaced by aex_brain_domain::effect::EffectClass at merge
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EffectClass {
    /// A total function of committed input.
    Pure,
    /// A managed operation with an AEX-controlled idempotency identity.
    IdempotentManaged,
    /// A detached operation that exposes a durable query identity.
    DurableDetached,
    /// An effect whose outcome is ambiguous after dispatch.
    NonReplayable,
}

/// The executor selected before any tool I/O.
// TODO(cross-stream): replaced by aex_brain_domain::tool::ExecutorRoute at merge
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExecutorRoute {
    /// Fold-local Brain control.
    Control,
    /// A durable park with no held activation resource.
    Park,
    /// The Brain child scheduler.
    SubagentScheduler,
    /// Brain-managed web egress.
    ManagedWeb,
    /// A pinned remote MCP server.
    Mcp,
    /// Hands filesystem operations.
    HandsFilesystem,
    /// Hands development operations.
    HandsDevelopment,
    /// Hands browser operations.
    HandsBrowser,
    /// A registered custom Hands executor.
    RegisteredCustom,
}

/// Whether the recovery loop may query a durable operation identity.
// TODO(cross-stream): replaced by aex_brain_domain::effect::DurableOperationSupport at merge
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DurableOperationSupport {
    /// No durable query identity exists.
    None,
    /// The operation can be queried by its committed identity.
    Query,
}
