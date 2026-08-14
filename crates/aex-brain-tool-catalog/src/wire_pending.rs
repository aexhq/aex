//! Temporary peer-owned type declarations.
//!
//! Items are added here only when the owning Brain stream has not landed the
//! domain type required by this crate. Both remaining items are now blocked on a
//! divergence rather than on an absence: `aex-brain-domain` has landed, and it
//! publishes neither concept in the shape this crate needs. Each marker names the
//! real path it was measured against, so no marker here names a path that does
//! not resolve.

use serde::{Deserialize, Serialize};

pub use aex_brain_domain::EffectClass;

/// The executor selected before any tool I/O.
// TODO(cross-stream): `aex-brain-domain` has no `tool` module. It publishes
// `aex_brain_domain::journal::ExecutorRoute`, which records only the coarse
// execution authority (`BrainInline` or `ToolMux`). Routing still needs the
// fine target to select native control, storage, remote MCP, or one of the
// exact-generation guest surfaces behind that authority.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExecutorRoute {
    /// Fold-local Brain control.
    Control,
    /// A durable park with no held activation resource.
    Park,
    /// The Brain child scheduler.
    SubagentScheduler,
    /// Brain-managed web target selected inside Tool Mux.
    ManagedWeb,
    /// Latest-only file persistence target selected inside Tool Mux.
    PlatformStorage,
    /// A pinned remote MCP target selected inside Tool Mux.
    Mcp,
    /// Sandbox filesystem target selected inside Tool Mux.
    HandsFilesystem,
    /// Sandbox development target selected inside Tool Mux.
    HandsDevelopment,
    /// Sandbox browser target selected inside Tool Mux.
    HandsBrowser,
    /// Registered sandbox-process MCP target selected inside Tool Mux.
    RegisteredCustom,
}

/// Whether the recovery loop may query a durable operation identity.
// TODO(cross-stream): `aex-brain-domain` has no such type in its `effect` module.
// The only one it has is `aex_brain_domain::wire_pending::DurableOperationSupport`,
// which is itself a stand-in and spells the second arm `Proven`, not `Query`.
// The type that will settle this is `aex_model_catalog::document::DurableOperationSupport`,
// whose arms carry the retention window (`ResultLookup { ttl_ms }`,
// `ResumableStream { .. }`) rather than a bare yes/no.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DurableOperationSupport {
    /// No durable query identity exists.
    None,
    /// The operation can be queried by its committed identity.
    Query,
}
