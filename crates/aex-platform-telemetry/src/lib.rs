//! `aex-platform-telemetry` owns the narrow `tracing`/`OpenTelemetry` facade
//! every AEX Rust host installs: bounded non-blocking recording, registry-driven
//! redaction, drop counters and a flush with a deadline.
//!
//! Domain crates use the generated semantic keys from `aex-telemetry-schema` and
//! this small span, event and instrument surface. They never construct an
//! `OpenTelemetry` provider or exporter, which is what keeps that beta dependency
//! churn at the host boundary.
//!
//! # Invariants
//!
//! - this crate is never a product-correctness dependency: an absent exporter, a
//!   failing exporter and an exceeded flush deadline are all counted no-ops
//! - it never panics; the queue is bounded and every lock is poison-free
//! - an attribute whose registry visibility class is not public is removed before
//!   a record can be queued, so it cannot reach an exporter at all
//! - no export, retry, flush or shutdown may extend a customer deadline or change
//!   a run or settlement result
//!
//! # Not this crate's job
//!
//! - customer observation admission: customer-visible semantic events go through
//!   `aex-observation-application` and obey its durable admission and gap contract
//! - naming: attribute, instrument, span and event names and their classes belong
//!   to `aex-telemetry-schema`
//! - transport choice: the host binary selects the profile — [`LongLivedTelemetry`]
//!   for a process that runs until it is drained, [`Settings::lambda`] plus an
//!   invocation-bound flush for a Lambda — and that is configuration rather than
//!   instrumentation

pub mod exporter;
pub mod facade;
pub mod host;
pub mod json;
pub mod pump;
pub mod record;
pub mod settings;

pub use exporter::{ExportError, Exporter, FailingExporter, InMemoryExporter};
pub use facade::{FlushOutcome, Handle, TelemetryStats};
pub use host::LongLivedTelemetry;
pub use json::{JsonLinesExporter, LineCeilingTooSmall, LineSink, LogStream};
pub use pump::{PumpStartError, StatsSink, TelemetryPump};
pub use record::{Attribute, AttributeValue, Record, RecordKind};
pub use settings::Settings;
