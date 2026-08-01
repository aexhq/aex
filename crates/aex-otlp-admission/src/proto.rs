//! The generated `OTLP` message types.
//!
//! Produced at build time by `prost-build` from the vendored `.proto` tree; no
//! `opentelemetry` SDK crate is involved, because the OTLP wire format is an
//! input format here rather than a telemetry library. The generated tree keeps
//! its upstream package nesting so the cross-package references inside it
//! resolve without patching generated code.
#![allow(missing_docs, reason = "generated from the vendored .proto tree")]
#![allow(clippy::all, clippy::pedantic, reason = "generated code")]

include!(concat!(env!("OUT_DIR"), "/_otlp.rs"));

/// `opentelemetry.proto.collector.logs.v1`.
pub use opentelemetry::proto::collector::logs::v1 as collector_logs;
/// `opentelemetry.proto.collector.metrics.v1`.
pub use opentelemetry::proto::collector::metrics::v1 as collector_metrics;
/// `opentelemetry.proto.collector.trace.v1`.
pub use opentelemetry::proto::collector::trace::v1 as collector_trace;
/// `opentelemetry.proto.common.v1`.
pub use opentelemetry::proto::common::v1 as common;
/// `opentelemetry.proto.logs.v1`.
pub use opentelemetry::proto::logs::v1 as logs;
/// `opentelemetry.proto.metrics.v1`.
pub use opentelemetry::proto::metrics::v1 as metrics;
/// `opentelemetry.proto.resource.v1`.
pub use opentelemetry::proto::resource::v1 as resource;
/// `opentelemetry.proto.trace.v1`.
pub use opentelemetry::proto::trace::v1 as trace;
