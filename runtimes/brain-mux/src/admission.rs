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

use aex_brain_application::kernel::{DrainGate, PermitKind, PermitSet, PermitSetFull, Reservation};
use std::sync::Arc;

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
    /// The launch bands. Slice 11's load gates are the sizing authority; these are the
    /// starting point those gates confirm or replace.
    fn default() -> Self {
        Self {
            target: 100,
            safety_cap: 200,
            offered_ceiling: 500,
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
}

impl Admission {
    /// Builds a controller over `permits`.
    #[must_use]
    pub const fn new(
        bounds: AdmissionBounds,
        permits: Arc<PermitSet>,
        drain: Arc<DrainGate>,
    ) -> Self {
        Self {
            bounds,
            permits,
            drain,
        }
    }

    /// The declared bands.
    #[must_use]
    pub const fn bounds(&self) -> AdmissionBounds {
        self.bounds
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
    #[must_use]
    pub fn should_receive(&self) -> bool {
        !self.drain.is_draining() && self.active() < self.bounds.target
    }

    /// Decides whether one activation may start.
    #[must_use]
    pub fn admit(&self, context_bytes: u64) -> AdmissionOutcome {
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
        let mut held = Vec::with_capacity(2);
        match self.permits.acquire(PermitKind::Activation, 1) {
            Ok(reservation) => held.push(reservation),
            Err(full) => return AdmissionOutcome::Shed(shed(&full)),
        }
        if context_bytes > 0 {
            match self
                .permits
                .acquire(PermitKind::ContextBytes, context_bytes)
            {
                Ok(reservation) => held.push(reservation),
                // Memory is deferred rather than shed: the bytes will be free again shortly
                // and the work is admissible, unlike an activation over the safety cap.
                Err(_) => {
                    return AdmissionOutcome::Deferred {
                        requeue_after: core::time::Duration::from_millis(250),
                    };
                }
            }
        }
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
    use super::{Admission, AdmissionBounds, AdmissionOutcome, TypedOverload};
    use aex_brain_application::kernel::{DrainGate, PermitKind, PermitSet};
    use std::collections::BTreeMap;
    use std::sync::Arc;

    fn admission(activations: u64, bytes: u64, bounds: AdmissionBounds) -> Admission {
        Admission::new(
            bounds,
            Arc::new(PermitSet::new(BTreeMap::from([
                (PermitKind::Activation, activations),
                (PermitKind::ContextBytes, bytes),
            ]))),
            Arc::new(DrainGate::new()),
        )
    }

    fn bounds() -> AdmissionBounds {
        AdmissionBounds {
            target: 2,
            safety_cap: 3,
            offered_ceiling: 8,
        }
    }

    #[test]
    fn the_launch_bands_are_the_measured_ones() {
        let default = AdmissionBounds::default();
        assert_eq!(default.target, 100);
        assert_eq!(default.safety_cap, 200);
        assert_eq!(default.offered_ceiling, 500);
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
        let first = admission.admit(0);
        let second = admission.admit(0);
        assert!(matches!(first, AdmissionOutcome::Admitted(_)));
        assert!(matches!(second, AdmissionOutcome::Admitted(_)));
        assert!(!admission.should_receive(), "at the target, stop receiving");
        assert!(
            matches!(admission.admit(0), AdmissionOutcome::Admitted(_)),
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
            let AdmissionOutcome::Admitted(permits) = admission.admit(0) else {
                panic!("under the cap");
            };
            held.push(permits);
        }
        let outcome = admission.admit(0);
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
        let _first = admission.admit(1_024);
        assert!(matches!(
            admission.admit(1_024),
            AdmissionOutcome::Deferred { .. }
        ));
    }

    /// A deferral must return the activation permit it took before the byte reservation
    /// failed, or an overloaded task would shrink itself every time memory was tight.
    #[test]
    fn a_deferral_leaks_no_permit() {
        let admission = admission(8, 1_024, bounds());
        let held = admission.admit(1_024);
        assert!(matches!(held, AdmissionOutcome::Admitted(_)));
        drop(admission.admit(1_024));
        assert_eq!(
            admission.active(),
            1,
            "only the admitted activation still holds a permit"
        );
    }

    #[test]
    fn a_draining_task_admits_nothing_and_receives_nothing() {
        let permits = Arc::new(PermitSet::new(BTreeMap::from([(
            PermitKind::Activation,
            8_u64,
        )])));
        let drain = Arc::new(DrainGate::new());
        let admission = Admission::new(bounds(), Arc::clone(&permits), Arc::clone(&drain));
        assert!(admission.should_receive());
        drain.start_drain();
        assert!(!admission.should_receive());
        assert!(matches!(
            admission.admit(0),
            AdmissionOutcome::Shed(TypedOverload::Draining)
        ));
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
            match admission.admit(0) {
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
