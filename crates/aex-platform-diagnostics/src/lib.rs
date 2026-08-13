//! `aex-platform-diagnostics` installs AEX's process-local JSON diagnostics.
//!
//! This crate is deliberately one standard-library adapter: `tracing` events go
//! through `tracing-subscriber`'s JSON formatter to stdout, filtered by
//! `RUST_LOG` with an `info` default. It owns no queue, retry loop, exporter,
//! schema registry, flush protocol, or generated vocabulary.
//!
//! Callers must emit only fields already safe for private operational logs. A
//! generic subscriber cannot infer which domain values contain credentials, and
//! this crate does not pretend to provide semantic redaction.
//!
//! Customer OpenTelemetry admission and durable session telemetry are separate
//! product paths. Nothing here receives, transforms, or persists customer data.

use tracing_subscriber::EnvFilter;

/// Failure to install the process-global tracing subscriber.
pub type InstallError = Box<dyn std::error::Error + Send + Sync + 'static>;

/// Installs JSON diagnostics on stdout.
///
/// `RUST_LOG` controls filtering. An absent or invalid directive falls back to
/// `info`, keeping a configuration typo from disabling every diagnostic.
///
/// # Errors
///
/// Returns an error when another process-global subscriber is already installed.
pub fn install_json() -> Result<(), InstallError> {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .json()
        .try_init()
}
