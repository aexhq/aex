use std::collections::BTreeMap;

use aex_hands_protocol::operation::GuestPath;
use aex_hands_protocol::rpc::{Fence, HandsOperationId};
use aex_runtime_control::HandId;
use aex_wire::Uuid7;
use aex_wire::ids::{
    AgentId, ContentHash, GenerationId, MessageId, OrganizationId, ResourceName, SessionId,
    WorkspaceId,
};
use serde::{Deserialize, Serialize};

/// The live stream and model-context preview ceiling.
///
/// This converts API Gateway/WebSocket and provider-context frame pressure into
/// a declared truncation boundary. Full bytes remain available separately.
pub const MAX_LIVE_PREVIEW_BYTES: usize = 64 * 1_024;

/// One call's durable attribution identity.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ToolCallIdentity {
    /// Organization whose storage and encryption context own the call.
    pub organization: OrganizationId,
    /// Workspace authorization boundary.
    pub workspace: WorkspaceId,
    /// Owning customer session.
    pub session: SessionId,
    /// Agent whose model emitted the call.
    pub agent: AgentId,
    /// Assistant message declaring the call.
    pub message: MessageId,
    /// Stable batch ordinal.
    pub batch: u64,
    /// Stable provider call identity.
    pub call: String,
    /// Stable dispatch attempt; ambiguity never mints another attempt silently.
    pub attempt: u32,
}

/// Session's frozen sandbox choice.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct SandboxConfig {
    /// Explicit opt-out is false; omission is resolved to true before this port.
    pub enabled: bool,
    /// Exact generation allocated during session admission when enabled.
    pub generation: Option<GenerationId>,
}

/// Session-admission notification that starts sandbox preparation without
/// making session creation wait for a provider boot.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct EagerPrepareRequest {
    /// Newly admitted session.
    pub session: SessionId,
    /// Frozen session sandbox selection and exact generation.
    pub sandbox: SandboxConfig,
}

/// Official sandbox commands exposed to the model.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OfficialSandboxTool {
    /// Read a file under `/workspace`.
    Read,
    /// Apply an exact edit under `/workspace`.
    Edit,
    /// Create or overwrite a file under `/workspace`.
    Write,
    /// Run the explicit shell tool.
    Bash,
}

/// Qualified target selected from the frozen session catalog.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "target",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub enum ToolTarget {
    /// One of the four official sandbox tools.
    OfficialSandbox {
        /// Exact installed tool.
        tool: OfficialSandboxTool,
    },
    /// Streamable HTTP MCP outside the Hand.
    RemoteMcp {
        /// Frozen server name.
        server: ResourceName,
        /// Qualified HTTPS endpoint.
        endpoint: String,
        /// Header name to session-scoped custody reference. Plaintext never
        /// crosses the Brain-to-Tool-Mux boundary.
        headers: BTreeMap<String, ResourceName>,
        /// Frozen remote tool name.
        tool: String,
    },
    /// MCP process inside this session's exact Hand generation.
    SandboxMcp {
        /// Frozen server name.
        server: ResourceName,
        /// Frozen executable selected at session admission.
        command: String,
        /// Frozen process arguments.
        args: Vec<String>,
        /// Environment name to session-scoped custody reference.
        environment: BTreeMap<String, ResourceName>,
        /// Frozen guest working directory.
        working_directory: Option<String>,
        /// Frozen remote tool name.
        tool: String,
    },
    /// Persist a sandbox file as the latest value of a workspace logical name.
    StoragePersist {
        /// Exact source path under `/workspace`.
        source: GuestPath,
        /// Workspace logical name; file authority validates its grammar.
        logical_name: String,
        /// Optional media type asserted by the caller and verified by policy.
        media_type: Option<String>,
    },
}

impl ToolTarget {
    /// Whether target needs a Hand.
    #[must_use]
    pub const fn needs_hand(&self) -> bool {
        !matches!(self, Self::RemoteMcp { .. })
    }
}

/// Internal start command sent by Brain after its durable dispatch write.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ToolStartRequest {
    /// Stable attribution and replay identity.
    pub identity: ToolCallIdentity,
    /// Frozen sandbox setting.
    pub sandbox: SandboxConfig,
    /// Qualified target.
    pub target: ToolTarget,
    /// Canonical JSON arguments already validated against the frozen schema.
    pub arguments: serde_json::Value,
    /// Absolute unix-millisecond deadline.
    pub deadline_ms: i64,
    /// Maximum complete result bytes admitted by the frozen catalog.
    pub max_result_bytes: usize,
    /// Operation wall-clock ceiling admitted by the frozen catalog.
    pub timeout_ms: u32,
}

/// Runtime-authoritative exact endpoint binding.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ReadyHand {
    /// One logical Hand for the session.
    pub hand: HandId,
    /// Exact current provider generation.
    pub generation: GenerationId,
    /// Current lifecycle fence.
    pub fence: Fence,
}

/// Full result location before trusted retention.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FullOutput {
    /// Bounded complete bytes from a small remote result.
    Inline(Vec<u8>),
    /// Complete result already written to a stable call-scoped sandbox path.
    SandboxFile {
        /// Exact path inside the sandbox.
        path: GuestPath,
        /// Complete byte count.
        bytes: u64,
        /// Hash over the complete file.
        hash: ContentHash,
    },
}

/// Executor output before preview and retention normalization.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecutorOutput {
    /// Bounded bytes suitable for live preview. Tool Mux applies its own bound again.
    pub preview: Vec<u8>,
    /// Complete bytes or exact sandbox file.
    pub full: FullOutput,
    /// Whether the tool itself returned an error result.
    pub is_error: bool,
}

/// Full retained result reference safe for durable handles and telemetry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct RetainedResult {
    /// Opaque S3/object reference, never a bearer URL.
    pub object_ref: String,
    /// Complete length.
    pub bytes: u64,
    /// Complete hash.
    pub hash: ContentHash,
    /// Sandbox path when the complete result remains available to the model.
    pub sandbox_path: Option<String>,
}

/// Model-visible, streamable terminal result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ToolCompletion {
    /// UTF-8-lossy bounded preview.
    pub preview: String,
    /// Whether the preview omits complete content.
    pub truncated: bool,
    /// Complete retained result metadata.
    pub retained: Option<RetainedResult>,
    /// Tool-result error, distinct from transport failure.
    pub error: Option<ToolError>,
}

/// Stable model-visible error.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ToolError {
    /// Stable machine code.
    pub code: String,
    /// Redacted recovery guidance for the model.
    pub message: String,
}

impl ToolCompletion {
    /// Explicit sandbox opt-out result.
    #[must_use]
    pub fn sandbox_disabled() -> Self {
        Self {
            preview: "Sandbox tools are unavailable because this session disabled its sandbox. Continue without the tool or ask the user to create a sandbox-enabled session.".to_owned(),
            truncated: false,
            retained: None,
            error: Some(ToolError {
                code: "sandbox_disabled".to_owned(),
                message: "This session explicitly disabled its sandbox; no runtime was created."
                    .to_owned(),
            }),
        }
    }
}

/// Durable executor handle. It contains identity, never bearer credentials.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "executor",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub enum ToolHandle {
    /// Guest operation on one exact Hand generation.
    Sandbox {
        /// Logical Hand.
        hand: HandId,
        /// Exact provider generation.
        generation: GenerationId,
        /// Fence observed after exact-generation readiness.
        fence: Fence,
        /// Stable guest operation.
        operation: HandsOperationId,
        /// Result ceiling required for resumable verification.
        max_result_bytes: usize,
        /// Original operation wall-clock ceiling.
        timeout_ms: u32,
    },
    /// Remote MCP operation identity.
    RemoteMcp {
        /// Frozen server.
        server: ResourceName,
        /// Stable opaque operation identity.
        operation: Uuid7,
    },
    /// Latest-only workspace persistence operation.
    StoragePersist {
        /// Logical Hand.
        hand: HandId,
        /// Exact provider generation.
        generation: GenerationId,
        /// Stable opaque operation identity.
        operation: Uuid7,
    },
}

/// Start response: terminal now or detached behind a durable handle.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "status",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub enum ToolStart {
    /// Completed during start.
    Completed {
        /// Terminal normalized result.
        result: ToolCompletion,
    },
    /// Accepted and readable later.
    Accepted {
        /// Durable credential-free handle.
        handle: ToolHandle,
    },
}

/// Read or cancel command for one exact durable handle.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ToolHandleRequest {
    /// Original call attribution.
    pub identity: ToolCallIdentity,
    /// Exact handle returned by start.
    pub handle: ToolHandle,
}

/// Bounded result read.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "status",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub enum ToolRead {
    /// Executor has not reached terminal state.
    Pending,
    /// Terminal normalized result.
    Completed {
        /// Terminal normalized result.
        result: ToolCompletion,
    },
}
