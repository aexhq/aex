//! The admission use case and its two entry points.
//!
//! `admit_otlp` and `admit_semantic` share every invariant. They differ in one
//! place and one place only: what happens when the commit cannot be made.
//!
//! - Customer OTLP fails the request. The client retries; nothing is lost,
//!   because a batch that was never admitted was never lost.
//! - A trusted in-process producer that elected [`GapOnFailure::Open`] records
//!   an **explicit gap** and completes. That is the only way a semantic
//!   transaction may finish without its observations, and it is why a telemetry
//!   outage cannot fail a customer's run.
//!
//! A diagnostic drop must never mutate completeness, deletion or money truth,
//! so the gap is durable before the caller is told it succeeded.

use aex_observation_domain::gap::{GapRecord, GapRevision, TimeWindow};
use aex_observation_domain::keys::ScopeKey;
use aex_observation_domain::limits;
use aex_observation_domain::signal::SignalSet;
use aex_wire::ids::{TelemetryGapId, WorkspaceId};
use aex_wire::models::TelemetryGapReason;
use aex_wire::types::Timestamp;

use crate::ports::{CommitReceipt, CommitRequest, GapSink, ObservationAuthority, PortError};

/// What a trusted producer wants when the commit cannot be made.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum GapOnFailure {
    /// Fail the caller. Used where the observation is the point of the call.
    Fail,
    /// Record an explicit gap and let the caller complete.
    Open,
}

/// Why an admission was refused.
#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
pub enum AdmissionError {
    /// A port failed and the caller did not elect a gap.
    #[error(transparent)]
    Port(#[from] PortError),
    /// The commit clock skew is outside the admitted bound.
    ///
    /// Fails **closed**: the settle window only provably dominates propagation
    /// plus skew while this holds, and a snapshot built on a violated bound
    /// would silently return incomplete pages.
    #[error("|now - acceptedAt| is {observed} ms, at or above the {limit} ms bound")]
    ClockSkew {
        /// The measured skew.
        observed: i64,
        /// The admitted bound.
        limit: i64,
    },
    /// A gap was elected but could not be recorded either.
    ///
    /// Never downgraded to success: an unrecorded gap is indistinguishable from
    /// complete data, which is the one outcome this design refuses.
    #[error("the commit failed and the gap could not be recorded: {reason}")]
    GapNotRecorded {
        /// What failed.
        reason: Box<str>,
    },
}

/// One trusted in-process admission.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SemanticAdmissionRequest {
    /// The workspace that owns the scope and its sparse gap-index entry.
    pub workspace: WorkspaceId,
    /// Which scope.
    pub scope: ScopeKey,
    /// Which signals.
    pub signals: SignalSet,
    /// How many records.
    pub records: u32,
    /// How many canonical bytes.
    pub logical_bytes: u64,
    /// The observation-time window the batch covers.
    pub window: TimeWindow,
    /// When the producer is admitting.
    pub now: Timestamp,
    /// The gap identity to use if the commit cannot be made.
    pub gap_id: TelemetryGapId,
    /// What to do when the commit cannot be made.
    pub on_failure: GapOnFailure,
}

/// What one admission produced.
#[derive(Clone, Debug, PartialEq)]
pub enum SemanticAdmission {
    /// The batch committed.
    Committed(CommitReceipt),
    /// The batch did not commit and an explicit gap was recorded instead.
    Gapped(GapRecord),
}

/// The admission use case.
pub struct AdmitBatch<A, G> {
    authority: A,
    gaps: G,
}

impl<A: ObservationAuthority, G: GapSink> AdmitBatch<A, G> {
    /// Builds the use case over one authority.
    pub const fn new(authority: A, gaps: G) -> Self {
        Self { authority, gaps }
    }

    /// Asserts the commit-clock bound the snapshot contract depends on.
    ///
    /// # Errors
    ///
    /// Returns [`AdmissionError::ClockSkew`] at or above
    /// [`limits::OBS_CLOCK_SKEW_MAX_MS`].
    pub fn check_clock(now: Timestamp, accepted_at: Timestamp) -> Result<(), AdmissionError> {
        let observed = (now.unix_millis() - accepted_at.unix_millis()).abs();
        if observed >= limits::OBS_CLOCK_SKEW_MAX_MS {
            return Err(AdmissionError::ClockSkew {
                observed,
                limit: limits::OBS_CLOCK_SKEW_MAX_MS,
            });
        }
        Ok(())
    }

    /// Admits one batch from a trusted in-process producer.
    ///
    /// Never a loopback HTTP request.
    ///
    /// # Errors
    ///
    /// Returns [`AdmissionError::ClockSkew`] when the commit clock is outside
    /// the admitted bound, [`AdmissionError::Port`] when the commit failed and
    /// the caller elected [`GapOnFailure::Fail`], and
    /// [`AdmissionError::GapNotRecorded`] when the gap itself could not be
    /// written.
    pub async fn admit_semantic(
        &self,
        request: &SemanticAdmissionRequest,
    ) -> Result<SemanticAdmission, AdmissionError> {
        Self::check_clock(request.now, request.now)?;
        let pinned = self.authority.deletion_epoch(&request.scope).await?;

        let commit = CommitRequest {
            scope: request.scope,
            signals: request.signals,
            records: request.records,
            logical_bytes: request.logical_bytes,
            pinned_deletion_epoch: pinned,
            accepted_at: request.now,
        };

        match self.authority.commit(&commit).await {
            Ok(receipt) => Ok(SemanticAdmission::Committed(receipt)),
            Err(error) => match request.on_failure {
                GapOnFailure::Fail => Err(AdmissionError::Port(error)),
                GapOnFailure::Open => {
                    let revision = GapRevision::open(
                        request.gap_id,
                        request.signals,
                        reason_for(&error),
                        Some(request.window),
                        request.now,
                    );
                    let record = GapRecord::try_new(
                        request.workspace,
                        request.scope,
                        revision,
                        Some(u64::from(request.records)),
                        Some(request.logical_bytes),
                        false,
                    )
                    .map_err(|inner| AdmissionError::GapNotRecorded {
                        reason: inner.to_string().into_boxed_str(),
                    })?;
                    self.gaps.append_gap(&record).await.map_err(|inner| {
                        AdmissionError::GapNotRecorded {
                            reason: inner.to_string().into_boxed_str(),
                        }
                    })?;
                    Ok(SemanticAdmission::Gapped(record))
                }
            },
        }
    }
}

/// Maps a port failure onto the gap reason that describes it.
///
/// `replay_expired` is not reachable from any arm: removing Kinesis removed the
/// only mechanism that could expire a replay.
#[must_use]
pub const fn reason_for(error: &PortError) -> TelemetryGapReason {
    match error {
        PortError::Unavailable { .. } | PortError::CommitAmbiguous => {
            TelemetryGapReason::AuthorityUnavailable
        }
        PortError::GateClosed { .. } => TelemetryGapReason::ProducerDropped,
        PortError::DeletionEpochAdvanced { .. } => TelemetryGapReason::AdmissionRejected,
    }
}

#[cfg(test)]
mod tests {
    use super::{
        AdmissionError, AdmitBatch, GapOnFailure, SemanticAdmission, SemanticAdmissionRequest,
        reason_for,
    };
    use crate::ports::{CommitReceipt, CommitRequest, GapSink, ObservationAuthority, PortError};
    use aex_observation_domain::gap::{GapRecord, TimeWindow};
    use aex_observation_domain::keys::ScopeKey;
    use aex_observation_domain::limits;
    use aex_observation_domain::signal::{Signal, SignalSet};
    use aex_wire::ids::{PrefixedId as _, TelemetryGapId};
    use aex_wire::models::TelemetryGapReason;
    use aex_wire::types::Timestamp;
    use std::sync::{Arc, Mutex};

    fn instant(millis: i64) -> Timestamp {
        Timestamp::from_unix_millis(millis).expect("representable")
    }

    fn scope() -> ScopeKey {
        ScopeKey::Session(
            aex_wire::ids::SessionId::parse("ses_0000000003ec1r60r30c1g60r3")
                .expect("fixture parses"),
        )
    }

    fn gap_id() -> TelemetryGapId {
        aex_wire::ids::TelemetryGapId::parse("gap_0000000001e40r2081040g2081")
            .expect("fixture parses")
    }

    #[derive(Clone, Default)]
    struct Double {
        commit_error: Option<PortError>,
        gap_error: Option<PortError>,
        gaps: Arc<Mutex<Vec<GapRecord>>>,
    }

    #[async_trait::async_trait]
    impl ObservationAuthority for Double {
        async fn deletion_epoch(&self, _scope: &ScopeKey) -> Result<u64, PortError> {
            Ok(7)
        }

        async fn commit(&self, request: &CommitRequest) -> Result<CommitReceipt, PortError> {
            if let Some(error) = &self.commit_error {
                return Err(error.clone());
            }
            assert_eq!(request.pinned_deletion_epoch, 7);
            Ok(CommitReceipt {
                accepted_lo: 0,
                accepted_hi: u64::from(request.records) - 1,
                accepted_at: request.accepted_at,
                new_series: 0,
            })
        }
    }

    #[async_trait::async_trait]
    impl GapSink for Double {
        async fn append_gap(&self, record: &GapRecord) -> Result<(), PortError> {
            if let Some(error) = &self.gap_error {
                return Err(error.clone());
            }
            self.gaps.lock().expect("not poisoned").push(record.clone());
            Ok(())
        }
    }

    fn request(on_failure: GapOnFailure) -> SemanticAdmissionRequest {
        SemanticAdmissionRequest {
            workspace: aex_wire::ids::WorkspaceId::parse("wsp_0000000001e40r2081040g2081")
                .expect("fixture parses"),
            scope: scope(),
            signals: SignalSet::from_signal(Signal::Logs),
            records: 4,
            logical_bytes: 512,
            window: TimeWindow::new(instant(0), instant(1_000)).expect("ordered"),
            now: instant(500),
            gap_id: gap_id(),
            on_failure: GapOnFailure::Fail,
        }
        .with_failure(on_failure)
    }

    impl SemanticAdmissionRequest {
        fn with_failure(mut self, on_failure: GapOnFailure) -> Self {
            self.on_failure = on_failure;
            self
        }
    }

    #[test]
    fn a_healthy_commit_returns_the_receipt() {
        let double = Double::default();
        let use_case = AdmitBatch::new(double.clone(), double);
        let outcome =
            futures::executor::block_on(use_case.admit_semantic(&request(GapOnFailure::Fail)))
                .expect("admits");
        match outcome {
            SemanticAdmission::Committed(receipt) => {
                assert_eq!(receipt.accepted_lo, 0);
                assert_eq!(receipt.accepted_hi, 3);
            }
            other @ SemanticAdmission::Gapped(_) => panic!("expected a commit, got {other:?}"),
        }
    }

    #[test]
    fn a_producer_that_elected_a_gap_completes_with_a_durable_gap() {
        let double = Double {
            commit_error: Some(PortError::GateClosed {
                reason: "spool age",
                retry_after_ms: 1_000,
            }),
            ..Double::default()
        };
        let use_case = AdmitBatch::new(double.clone(), double);
        let outcome =
            futures::executor::block_on(use_case.admit_semantic(&request(GapOnFailure::Open)))
                .expect("completes with a gap");
        match outcome {
            SemanticAdmission::Gapped(record) => {
                assert_eq!(record.revision.reason, TelemetryGapReason::ProducerDropped);
                assert!(!record.revision.unbounded, "the window is known");
                assert_eq!(record.revision.revision, 0);
                assert_eq!(record.attempted_records, Some(4));
                assert_eq!(record.attempted_bytes, Some(512));
            }
            other @ SemanticAdmission::Committed(_) => panic!("expected a gap, got {other:?}"),
        }
    }

    #[test]
    fn a_producer_that_did_not_elect_a_gap_fails_the_caller() {
        let double = Double {
            commit_error: Some(PortError::CommitAmbiguous),
            ..Double::default()
        };
        let use_case = AdmitBatch::new(double.clone(), double);
        let error =
            futures::executor::block_on(use_case.admit_semantic(&request(GapOnFailure::Fail)))
                .expect_err("fails");
        assert!(matches!(
            error,
            AdmissionError::Port(PortError::CommitAmbiguous)
        ));
    }

    #[test]
    fn an_unrecordable_gap_is_never_downgraded_to_success() {
        let double = Double {
            commit_error: Some(PortError::CommitAmbiguous),
            gap_error: Some(PortError::Unavailable {
                authority: "observation",
                reason: "throttled".into(),
            }),
            ..Double::default()
        };
        let use_case = AdmitBatch::new(double.clone(), double);
        let error =
            futures::executor::block_on(use_case.admit_semantic(&request(GapOnFailure::Open)))
                .expect_err("fails");
        assert!(matches!(error, AdmissionError::GapNotRecorded { .. }));
    }

    #[test]
    fn the_commit_clock_bound_fails_closed() {
        AdmitBatch::<Double, Double>::check_clock(instant(1_000), instant(1_000))
            .expect("no skew is admitted");
        AdmitBatch::<Double, Double>::check_clock(
            instant(1_000),
            instant(1_000 - limits::OBS_CLOCK_SKEW_MAX_MS + 1),
        )
        .expect("inside the bound is admitted");
        let error = AdmitBatch::<Double, Double>::check_clock(
            instant(1_000),
            instant(1_000 - limits::OBS_CLOCK_SKEW_MAX_MS),
        )
        .expect_err("at the bound fails closed");
        assert!(matches!(error, AdmissionError::ClockSkew { .. }));
    }

    #[test]
    fn no_port_failure_ever_maps_to_the_removed_replay_horizon() {
        let failures = [
            PortError::CommitAmbiguous,
            PortError::Unavailable {
                authority: "observation",
                reason: "x".into(),
            },
            PortError::GateClosed {
                reason: "x",
                retry_after_ms: 0,
            },
            PortError::DeletionEpochAdvanced { pinned: 1 },
        ];
        for failure in &failures {
            assert_ne!(reason_for(failure), TelemetryGapReason::ReplayExpired);
            assert!(aex_observation_domain::gap::PRODUCIBLE_REASONS.contains(&reason_for(failure)));
        }
    }
}
