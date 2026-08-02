//! GENERATED — DO NOT EDIT.
//!
//! The effective-limit registry.
//!
//! Produced by `aex-contract-gen` from `api/`; contract digest
//! `sha256:17cc35493241db0815502eb31a2e2f691aa2e1f1f06e1f489c3fa362ff783369`.
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
    /// `query.page` — Largest accepted collection page size.
    #[serde(rename = "query.page")]
    QueryPage,
    /// `request.body_bytes` — Largest accepted encoded JSON request body.
    #[serde(rename = "request.body_bytes")]
    RequestBodyBytes,
    /// `otlp.body_bytes` — Largest accepted encoded and decoded OTLP body.
    #[serde(rename = "otlp.body_bytes")]
    OtlpBodyBytes,
    /// `stream.frame` — Largest NDJSON records frame in observations and bytes.
    #[serde(rename = "stream.frame")]
    StreamFrame,
    /// `session.concurrent` — Concurrently non-idle sessions in the workspace.
    #[serde(rename = "session.concurrent")]
    SessionConcurrent,
    /// `session.subagent_depth` — Deepest subagent nesting inside one run.
    #[serde(rename = "session.subagent_depth")]
    SessionSubagentDepth,
    /// `session.subagent_concurrency` — Concurrently admitted subagents inside one run.
    #[serde(rename = "session.subagent_concurrency")]
    SessionSubagentConcurrency,
    /// `upload.part_bytes` — Largest single staged upload part.
    #[serde(rename = "upload.part_bytes")]
    UploadPartBytes,
    /// `upload.object_bytes` — Largest staged upload object.
    #[serde(rename = "upload.object_bytes")]
    UploadObjectBytes,
    /// `download.range_bytes` — Largest byte range one download grant may sign.
    #[serde(rename = "download.range_bytes")]
    DownloadRangeBytes,
    /// `registry.value_bytes` — Largest registered resource value.
    #[serde(rename = "registry.value_bytes")]
    RegistryValueBytes,
    /// `registry.entries` — Registered resource count per kind.
    #[serde(rename = "registry.entries")]
    RegistryEntries,
    /// `secret.value_bytes` — Largest accepted secret value.
    #[serde(rename = "secret.value_bytes")]
    SecretValueBytes,
    /// `telemetry.retention_days` — Days an admitted observation is retained.
    #[serde(rename = "telemetry.retention_days")]
    TelemetryRetentionDays,
    /// `telemetry.export_bytes` — Largest telemetry export artifact.
    #[serde(rename = "telemetry.export_bytes")]
    TelemetryExportBytes,
    /// `observation.filter` — Structural bounds of an observation filter.
    #[serde(rename = "observation.filter")]
    ObservationFilter,
}

impl LimitId {
    /// Every limit, in registry order.
    pub const ALL: &'static [LimitId] = &[
        LimitId::QueryPage,
        LimitId::RequestBodyBytes,
        LimitId::OtlpBodyBytes,
        LimitId::StreamFrame,
        LimitId::SessionConcurrent,
        LimitId::SessionSubagentDepth,
        LimitId::SessionSubagentConcurrency,
        LimitId::UploadPartBytes,
        LimitId::UploadObjectBytes,
        LimitId::DownloadRangeBytes,
        LimitId::RegistryValueBytes,
        LimitId::RegistryEntries,
        LimitId::SecretValueBytes,
        LimitId::TelemetryRetentionDays,
        LimitId::TelemetryExportBytes,
        LimitId::ObservationFilter,
    ];

    /// The wire spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::QueryPage => "query.page",
            Self::RequestBodyBytes => "request.body_bytes",
            Self::OtlpBodyBytes => "otlp.body_bytes",
            Self::StreamFrame => "stream.frame",
            Self::SessionConcurrent => "session.concurrent",
            Self::SessionSubagentDepth => "session.subagent_depth",
            Self::SessionSubagentConcurrency => "session.subagent_concurrency",
            Self::UploadPartBytes => "upload.part_bytes",
            Self::UploadObjectBytes => "upload.object_bytes",
            Self::DownloadRangeBytes => "download.range_bytes",
            Self::RegistryValueBytes => "registry.value_bytes",
            Self::RegistryEntries => "registry.entries",
            Self::SecretValueBytes => "secret.value_bytes",
            Self::TelemetryRetentionDays => "telemetry.retention_days",
            Self::TelemetryExportBytes => "telemetry.export_bytes",
            Self::ObservationFilter => "observation.filter",
        }
    }

    /// Whether the effective value is a scalar or a map.
    #[must_use]
    pub const fn shape(self) -> LimitShape {
        match self {
            Self::QueryPage => LimitShape::Scalar,
            Self::RequestBodyBytes => LimitShape::Scalar,
            Self::OtlpBodyBytes => LimitShape::Map,
            Self::StreamFrame => LimitShape::Map,
            Self::SessionConcurrent => LimitShape::Scalar,
            Self::SessionSubagentDepth => LimitShape::Scalar,
            Self::SessionSubagentConcurrency => LimitShape::Scalar,
            Self::UploadPartBytes => LimitShape::Scalar,
            Self::UploadObjectBytes => LimitShape::Scalar,
            Self::DownloadRangeBytes => LimitShape::Scalar,
            Self::RegistryValueBytes => LimitShape::Scalar,
            Self::RegistryEntries => LimitShape::Map,
            Self::SecretValueBytes => LimitShape::Scalar,
            Self::TelemetryRetentionDays => LimitShape::Scalar,
            Self::TelemetryExportBytes => LimitShape::Scalar,
            Self::ObservationFilter => LimitShape::Map,
        }
    }

    /// Resolves a wire spelling.
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        Self::ALL.iter().copied().find(|it| it.as_str() == text)
    }
}
