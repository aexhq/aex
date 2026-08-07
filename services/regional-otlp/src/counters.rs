//! Per-request admission accounting, emitted through the installed telemetry
//! handle.
//!
//! Before this module the deployable emitted exactly one record in its life —
//! the startup event — so the degraded-gate counter the spool contract
//! documents ([`aex_observation_store_aws::spool::GateState::Degraded`]:
//! "admission proceeds and a counter is emitted") did not exist, and neither an
//! emptying redaction manifest nor an exhausted materialization left any trace.
//!
//! Counters are emitted directly per occurrence rather than through the
//! interval publisher the stream service uses: this is a Lambda, frozen between
//! invocations, so a background drain task has no reliable clock — and the
//! record rate is bounded by a handful of counters per admission, not by the
//! record count inside a batch.

use aex_platform_telemetry::{Handle, Record};
use aex_telemetry_schema::generated::{
    AEX_DEPLOYABLE, AEX_OTLP_COUNTER, AEX_PLANE, AEX_REGION, METRIC_AEX_OTLP_COUNT,
};

/// The identity this deployable reports in every record.
const DEPLOYABLE: &str = "regional-otlp";

/// One measured admission outcome.
///
/// The set is closed on purpose: it is the metric dimension, so a variant added
/// here is a registry decision rather than a call-site decision.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum AdmissionCounter {
    /// Batches admitted while the regional ingress gate was `degraded` — the
    /// counter the gate contract promises.
    DegradedGateAdmission,
    /// Observation records committed by an admission.
    RecordsAdmitted,
    /// Custody manifest entries that could not be parsed. Every occurrence is
    /// a batch that failed closed; after the fail-closed fix this counts
    /// attempts, never silently dropped entries.
    CustodyEntriesMalformed,
    /// Materializations that exhausted their bounded retries with unprocessed
    /// items remaining.
    MaterializeRetryExhausted,
}

impl AdmissionCounter {
    /// Every counter, in declaration order. Test-only: production call sites
    /// name the counter they count.
    #[cfg(test)]
    pub const ALL: &'static [Self] = &[
        Self::DegradedGateAdmission,
        Self::RecordsAdmitted,
        Self::CustodyEntriesMalformed,
        Self::MaterializeRetryExhausted,
    ];

    /// The stable dimension value of this counter.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::DegradedGateAdmission => "degraded_gate_admission",
            Self::RecordsAdmitted => "records_admitted",
            Self::CustodyEntriesMalformed => "custody_entries_malformed",
            Self::MaterializeRetryExhausted => "materialize_retry_exhausted",
        }
    }
}

/// The bound emitter every admission shares.
#[derive(Clone, Debug)]
pub struct AdmissionTelemetry {
    handle: Handle,
    plane: String,
    region: String,
}

impl AdmissionTelemetry {
    /// Binds the emitter to the installed handle and this process's identity.
    #[must_use]
    pub fn new(handle: Handle, plane: impl Into<String>, region: impl Into<String>) -> Self {
        Self {
            handle,
            plane: plane.into(),
            region: region.into(),
        }
    }

    /// Emits one delta of one counter. A zero delta is a non-event.
    pub fn count(&self, counter: AdmissionCounter, delta: u64) {
        if delta == 0 {
            return;
        }
        let delta = i64::try_from(delta).unwrap_or(i64::MAX);
        self.handle.emit(
            Record::metric(METRIC_AEX_OTLP_COUNT, delta)
                .with(AEX_OTLP_COUNTER, counter.as_str())
                .with(AEX_PLANE, self.plane.clone())
                .with(AEX_REGION, self.region.clone())
                .with(AEX_DEPLOYABLE, DEPLOYABLE),
        );
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use aex_platform_telemetry::{
        AttributeValue, Exporter, Handle, InMemoryExporter, RecordKind, Settings,
    };
    use aex_telemetry_schema::generated::{AEX_OTLP_COUNTER, METRIC_AEX_OTLP_COUNT};

    use super::{AdmissionCounter, AdmissionTelemetry};

    fn telemetry() -> (AdmissionTelemetry, Arc<InMemoryExporter>) {
        let exporter = Arc::new(InMemoryExporter::default());
        let handle = Handle::install(
            &Settings::default(),
            Some(Arc::clone(&exporter) as Arc<dyn Exporter>),
        );
        (
            AdmissionTelemetry::new(handle, "dev", "eu-west-1"),
            exporter,
        )
    }

    #[test]
    fn counter_dimension_values_are_unique_and_within_the_declared_length() {
        let mut seen = std::collections::BTreeSet::new();
        for counter in AdmissionCounter::ALL {
            assert!(seen.insert(counter.as_str()), "{counter:?} is not unique");
            assert!(counter.as_str().len() <= 32, "{counter:?} exceeds max_len");
        }
    }

    #[test]
    fn one_delta_becomes_one_registry_declared_record() {
        let (telemetry, _exporter) = telemetry();
        telemetry.count(AdmissionCounter::RecordsAdmitted, 250);
        telemetry.count(AdmissionCounter::DegradedGateAdmission, 1);
        assert_eq!(telemetry.handle.pending(), 2);
        let outcome = telemetry
            .handle
            .flush(std::time::Duration::from_millis(100));
        assert!(matches!(
            outcome,
            aex_platform_telemetry::FlushOutcome::Drained { exported: 2 }
        ));
    }

    #[test]
    fn a_zero_delta_emits_nothing() {
        let (telemetry, _exporter) = telemetry();
        telemetry.count(AdmissionCounter::CustodyEntriesMalformed, 0);
        assert_eq!(telemetry.handle.pending(), 0);
    }

    #[test]
    fn the_record_carries_the_instrument_and_its_dimension() {
        let (telemetry, exporter) = telemetry();
        telemetry.count(AdmissionCounter::MaterializeRetryExhausted, 1);
        let _ = telemetry
            .handle
            .flush(std::time::Duration::from_millis(100));
        let records = exporter.delivered();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].kind, RecordKind::Metric);
        assert_eq!(records[0].name, METRIC_AEX_OTLP_COUNT);
        assert_eq!(records[0].value, Some(1));
        assert_eq!(
            records[0].attribute(AEX_OTLP_COUNTER),
            Some(&AttributeValue::Text(
                "materialize_retry_exhausted".to_owned()
            ))
        );
    }
}
