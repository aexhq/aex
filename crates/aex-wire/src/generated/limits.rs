//! GENERATED — DO NOT EDIT.
//!
//! The effective-limit registry.
//!
//! Produced by `aex-contract-gen` from `api/`; contract digest
//! `sha256:ac9f9d4da5543cd71acab49d7078ab631b8e776b8b9e3a2239fd64ce54f060ff`.
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
    /// `content.bundle_expand` — Expanded bytes, entries and path length admitted from a bundle.
    #[serde(rename = "content.bundle_expand")]
    ContentBundleExpand,
    /// `tools.io_safety` — Per-tool byte, traversal and listing safety bounds.
    #[serde(rename = "tools.io_safety")]
    ToolsIoSafety,
}

impl LimitId {
    /// Every limit, in registry order.
    pub const ALL: &'static [LimitId] = &[
        LimitId::ContextToolResultBytes,
        LimitId::SessionMaterializedAgents,
        LimitId::ApiJsonBody,
        LimitId::TelemetryBatch,
        LimitId::TelemetryIngestRate,
        LimitId::TelemetryMetricSeries,
        LimitId::QueryFilter,
        LimitId::QueryPage,
        LimitId::StreamFrame,
        LimitId::MetricAggregate,
        LimitId::ContentBundleExpand,
        LimitId::ToolsIoSafety,
    ];

    /// The wire spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ContextToolResultBytes => "context.tool_result_bytes",
            Self::SessionMaterializedAgents => "session.materialized_agents",
            Self::ApiJsonBody => "api.json_body",
            Self::TelemetryBatch => "telemetry.batch",
            Self::TelemetryIngestRate => "telemetry.ingest_rate",
            Self::TelemetryMetricSeries => "telemetry.metric_series",
            Self::QueryFilter => "query.filter",
            Self::QueryPage => "query.page",
            Self::StreamFrame => "stream.frame",
            Self::MetricAggregate => "metric.aggregate",
            Self::ContentBundleExpand => "content.bundle_expand",
            Self::ToolsIoSafety => "tools.io_safety",
        }
    }

    /// Whether the effective value is a scalar or a map.
    #[must_use]
    pub const fn shape(self) -> LimitShape {
        match self {
            Self::ContextToolResultBytes => LimitShape::Scalar,
            Self::SessionMaterializedAgents => LimitShape::Scalar,
            Self::ApiJsonBody => LimitShape::Scalar,
            Self::TelemetryBatch => LimitShape::Map,
            Self::TelemetryIngestRate => LimitShape::Map,
            Self::TelemetryMetricSeries => LimitShape::Scalar,
            Self::QueryFilter => LimitShape::Map,
            Self::QueryPage => LimitShape::Map,
            Self::StreamFrame => LimitShape::Map,
            Self::MetricAggregate => LimitShape::Map,
            Self::ContentBundleExpand => LimitShape::Map,
            Self::ToolsIoSafety => LimitShape::Map,
        }
    }

    /// The complete ordered dimension vocabulary for a map limit.
    #[must_use]
    pub const fn dimensions(self) -> &'static [&'static str] {
        match self {
            Self::ContextToolResultBytes => &[],
            Self::SessionMaterializedAgents => &[],
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
            Self::ContentBundleExpand => &["expanded_bytes", "entries", "path_bytes"],
            Self::ToolsIoSafety => &[
                "web_fetch_bytes",
                "shell_output_bytes",
                "grep_input_bytes",
                "head_tail_input_bytes",
                "walk_files",
                "list_entries",
                "list_depth",
            ],
        }
    }

    /// Resolves a wire spelling.
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        Self::ALL.iter().copied().find(|it| it.as_str() == text)
    }
}
