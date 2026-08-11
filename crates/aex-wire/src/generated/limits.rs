//! GENERATED — DO NOT EDIT.
//!
//! The effective-limit registry.
//!
//! Produced by `aex-contract-gen` from `api/`; contract digest
//! `sha256:c74b728e361ca151f7d1db58d802e04b88cbd663dc2e03f9ab6d4c27f300b3b8`.
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
    /// materialized synchronously before session creation returns ready.
    #[serde(rename = "session.initial_files_bytes")]
    SessionInitialFilesBytes,
    /// `session.initial_files_count` — Largest number of registered workspace files selected for
    /// synchronous session creation.
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
    /// `telemetry.batch` — Encoded, decoded, record and normalized-observation admission bounds.
    #[serde(rename = "telemetry.batch")]
    TelemetryBatch,
    /// `telemetry.ingest_rate` — Sustained and burst byte and request rates per workspace.
    #[serde(rename = "telemetry.ingest_rate")]
    TelemetryIngestRate,
    /// `telemetry.metric_series` — Exact active metric-series ceiling per workspace.
    #[serde(rename = "telemetry.metric_series")]
    TelemetryMetricSeries,
    /// `query.filter` — Structural bounds of an observation query filter.
    #[serde(rename = "query.filter")]
    QueryFilter,
    /// `query.page` — Largest accepted collection page in items and serialized bytes.
    #[serde(rename = "query.page")]
    QueryPage,
    /// `stream.frame` — Largest NDJSON records frame in observations and encoded bytes.
    #[serde(rename = "stream.frame")]
    StreamFrame,
    /// `metric.aggregate` — Interval, grouping, calculation, bucket and row aggregation bounds.
    #[serde(rename = "metric.aggregate")]
    MetricAggregate,
    /// `registry.entries` — Registered names one workspace may hold in one registry.
    #[serde(rename = "registry.entries")]
    RegistryEntries,
    /// `registry.value_bytes` — Largest canonical value document a registered name may carry.
    #[serde(rename = "registry.value_bytes")]
    RegistryValueBytes,
    /// `telemetry.export` — Concurrent non-terminal telemetry exports per workspace, and the widest
    /// observation-time window one export may cover. A cost guard rather than a correctness guard:
    /// the active count is read eventually consistently, so a race may admit one or two over the
    /// cap, which is a better failure than the transactional counter that leaks and blocks a
    /// workspace permanently.
    #[serde(rename = "telemetry.export")]
    TelemetryExport,
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
        LimitId::TelemetryBatch,
        LimitId::TelemetryIngestRate,
        LimitId::TelemetryMetricSeries,
        LimitId::QueryFilter,
        LimitId::QueryPage,
        LimitId::StreamFrame,
        LimitId::MetricAggregate,
        LimitId::RegistryEntries,
        LimitId::RegistryValueBytes,
        LimitId::TelemetryExport,
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
            Self::TelemetryBatch => "telemetry.batch",
            Self::TelemetryIngestRate => "telemetry.ingest_rate",
            Self::TelemetryMetricSeries => "telemetry.metric_series",
            Self::QueryFilter => "query.filter",
            Self::QueryPage => "query.page",
            Self::StreamFrame => "stream.frame",
            Self::MetricAggregate => "metric.aggregate",
            Self::RegistryEntries => "registry.entries",
            Self::RegistryValueBytes => "registry.value_bytes",
            Self::TelemetryExport => "telemetry.export",
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
            Self::TelemetryBatch => LimitShape::Map,
            Self::TelemetryIngestRate => LimitShape::Map,
            Self::TelemetryMetricSeries => LimitShape::Scalar,
            Self::QueryFilter => LimitShape::Map,
            Self::QueryPage => LimitShape::Map,
            Self::StreamFrame => LimitShape::Map,
            Self::MetricAggregate => LimitShape::Map,
            Self::RegistryEntries => LimitShape::Scalar,
            Self::RegistryValueBytes => LimitShape::Scalar,
            Self::TelemetryExport => LimitShape::Map,
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
            Self::TelemetryBatch => &[
                "encoded_bytes",
                "decoded_bytes",
                "records",
                "observation_bytes",
                "attributes",
                "attribute_key_bytes",
                "attribute_value_bytes",
                "array_elements",
            ],
            Self::TelemetryIngestRate => &[
                "sustained_bytes_per_second",
                "burst_bytes",
                "sustained_requests_per_second",
                "burst_requests",
            ],
            Self::TelemetryMetricSeries => &[],
            Self::QueryFilter => &[
                "depth",
                "leaves",
                "children_per_boolean",
                "in_values",
                "string_bytes",
            ],
            Self::QueryPage => &["items", "serialized_bytes"],
            Self::StreamFrame => &["records", "encoded_bytes"],
            Self::MetricAggregate => &[
                "interval_min_seconds",
                "interval_max_seconds",
                "group_fields",
                "calculations",
                "buckets_per_series",
                "rows",
            ],
            Self::RegistryEntries => &[],
            Self::RegistryValueBytes => &[],
            Self::TelemetryExport => &["active", "window_days"],
        }
    }

    /// Resolves a wire spelling.
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        Self::ALL.iter().copied().find(|it| it.as_str() == text)
    }
}
