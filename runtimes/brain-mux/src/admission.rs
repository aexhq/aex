//! Admission control.
//!
//! Overload must not become loss. Every refusal here is typed and retryable and leaves the
//! durable wake untouched, so the worst an overloaded task can do is schedule work later
//! than it would have liked.
//!
//! Three bands, from the pool evaluation's measurements:
//!
//! - up to the **target**, admit normally;
//! - between target and the **safety cap**, stop pulling from the queue and drain only what
//!   is already local — an unbounded global semaphore produced a 9.5x victim p95 slowdown,
//!   while a bounded cap produced 86.5 ms victim p95 and finished the mixed batch 43 %
//!   sooner;
//! - above the safety cap, shed with a typed reason.
//!
//! Bands bound the bytes this process *promised*. Measured memory pressure is the second gate
//! over the bytes it did not — see [`crate::pressure`] — and it acts on receipt only.

use crate::pressure::PressureGate;
use aex_brain_application::kernel::{DrainGate, PermitKind, PermitSet, PermitSetFull, Reservation};
use std::sync::Arc;

/// The process resources held for one activation's lifetime.
///
/// External-I/O permits remain part of the validated task envelope, but are acquired by the
/// phase-specific dispatch gate only after durable preparation. A refusal writes a typed
/// continuation and releases the lease; it never waits locally while owning the session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ActivationResources {
    /// Peak resident bytes reserved before any snapshot or journal body is read.
    pub context_bytes: u64,
    /// Maximum provider stream buffer held by one activation.
    pub stream_buffer_bytes: u64,
    /// Provider streams required by one provider/network dispatch.
    pub provider_streams: u64,
    /// Hands RPC slots required by one Hands dispatch.
    pub hands_rpcs: u64,
}

impl ActivationResources {
    const fn requests(self) -> [(PermitKind, u64); 3] {
        [
            (PermitKind::Activation, 1),
            (PermitKind::ContextBytes, self.context_bytes),
            (PermitKind::StreamBufferBytes, self.stream_buffer_bytes),
        ]
    }

    /// Whether every per-activation resource has a non-zero reservation.
    pub(crate) const fn validate(self) -> Result<(), &'static str> {
        if self.context_bytes == 0 {
            return Err("the activation context reservation must be positive");
        }
        if self.stream_buffer_bytes == 0 {
            return Err("the activation stream-buffer reservation must be positive");
        }
        if self.provider_streams == 0 {
            return Err("the activation provider-stream reservation must be positive");
        }
        if self.hands_rpcs == 0 {
            return Err("the activation Hands reservation must be positive");
        }
        Ok(())
    }
}

/// The declared bands one task admits within.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AdmissionBounds {
    /// The number of concurrently active activations normal admission targets.
    pub target: u32,
    /// The number above which no new work is admitted at all.
    pub safety_cap: u32,
    /// The offered load the task must survive without loss, deadlock or an OOM.
    pub offered_ceiling: u32,
}

impl Default for AdmissionBounds {
    /// The approved first-alpha bands: 16 active activations per task.
    ///
    /// The task envelope proves 48 simultaneous worst-case restores and the accepted launch
    /// profile deliberately does not use them. Sixteen is a decision to observe before raising
    /// concurrency, not a capacity finding, and the load evidence is what raises it. Production
    /// runs two tasks, so 32 activations are active across the placement before either task
    /// queues — a different number from this task's own 32 safety cap, which is the point above
    /// which one task sheds.
    ///
    /// Production derives its bands from `AEX_MAX_ACTIVE_ACTIVATIONS` rather than from here;
    /// this is the same profile, held where a test can pin it.
    fn default() -> Self {
        Self {
            target: 16,
            safety_cap: 32,
            offered_ceiling: 80,
        }
    }
}

impl AdmissionBounds {
    /// Whether the bands are usable.
    ///
    /// # Errors
    ///
    /// Returns the reason when the target is above the safety cap, the cap is above the
    /// offered ceiling, or any is zero. A cap below its target would shed every request the
    /// target admitted, which is worse than either bound alone.
    pub const fn validate(&self) -> Result<(), &'static str> {
        if self.target == 0 {
            return Err("the admission target must be positive");
        }
        if self.target > self.safety_cap {
            return Err("the admission target must not exceed the safety cap");
        }
        if self.safety_cap > self.offered_ceiling {
            return Err("the safety cap must not exceed the offered ceiling");
        }
        Ok(())
    }
}

/// Why work was not admitted.
///
/// Both arms are retryable and both leave the durable wake in place. There is deliberately
/// no arm that means "dropped".
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum TypedOverload {
    /// The task is at or above its safety cap.
    #[error("{active} active activations at the {cap} safety cap")]
    AtSafetyCap {
        /// How many are running.
        active: u32,
        /// The cap.
        cap: u32,
    },
    /// A bounded resource the activation needs is exhausted.
    #[error("{kind:?} is exhausted: {requested} requested, {available} available")]
    ResourceExhausted {
        /// Which resource.
        kind: PermitKind,
        /// How much was asked for.
        requested: u64,
        /// How much was free.
        available: u64,
    },
    /// The process is draining, so it admits nothing new.
    #[error("draining")]
    Draining,
}

impl TypedOverload {
    /// Whether the caller may retry. Always true: shedding is a scheduling decision, never
    /// a verdict about the work.
    #[must_use]
    pub const fn retryable(self) -> bool {
        true
    }
}

/// What admission decided.
#[derive(Debug)]
pub enum AdmissionOutcome {
    /// Admitted, holding every permit the activation needs.
    Admitted(Vec<Reservation>),
    /// Not now. The local delivery is released with visibility zero; the durable wake is
    /// untouched, so the work reappears rather than waiting out a timeout.
    Deferred {
        /// How long before the delivery becomes visible again.
        requeue_after: core::time::Duration,
    },
    /// Refused, with a typed retryable reason.
    Shed(TypedOverload),
}

/// The admission controller.
#[derive(Debug)]
pub struct Admission {
    bounds: AdmissionBounds,
    permits: Arc<PermitSet>,
    drain: Arc<DrainGate>,
    resources: ActivationResources,
    pressure: Arc<PressureGate>,
}

impl Admission {
    /// Builds a controller over `permits`, minting the pressure gate it consults.
    ///
    /// The gate is minted here rather than injected so that no admission controller can exist
    /// without one. The sampler and the health responder take it from
    /// [`Admission::pressure`]; a second gate would be a second answer to the same question.
    #[must_use]
    pub fn new(
        bounds: AdmissionBounds,
        permits: Arc<PermitSet>,
        drain: Arc<DrainGate>,
        resources: ActivationResources,
    ) -> Self {
        Self {
            bounds,
            permits,
            drain,
            resources,
            pressure: Arc::new(PressureGate::new()),
        }
    }

    /// The declared bands.
    #[must_use]
    pub const fn bounds(&self) -> AdmissionBounds {
        self.bounds
    }

    /// The measured-memory-pressure gate this controller consults.
    #[must_use]
    pub fn pressure(&self) -> &Arc<PressureGate> {
        &self.pressure
    }

    /// The exact resources every admitted activation holds.
    #[must_use]
    pub const fn resources(&self) -> ActivationResources {
        self.resources
    }

    /// How many activations are running.
    #[must_use]
    pub fn active(&self) -> u32 {
        u32::try_from(self.permits.held(PermitKind::Activation)).unwrap_or(u32::MAX)
    }

    /// Whether the task should keep pulling from the queue.
    ///
    /// Above the target it stops: the local queue still drains, but a task that keeps
    /// receiving while it cannot start anything simply hides work in its own memory where
    /// no other task can take it.
    ///
    /// Measured memory pressure stops it for the same reason and with the same effect: the
    /// durable wake stays on the queue, visible to the other task in the placement, and
    /// nothing already delivered is abandoned. Pressure deliberately gates *receipt* only. A
    /// delivery this task has already taken is invisible to every other task until its
    /// visibility timeout expires, so refusing to admit one would hide work rather than shed
    /// it — which is the failure the whole module is written to avoid.
    #[must_use]
    pub fn should_receive(&self) -> bool {
        !self.drain.is_draining()
            && !self.pressure.stops_receiving()
            && self.active() < self.bounds.target
            && self.permits.can_acquire_many(&self.resources.requests())
    }

    /// Decides whether one activation may start.
    #[must_use]
    pub fn admit(&self) -> AdmissionOutcome {
        if self.drain.is_draining() {
            return AdmissionOutcome::Shed(TypedOverload::Draining);
        }
        let active = self.active();
        if active >= self.bounds.safety_cap {
            return AdmissionOutcome::Shed(TypedOverload::AtSafetyCap {
                active,
                cap: self.bounds.safety_cap,
            });
        }
        let held = match self.permits.acquire_many(&self.resources.requests()) {
            Ok(reservations) => reservations,
            // Memory is deferred rather than shed: the bytes will be free again shortly
            // and the work is admissible, unlike an activation over the safety cap.
            Err(full)
                if matches!(
                    full.kind,
                    PermitKind::ContextBytes | PermitKind::StreamBufferBytes
                ) =>
            {
                return AdmissionOutcome::Deferred {
                    requeue_after: core::time::Duration::from_millis(250),
                };
            }
            Err(full) => return AdmissionOutcome::Shed(shed(&full)),
        };
        if active >= self.bounds.target {
            // Between target and cap the task still finishes what it has locally, so an
            // admitted activation keeps its permits and runs.
            return AdmissionOutcome::Admitted(held);
        }
        AdmissionOutcome::Admitted(held)
    }
}

const fn shed(full: &PermitSetFull) -> TypedOverload {
    TypedOverload::ResourceExhausted {
        kind: full.kind,
        requested: full.requested,
        available: full.available,
    }
}

#[cfg(test)]
mod tests {
    use super::{ActivationResources, Admission, AdmissionBounds, AdmissionOutcome, TypedOverload};
    use crate::pressure::{MemoryReading, MemorySource, PressureState};
    use aex_brain_application::kernel::{DrainGate, PermitKind, PermitSet};
    use std::collections::BTreeMap;
    use std::sync::Arc;

    /// A reading of exactly `percent` of a declared limit.
    fn measured_at(percent: u32) -> MemoryReading {
        MemoryReading::Measured {
            source: MemorySource::CgroupV2,
            current_bytes: u64::from(percent),
            limit_bytes: 100,
        }
    }

    fn resources(context_bytes: u64) -> ActivationResources {
        ActivationResources {
            context_bytes,
            stream_buffer_bytes: 1,
            provider_streams: 1,
            hands_rpcs: 1,
        }
    }

    fn admission(activations: u64, bytes: u64, bounds: AdmissionBounds) -> Admission {
        let context_per_activation = if bytes == 0 { 1 } else { bytes };
        let context_pool = if bytes == 0 { activations } else { bytes };
        Admission::new(
            bounds,
            Arc::new(PermitSet::new(BTreeMap::from([
                (PermitKind::Activation, activations),
                (PermitKind::ContextBytes, context_pool),
                (PermitKind::StreamBufferBytes, activations),
                (PermitKind::ProviderStream, activations),
                (PermitKind::HandsRpc, activations),
            ]))),
            Arc::new(DrainGate::new()),
            resources(context_per_activation),
        )
    }

    fn bounds() -> AdmissionBounds {
        AdmissionBounds {
            target: 2,
            safety_cap: 3,
            offered_ceiling: 8,
        }
    }

    /// The approved first-alpha profile: 16 active per task, and the derived bands the
    /// composition root computes from it.
    #[test]
    fn the_launch_bands_are_the_approved_alpha_profile() {
        let default = AdmissionBounds::default();
        assert_eq!(default.target, 16);
        assert_eq!(default.safety_cap, 32);
        assert_eq!(default.offered_ceiling, 80);
        assert!(default.validate().is_ok());
    }

    /// A cap below its target would shed everything the target admitted, which is worse
    /// than either bound alone.
    #[test]
    fn inverted_bands_are_refused_at_startup() {
        assert!(
            AdmissionBounds {
                target: 200,
                safety_cap: 100,
                offered_ceiling: 500
            }
            .validate()
            .is_err()
        );
        assert!(
            AdmissionBounds {
                target: 100,
                safety_cap: 600,
                offered_ceiling: 500
            }
            .validate()
            .is_err()
        );
        assert!(
            AdmissionBounds {
                target: 0,
                safety_cap: 1,
                offered_ceiling: 1
            }
            .validate()
            .is_err()
        );
    }

    /// Between target and safety cap the task stops pulling from the queue. A task that
    /// keeps receiving while it cannot start anything hides work in its own memory where no
    /// other task can take it.
    #[test]
    fn receiving_stops_at_the_target_but_admission_continues_to_the_cap() {
        let admission = admission(4, 0, bounds());
        let first = admission.admit();
        let second = admission.admit();
        assert!(matches!(first, AdmissionOutcome::Admitted(_)));
        assert!(matches!(second, AdmissionOutcome::Admitted(_)));
        assert!(!admission.should_receive(), "at the target, stop receiving");
        assert!(
            matches!(admission.admit(), AdmissionOutcome::Admitted(_)),
            "local work still starts between target and cap"
        );
    }

    /// Above the safety cap the refusal is typed and names the numbers, so an operator can
    /// see the cap rather than infer it.
    #[test]
    fn the_safety_cap_sheds_with_a_typed_retryable_reason() {
        let admission = admission(8, 0, bounds());
        let mut held = Vec::new();
        for _ in 0..3 {
            let AdmissionOutcome::Admitted(permits) = admission.admit() else {
                panic!("under the cap");
            };
            held.push(permits);
        }
        let outcome = admission.admit();
        let AdmissionOutcome::Shed(overload) = outcome else {
            panic!("expected a shed at the cap");
        };
        assert_eq!(overload, TypedOverload::AtSafetyCap { active: 3, cap: 3 });
        assert!(overload.retryable(), "shedding is never a verdict");
    }

    /// Memory pressure defers rather than sheds: the bytes will be free shortly and the
    /// work is admissible, unlike an activation over the safety cap.
    #[test]
    fn exhausted_context_memory_defers_rather_than_sheds() {
        let admission = admission(8, 1_024, bounds());
        let _first = admission.admit();
        assert!(matches!(
            admission.admit(),
            AdmissionOutcome::Deferred { .. }
        ));
    }

    /// A deferral must return the activation permit it took before the byte reservation
    /// failed, or an overloaded task would shrink itself every time memory was tight.
    #[test]
    fn a_deferral_leaks_no_permit() {
        let admission = admission(8, 1_024, bounds());
        let held = admission.admit();
        assert!(matches!(held, AdmissionOutcome::Admitted(_)));
        drop(admission.admit());
        assert_eq!(
            admission.active(),
            1,
            "only the admitted activation still holds a permit"
        );
    }

    /// Multi-resource admission is one transaction over local counters. A late resource
    /// failure must not consume activation or memory capacity and slowly shrink the task.
    #[test]
    fn one_exhausted_resource_leaks_no_other_reservation() {
        let permits = Arc::new(PermitSet::new(BTreeMap::from([
            (PermitKind::Activation, 2_u64),
            (PermitKind::ContextBytes, 2_u64),
            (PermitKind::StreamBufferBytes, 0_u64),
            (PermitKind::ProviderStream, 2_u64),
            (PermitKind::HandsRpc, 2_u64),
        ])));
        let admission = Admission::new(
            bounds(),
            Arc::clone(&permits),
            Arc::new(DrainGate::new()),
            resources(1),
        );

        assert!(!admission.should_receive());
        assert!(matches!(
            admission.admit(),
            AdmissionOutcome::Deferred { .. }
        ));
        for kind in [
            PermitKind::Activation,
            PermitKind::ContextBytes,
            PermitKind::StreamBufferBytes,
            PermitKind::ProviderStream,
            PermitKind::HandsRpc,
        ] {
            assert_eq!(permits.held(kind), 0, "{kind:?} leaked");
        }
    }

    /// Restore resources stay held until the RAII bundle drops; dispatch lanes remain free.
    #[test]
    fn admitted_activation_holds_only_the_restore_resource_bundle() {
        let permits = Arc::new(PermitSet::new(BTreeMap::from([
            (PermitKind::Activation, 1_u64),
            (PermitKind::ContextBytes, 64_u64),
            (PermitKind::StreamBufferBytes, 8_u64),
            (PermitKind::ProviderStream, 1_u64),
            (PermitKind::HandsRpc, 1_u64),
        ])));
        let admission = Admission::new(
            AdmissionBounds {
                target: 1,
                safety_cap: 1,
                offered_ceiling: 1,
            },
            Arc::clone(&permits),
            Arc::new(DrainGate::new()),
            ActivationResources {
                context_bytes: 64,
                stream_buffer_bytes: 8,
                provider_streams: 1,
                hands_rpcs: 1,
            },
        );
        let AdmissionOutcome::Admitted(held) = admission.admit() else {
            panic!("the exact resource bundle is available");
        };
        for (kind, units) in [
            (PermitKind::Activation, 1),
            (PermitKind::ContextBytes, 64),
            (PermitKind::StreamBufferBytes, 8),
            (PermitKind::ProviderStream, 0),
            (PermitKind::HandsRpc, 0),
        ] {
            assert_eq!(permits.held(kind), units, "{kind:?} was not reserved");
        }
        drop(held);
        for kind in [
            PermitKind::Activation,
            PermitKind::ContextBytes,
            PermitKind::StreamBufferBytes,
            PermitKind::ProviderStream,
            PermitKind::HandsRpc,
        ] {
            assert_eq!(permits.held(kind), 0, "{kind:?} did not release");
        }
    }

    #[test]
    fn a_draining_task_admits_nothing_and_receives_nothing() {
        let permits = Arc::new(PermitSet::new(BTreeMap::from([
            (PermitKind::Activation, 8_u64),
            (PermitKind::ContextBytes, 1_u64),
            (PermitKind::StreamBufferBytes, 1_u64),
            (PermitKind::ProviderStream, 1_u64),
            (PermitKind::HandsRpc, 1_u64),
        ])));
        let drain = Arc::new(DrainGate::new());
        let admission = Admission::new(
            bounds(),
            Arc::clone(&permits),
            Arc::clone(&drain),
            resources(1),
        );
        assert!(admission.should_receive());
        drain.start_drain();
        assert!(!admission.should_receive());
        assert!(matches!(
            admission.admit(),
            AdmissionOutcome::Shed(TypedOverload::Draining)
        ));
    }

    /// The 80 % watermark stops receipt and nothing else. The task keeps its permits, keeps
    /// starting the deliveries it already holds, and is not draining: a wake left on the queue
    /// is visible to the other task in the placement, while an activation killed here would
    /// leave an effect nobody can settle.
    #[test]
    fn measured_pressure_stops_receipt_and_leaves_admitted_work_running() {
        let admission = admission(8, 0, bounds());
        let AdmissionOutcome::Admitted(held) = admission.admit() else {
            panic!("nothing is under pressure yet");
        };
        assert!(admission.should_receive());

        let _ = admission
            .pressure()
            .observe(measured_at(PressureState::AdmissionStop.entry_percent()));
        assert!(
            !admission.should_receive(),
            "receipt stops at the admission watermark"
        );
        assert_eq!(
            admission.active(),
            1,
            "an admitted activation keeps its permits under pressure"
        );
        assert!(
            matches!(admission.admit(), AdmissionOutcome::Admitted(_)),
            "a delivery already taken is still started"
        );
        drop(held);
    }

    /// Critical pressure is reported and acted on exactly like the stop below it. It never
    /// terminates the process and never abandons an activation: a Brain task at 90 % is
    /// usually a task holding a provider call it cannot prove was not sent.
    #[test]
    fn critical_pressure_stops_receipt_without_abandoning_anything() {
        let admission = admission(8, 0, bounds());
        let AdmissionOutcome::Admitted(held) = admission.admit() else {
            panic!("nothing is under pressure yet");
        };

        let _ = admission
            .pressure()
            .observe(measured_at(PressureState::Critical.entry_percent()));
        assert_eq!(admission.pressure().state(), PressureState::Critical);
        assert!(!admission.should_receive());
        assert_eq!(admission.active(), 1);
        assert!(
            !admission.drain.is_draining(),
            "pressure never starts a drain of its own"
        );
        drop(held);
        assert_eq!(admission.active(), 0, "the activation released normally");
    }

    /// Recovery below the lower threshold puts the task back in service. The hysteresis band
    /// is what stops one completing activation reopening receipt into the same pressure.
    #[test]
    fn receipt_resumes_only_once_the_measurement_is_below_the_recovery_threshold() {
        let admission = admission(8, 0, bounds());
        let _ = admission
            .pressure()
            .observe(measured_at(PressureState::AdmissionStop.entry_percent()));
        assert!(!admission.should_receive());

        let _ = admission
            .pressure()
            .observe(measured_at(PressureState::AdmissionStop.recovery_percent()));
        assert!(
            !admission.should_receive(),
            "at the recovery threshold is not below it"
        );

        let _ = admission.pressure().observe(measured_at(
            PressureState::AdmissionStop.recovery_percent() - 1,
        ));
        assert!(admission.should_receive());
    }

    /// A host that declares no memory limit — every development machine, and any deployment
    /// whose cgroup files move — leaves the conservative activation cap as the only bound, and
    /// that cap still admits exactly the approved 16 before it stops receiving.
    #[test]
    fn a_task_that_cannot_measure_memory_still_admits_no_more_than_the_approved_target() {
        let bounds = AdmissionBounds::default();
        let admission = admission(u64::from(bounds.safety_cap), 0, bounds);
        let _ = admission.pressure().observe(MemoryReading::Unreadable);
        assert_eq!(admission.pressure().state(), PressureState::Normal);

        let mut held = Vec::new();
        while admission.should_receive() {
            let AdmissionOutcome::Admitted(permits) = admission.admit() else {
                panic!("under the target");
            };
            held.push(permits);
        }
        assert_eq!(held.len(), 16);
        assert_eq!(admission.active(), bounds.target);
    }

    /// The offered ceiling is survived without loss: every offer ends admitted, deferred or
    /// typed-shed, and nothing is dropped.
    #[test]
    fn the_offered_ceiling_produces_only_typed_outcomes() {
        let bounds = AdmissionBounds::default();
        let admission = admission(u64::from(bounds.safety_cap), 0, bounds);
        let mut admitted = 0_u32;
        let mut shed = 0_u32;
        let mut held = Vec::new();
        for _ in 0..bounds.offered_ceiling {
            match admission.admit() {
                AdmissionOutcome::Admitted(permits) => {
                    admitted += 1;
                    held.push(permits);
                }
                AdmissionOutcome::Shed(overload) => {
                    assert!(overload.retryable());
                    shed += 1;
                }
                AdmissionOutcome::Deferred { .. } => panic!("no memory pressure here"),
            }
        }
        assert_eq!(admitted + shed, bounds.offered_ceiling);
        assert_eq!(admitted, bounds.safety_cap);
    }
}
