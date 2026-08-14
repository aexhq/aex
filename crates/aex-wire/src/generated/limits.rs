//! GENERATED — DO NOT EDIT.
//!
//! The effective-limit registry.
//!
//! Produced by `aex-contract-gen` from `api/`; contract digest
//! `sha256:e7f95cd7fc830a0871b1276cde9ae11e4245d0b843cd3dba567e133e551e051f`.
//! Regenerate with `cargo run -p aex-contract-gen -- build`.

#![allow(clippy::large_enum_variant, reason = "a wire union is never boxed")]
#![allow(clippy::match_same_arms, reason = "one arm per row")]
#![allow(clippy::too_many_lines, reason = "one arm per row")]

use serde::{Deserialize, Serialize};

/// Whether a limit's effective value is one integer or a map of named dimensions.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LimitShape {
    /// A single non-negative integer.
    Scalar,
    /// A map from a named dimension to a non-negative integer.
    Map,
}

/// Every adjustable workspace safety limit. Enforcement belongs to the regional applications; this
/// crate owns only the identity and the public shape.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LimitId {
    /// `context.tool_result_bytes` — Largest tool result admitted into the next model context.
    #[serde(rename = "context.tool_result_bytes")]
    ContextToolResultBytes,
    /// `session.materialized_agents` — Root-plus-subagents concurrently materialized in one
    /// session.
    #[serde(rename = "session.materialized_agents")]
    SessionMaterializedAgents,
    /// `session.initial_files_bytes` — Largest aggregate byte size of registered workspace files
    /// frozen at session admission and materialized during asynchronous sandbox preparation.
    #[serde(rename = "session.initial_files_bytes")]
    SessionInitialFilesBytes,
    /// `session.initial_files_count` — Largest number of registered workspace files frozen at
    /// session admission for asynchronous sandbox preparation.
    #[serde(rename = "session.initial_files_count")]
    SessionInitialFilesCount,
    /// `session.agent_execution` — Revisioned Brain planner and structural ceilings pinned when a
    /// session is created.
    #[serde(rename = "session.agent_execution")]
    SessionAgentExecution,
    /// `session.run_budget` — Revisioned per-message duration and Brain budget ceilings pinned when
    /// a session is created.
    #[serde(rename = "session.run_budget")]
    SessionRunBudget,
    /// `api.json_body` — Largest accepted encoded JSON request body.
    #[serde(rename = "api.json_body")]
    ApiJsonBody,
    /// `query.page` — Largest accepted collection page in items and serialized bytes.
    #[serde(rename = "query.page")]
    QueryPage,
    /// `registry.entries` — Registered names one workspace may hold in one registry.
    #[serde(rename = "registry.entries")]
    RegistryEntries,
    /// `registry.value_bytes` — Largest canonical value document a registered name may carry.
    #[serde(rename = "registry.value_bytes")]
    RegistryValueBytes,
}

impl LimitId {
    /// Every limit, in registry order.
    pub const ALL: &'static [LimitId] = &[
        LimitId::ContextToolResultBytes,
        LimitId::SessionMaterializedAgents,
        LimitId::SessionInitialFilesBytes,
        LimitId::SessionInitialFilesCount,
        LimitId::SessionAgentExecution,
        LimitId::SessionRunBudget,
        LimitId::ApiJsonBody,
        LimitId::QueryPage,
        LimitId::RegistryEntries,
        LimitId::RegistryValueBytes,
    ];

    /// The wire spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ContextToolResultBytes => "context.tool_result_bytes",
            Self::SessionMaterializedAgents => "session.materialized_agents",
            Self::SessionInitialFilesBytes => "session.initial_files_bytes",
            Self::SessionInitialFilesCount => "session.initial_files_count",
            Self::SessionAgentExecution => "session.agent_execution",
            Self::SessionRunBudget => "session.run_budget",
            Self::ApiJsonBody => "api.json_body",
            Self::QueryPage => "query.page",
            Self::RegistryEntries => "registry.entries",
            Self::RegistryValueBytes => "registry.value_bytes",
        }
    }

    /// Whether the effective value is a scalar or a map.
    #[must_use]
    pub const fn shape(self) -> LimitShape {
        match self {
            Self::ContextToolResultBytes => LimitShape::Scalar,
            Self::SessionMaterializedAgents => LimitShape::Scalar,
            Self::SessionInitialFilesBytes => LimitShape::Scalar,
            Self::SessionInitialFilesCount => LimitShape::Scalar,
            Self::SessionAgentExecution => LimitShape::Map,
            Self::SessionRunBudget => LimitShape::Map,
            Self::ApiJsonBody => LimitShape::Scalar,
            Self::QueryPage => LimitShape::Map,
            Self::RegistryEntries => LimitShape::Scalar,
            Self::RegistryValueBytes => LimitShape::Scalar,
        }
    }

    /// The complete ordered dimension vocabulary for a map limit.
    #[must_use]
    pub const fn dimensions(self) -> &'static [&'static str] {
        match self {
            Self::ContextToolResultBytes => &[],
            Self::SessionMaterializedAgents => &[],
            Self::SessionInitialFilesBytes => &[],
            Self::SessionInitialFilesCount => &[],
            Self::SessionAgentExecution => &[
                "max_turns",
                "max_steps_per_turn",
                "turn_deadline_ms",
                "max_depth",
                "max_fanout",
            ],
            Self::SessionRunBudget => &[
                "max_run_duration_ms",
                "total_children_created",
                "provider_calls",
                "hands_calls",
                "queued_children",
                "retained_result_bytes",
            ],
            Self::ApiJsonBody => &[],
            Self::QueryPage => &["items", "serialized_bytes"],
            Self::RegistryEntries => &[],
            Self::RegistryValueBytes => &[],
        }
    }

    /// Resolves a wire spelling.
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        Self::ALL.iter().copied().find(|it| it.as_str() == text)
    }
}
