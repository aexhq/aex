//! The telemetry composition a long-lived host installs.
//!
//! One call, so a composition root gains production diagnostics in a handful of
//! lines and no host re-derives the settings profile, the exporter or the pump
//! cadence. A second host copying those three choices is how two processes end
//! up with different queue bounds and nobody notices.
//!
//! A Lambda does not use this: it keeps [`Settings::lambda`] and flushes on its
//! own invocation boundary, which is a bound this composition does not have.

use std::sync::Arc;
use std::time::Duration;

use crate::exporter::Exporter;
use crate::facade::{FlushOutcome, Handle};
use crate::json::{JsonLinesExporter, LogStream};
use crate::pump::{PumpStartError, StatsSink, TelemetryPump};
use crate::settings::Settings;

/// Production telemetry for a process that runs until it is drained.
#[derive(Debug)]
pub struct LongLivedTelemetry {
    handle: Handle,
    pump: TelemetryPump,
}

impl LongLivedTelemetry {
    /// How often the pump drains the queue.
    ///
    /// A second is short enough that a burst is visible in the log while it is
    /// still happening, and long enough that a quiet process writes nothing.
    pub const FLUSH_INTERVAL: Duration = Duration::from_secs(1);

    /// How often the pump publishes its own queue state.
    ///
    /// Rare compared to the flush interval: the queue state is a background
    /// health signal, and one line every half minute is enough to see a queue
    /// filling or a drop counter climbing under load without becoming noise.
    pub const STATS_INTERVAL: Duration = Duration::from_secs(30);

    /// Installs the long-lived profile, a `JSON`-lines exporter on stdout and a
    /// pump that drains it.
    ///
    /// # Errors
    ///
    /// Returns [`PumpStartError`] when the pump thread cannot be spawned. A
    /// host treats that as a refusal to start rather than running blind: an
    /// installed exporter with nothing draining it fills its queue and drops
    /// every record after that, which is worse than no telemetry because it
    /// looks like a quiet process.
    pub fn install() -> Result<Self, PumpStartError> {
        let settings = Settings::long_lived();
        let exporter = Arc::new(JsonLinesExporter::new(Arc::new(LogStream::Stdout)));
        let handle = Handle::install(&settings, Some(Arc::clone(&exporter) as Arc<dyn Exporter>));
        let pump = TelemetryPump::start_reporting(
            handle.clone(),
            Self::FLUSH_INTERVAL,
            settings.flush_deadline,
            exporter as Arc<dyn StatsSink>,
            Self::STATS_INTERVAL,
        )?;
        Ok(Self { handle, pump })
    }

    /// The handle product code emits through.
    #[must_use]
    pub const fn handle(&self) -> &Handle {
        &self.handle
    }

    /// The longest [`Self::shutdown`] can wait.
    #[must_use]
    pub const fn shutdown_budget(&self) -> Duration {
        self.pump.shutdown_budget()
    }

    /// Stops the pump and reports the final bounded flush.
    #[must_use]
    pub fn shutdown(mut self) -> FlushOutcome {
        self.pump.stop_and_join()
    }
}
