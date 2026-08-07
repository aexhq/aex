//! Memory reservation tokens.
//!
//! Memory is billed from an explicit reservation, never an RSS share. The token
//! is RAII: holding it *is* the measured interval, and dropping it closes that
//! interval and emits one `memory.byte_ms.v1` fact. A resize closes the current
//! interval and opens a new one, so the integral over a resized reservation
//! equals the integral over the equivalent fixed sequence exactly.
//!
//! Base executable memory, shared pools, code pages and allocator fragmentation
//! are platform overhead and have no fact.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Weak};
use std::time::Instant;

use aex_usage_domain::fact::{Attribution, FactDraft, FactKind, SCHEMA_VERSION};
use aex_usage_domain::identity::{AuthorityId, AuthorityKey, AuthorityKind, SegmentOrdinal};
use aex_usage_domain::measurement::{
    Evidence, FactBasis, Measurement, ReceiptKind, ReservationClass, ServiceTime, SourceReceipt,
};
use aex_usage_domain::meter::{Category, Meter};
use aex_usage_domain::quantity::Quantity;

use super::clock::WallClock;
use super::sink::BoundedFactSink;
use super::{ProbeContext, ProbeError, to_timestamp};

/// What one reconciliation compared.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MemoryReport {
    /// Bytes currently reserved through live tokens.
    pub live_bytes: u64,
    /// What the cgroup reports the whole task using.
    pub physical_bytes: u64,
    /// The envelope reservations are drawn from.
    pub envelope_bytes: u64,
    /// The headroom held back from the envelope.
    pub headroom_bytes: u64,
    /// Whether reserved bytes exceeded what the task physically holds, which is
    /// an accounting defect rather than a billable state.
    pub over_reserved: bool,
}

/// A bounded memory envelope reservations are drawn from.
#[derive(Debug)]
pub struct MemoryBudget {
    envelope_bytes: u64,
    headroom_bytes: u64,
    live_bytes: AtomicU64,
    next_ordinal: AtomicU64,
    sink: Arc<BoundedFactSink>,
    clock: Arc<dyn WallClock>,
}

impl MemoryBudget {
    /// Builds a budget over an explicit envelope.
    ///
    /// # Errors
    ///
    /// Returns [`ProbeError::Configuration`] when the headroom is not strictly
    /// inside the envelope: a budget that cannot grant anything is a
    /// misconfiguration, not a runtime condition to discover under load.
    pub fn new(
        envelope_bytes: u64,
        headroom_bytes: u64,
        sink: Arc<BoundedFactSink>,
        clock: Arc<dyn WallClock>,
    ) -> Result<Arc<Self>, ProbeError> {
        if headroom_bytes >= envelope_bytes {
            return Err(ProbeError::Configuration {
                what: "memory budget",
                reason: format!(
                    "headroom {headroom_bytes} leaves nothing inside envelope {envelope_bytes}"
                ),
            });
        }
        Ok(Arc::new(Self {
            envelope_bytes,
            headroom_bytes,
            live_bytes: AtomicU64::new(0),
            next_ordinal: AtomicU64::new(0),
            sink,
            clock,
        }))
    }

    /// Bytes currently held by live reservations.
    #[must_use]
    pub fn live_bytes(&self) -> u64 {
        self.live_bytes.load(Ordering::Acquire)
    }

    /// The grantable ceiling: the envelope less the reserved headroom.
    #[must_use]
    pub const fn grantable_bytes(&self) -> u64 {
        self.envelope_bytes - self.headroom_bytes
    }

    /// Reserves `bytes` without waiting.
    ///
    /// # Errors
    ///
    /// Returns [`ProbeError::BudgetExhausted`] when the grant would over-commit
    /// the envelope. The budget never queues unboundedly and never over-commits.
    pub fn try_reserve(
        self: &Arc<Self>,
        context: &ProbeContext,
        attribution: &Attribution,
        class: ReservationClass,
        bytes: u64,
    ) -> Result<MemoryReservation, ProbeError> {
        let grantable = self.grantable_bytes();
        let mut current = self.live_bytes.load(Ordering::Acquire);
        loop {
            let next = current
                .checked_add(bytes)
                .ok_or(ProbeError::BudgetExhausted {
                    requested: bytes,
                    live: current,
                    grantable,
                })?;
            if next > grantable {
                return Err(ProbeError::BudgetExhausted {
                    requested: bytes,
                    live: current,
                    grantable,
                });
            }
            match self.live_bytes.compare_exchange_weak(
                current,
                next,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => break,
                Err(observed) => current = observed,
            }
        }

        Ok(MemoryReservation {
            budget: Arc::downgrade(self),
            context: context.clone(),
            attribution: attribution.clone(),
            class,
            bytes,
            acquired_at: self.clock.instant(),
            acquired_wall: self.clock.now(),
            released: false,
        })
    }

    /// Asserts the budget against the physical reading.
    ///
    /// A violation is an alarm plus pressure escalation, never a silent
    /// overcharge: reserving more than the task physically holds means the
    /// accounting is wrong, and the honest response is to say so.
    ///
    /// # Errors
    ///
    /// Returns [`ProbeError::PhysicalSource`] when the reading is unavailable.
    pub fn reconcile(
        &self,
        physical: &dyn super::clock::PhysicalMemorySource,
    ) -> Result<MemoryReport, ProbeError> {
        let physical_bytes = physical.task_memory_current()?;
        let live_bytes = self.live_bytes();
        Ok(MemoryReport {
            live_bytes,
            physical_bytes,
            envelope_bytes: self.envelope_bytes,
            headroom_bytes: self.headroom_bytes,
            over_reserved: live_bytes > physical_bytes,
        })
    }

    /// Closes one held interval, emitting exactly one fact.
    ///
    /// The interval end is derived from `held_ms` rather than read from the wall
    /// clock a second time, so the emitted `[start, end)` and the billed
    /// `bytes x held_ms` can never disagree by a scheduling delay.
    ///
    /// This does **not** adjust `live_bytes`; the caller owns that, because a
    /// resize closes an interval without releasing the grant.
    fn close(
        &self,
        context: &ProbeContext,
        attribution: &Attribution,
        class: ReservationClass,
        bytes: u64,
        held_ms: u64,
        start: time::OffsetDateTime,
    ) -> Result<Quantity, ProbeError> {
        let ordinal = self.next_ordinal.fetch_add(1, Ordering::Relaxed);
        let start_stamp = to_timestamp(start)?;
        let end_stamp = start_stamp.plus_millis(held_ms)?;
        let authority_id = AuthorityId::parse(&format!("{}:{ordinal}", class.id()))?;
        let measurement = Measurement::new(
            Meter::MemoryByteMs,
            FactBasis::Reserved,
            ServiceTime::Interval {
                start: start_stamp,
                end: end_stamp,
            },
            SourceReceipt {
                kind: ReceiptKind::ReservationToken,
                id: Box::from(authority_id.as_str()),
                digest: None,
            },
            Evidence::Reservation {
                class,
                bytes,
                held_ms,
            },
        )?;
        let quantity = measurement.quantity();
        let draft = FactDraft {
            schema_version: SCHEMA_VERSION,
            organization: context.organization.clone(),
            workspace: context.workspace.clone(),
            region: context.region.clone(),
            attribution: attribution.clone(),
            service: context.service.clone(),
            resource: context.resource.clone(),
            authority: AuthorityKey {
                region: context.region.clone(),
                category: Category::Compute,
                kind: AuthorityKind::MemoryReservation,
                authority_id,
                segment_ordinal: SegmentOrdinal::new(ordinal),
            },
            pricing_version: context.pricing_version.clone(),
            reservation: context.reservation.clone(),
            kind: FactKind::Measured(measurement),
        };
        // A `Drop` cannot propagate, so a refused offer parks rather than
        // vanishes; the drain refuses to finish while the ledger is non-empty.
        self.sink.offer_or_park(draft);
        Ok(quantity)
    }
}

/// A held memory reservation. Holding it is the measured interval.
#[must_use = "a MemoryReservation must be held for the measured interval; \
              binding it to `_` releases it immediately"]
#[derive(Debug)]
pub struct MemoryReservation {
    budget: Weak<MemoryBudget>,
    context: ProbeContext,
    attribution: Attribution,
    class: ReservationClass,
    bytes: u64,
    acquired_at: Instant,
    acquired_wall: time::OffsetDateTime,
    released: bool,
}

impl MemoryReservation {
    /// Bytes currently reserved by this token.
    #[must_use]
    pub const fn bytes(&self) -> u64 {
        self.bytes
    }

    /// What the reservation is held for.
    #[must_use]
    pub const fn class(&self) -> ReservationClass {
        self.class
    }

    /// Closes the reservation explicitly and returns the emitted quantity.
    ///
    /// Equivalent to `Drop`, but observable — which is what lets a test assert
    /// the exact integral rather than infer it.
    ///
    /// # Errors
    ///
    /// Returns [`ProbeError::BudgetGone`] when the budget has already been torn
    /// down, and [`ProbeError::Measurement`] when the closed interval fails its
    /// own construction rules.
    pub fn release(mut self) -> Result<Quantity, ProbeError> {
        let budget = self.budget.upgrade().ok_or(ProbeError::BudgetGone)?;
        let quantity = self.close_at_size(&budget, self.bytes)?;
        budget.live_bytes.fetch_sub(self.bytes, Ordering::AcqRel);
        self.released = true;
        Ok(quantity)
    }

    /// Closes the current interval and opens a new one at `bytes`.
    ///
    /// This is what keeps the byte-millisecond integral exact across a resize
    /// instead of retroactively restating the whole hold at the new size.
    ///
    /// # Errors
    ///
    /// Returns [`ProbeError::BudgetGone`] when the budget is gone and
    /// [`ProbeError::BudgetExhausted`] when growing would over-commit it.
    pub fn resize(&mut self, bytes: u64) -> Result<Quantity, ProbeError> {
        let budget = self.budget.upgrade().ok_or(ProbeError::BudgetGone)?;

        // Admission first: a refused resize must leave the token untouched, so
        // the grant is only moved once the new size is known to fit.
        if bytes > self.bytes {
            let growth = bytes - self.bytes;
            let grantable = budget.grantable_bytes();
            let live = budget.live_bytes();
            if live.saturating_add(growth) > grantable {
                return Err(ProbeError::BudgetExhausted {
                    requested: growth,
                    live,
                    grantable,
                });
            }
        }

        // Close the old interval at the old size, then reopen at the new one.
        let closed = self.close_at_size(&budget, self.bytes)?;
        if bytes > self.bytes {
            budget
                .live_bytes
                .fetch_add(bytes - self.bytes, Ordering::AcqRel);
        } else {
            budget
                .live_bytes
                .fetch_sub(self.bytes - bytes, Ordering::AcqRel);
        }
        self.bytes = bytes;
        self.acquired_at = budget.clock.instant();
        self.acquired_wall = budget.clock.now();
        Ok(closed)
    }

    /// Closes the interval currently open at `bytes`, leaving `live_bytes` to
    /// the caller: a resize closes an interval without releasing the grant.
    fn close_at_size(
        &self,
        budget: &Arc<MemoryBudget>,
        bytes: u64,
    ) -> Result<Quantity, ProbeError> {
        let held_ms = u64::try_from(
            budget
                .clock
                .instant()
                .saturating_duration_since(self.acquired_at)
                .as_millis(),
        )
        .unwrap_or(u64::MAX);
        budget.close(
            &self.context,
            &self.attribution,
            self.class,
            bytes,
            held_ms,
            self.acquired_wall,
        )
    }
}

impl Drop for MemoryReservation {
    fn drop(&mut self) {
        if self.released {
            return;
        }
        // A failure here cannot be propagated. The draft is parked by
        // `offer_or_park` inside `close`, and a budget that is already gone
        // means the whole process is shutting down.
        if let Some(budget) = self.budget.upgrade() {
            let _ = self.close_at_size(&budget, self.bytes);
            budget.live_bytes.fetch_sub(self.bytes, Ordering::AcqRel);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{MemoryBudget, MemoryReservation};
    use crate::probe::sink::{BoundedFactSink, OverflowLedger};
    use crate::probe::testing::{SteppingClock, probe_context};
    use crate::probe::{ProbeError, WallClock};
    use aex_usage_domain::fact::{Attribution, FactKind};
    use aex_usage_domain::measurement::{Evidence, ReservationClass};
    use std::sync::Arc;

    const MIB: u64 = 1024 * 1024;

    fn budget(clock: Arc<dyn WallClock>) -> (Arc<MemoryBudget>, Arc<BoundedFactSink>) {
        let sink =
            Arc::new(BoundedFactSink::new(64, Arc::new(OverflowLedger::new())).expect("capacity"));
        let budget =
            MemoryBudget::new(64 * MIB, 8 * MIB, Arc::clone(&sink), clock).expect("budget");
        (budget, sink)
    }

    fn reserve(
        budget: &Arc<MemoryBudget>,
        class: ReservationClass,
        bytes: u64,
    ) -> Result<MemoryReservation, ProbeError> {
        budget.try_reserve(&probe_context(), &Attribution::default(), class, bytes)
    }

    #[test]
    fn a_headroom_that_swallows_the_envelope_is_refused_at_construction() {
        let sink =
            Arc::new(BoundedFactSink::new(4, Arc::new(OverflowLedger::new())).expect("capacity"));
        let clock: Arc<dyn WallClock> = SteppingClock::new(0, 0);
        assert!(MemoryBudget::new(8 * MIB, 8 * MIB, sink, clock).is_err());
    }

    #[test]
    fn the_envelope_is_never_over_committed() {
        let clock: Arc<dyn WallClock> = SteppingClock::new(0, 0);
        let (budget, _sink) = budget(clock);
        assert_eq!(budget.grantable_bytes(), 56 * MIB);

        let held = reserve(&budget, ReservationClass::Context, 50 * MIB).expect("fits");
        assert_eq!(budget.live_bytes(), 50 * MIB);

        let refused = reserve(&budget, ReservationClass::Result, 10 * MIB);
        assert!(matches!(refused, Err(ProbeError::BudgetExhausted { .. })));

        drop(held);
        assert_eq!(budget.live_bytes(), 0, "release returns the whole grant");
    }

    #[test]
    fn releasing_emits_exactly_one_fact_with_the_exact_integral() {
        // 250 ms of hold at 4 MiB.
        let clock: Arc<dyn WallClock> = SteppingClock::new(0, 250);
        let (budget, sink) = budget(clock);

        let held = reserve(&budget, ReservationClass::ParserBuffer, 4 * MIB).expect("fits");
        let quantity = held.release().expect("releases");
        assert_eq!(quantity.get(), u128::from(4 * MIB) * 250);

        let batch = sink.take_batch(64);
        assert_eq!(batch.len(), 1, "exactly one closed fact");
        let FactKind::Measured(measurement) = &batch[0].kind else {
            panic!("a reservation close is a measured fact");
        };
        assert_eq!(measurement.quantity().get(), u128::from(4 * MIB) * 250);
        match measurement.evidence() {
            Evidence::Reservation {
                class,
                bytes,
                held_ms,
            } => {
                assert_eq!(*class, ReservationClass::ParserBuffer);
                assert_eq!(*bytes, 4 * MIB);
                assert_eq!(*held_ms, 250);
            }
            other => panic!("expected reservation evidence, got {other:?}"),
        }
    }

    #[test]
    fn dropping_a_reservation_closes_its_interval_exactly_once() {
        let clock: Arc<dyn WallClock> = SteppingClock::new(0, 100);
        let (budget, sink) = budget(clock);
        {
            let _held = reserve(&budget, ReservationClass::WarmCacheEntry, MIB).expect("fits");
        }
        assert_eq!(sink.take_batch(64).len(), 1);
        assert_eq!(budget.live_bytes(), 0);
    }

    #[test]
    fn a_resize_closes_one_interval_and_opens_another_so_the_integral_stays_exact() {
        // Each clock read advances 100 ms.
        let clock: Arc<dyn WallClock> = SteppingClock::new(0, 100);
        let (budget, sink) = budget(clock);

        let mut held = reserve(&budget, ReservationClass::Context, MIB).expect("fits");
        let first = held.resize(2 * MIB).expect("grows");
        assert_eq!(budget.live_bytes(), 2 * MIB);
        let second = held.release().expect("releases");

        // Two closed intervals, and the total is the sum of the fixed segments
        // rather than a restatement of the whole hold at the final size.
        let batch = sink.take_batch(64);
        assert_eq!(batch.len(), 2);
        assert_eq!(
            first.get() + second.get(),
            u128::from(MIB) * 100 + u128::from(2 * MIB) * 100
        );
        assert_eq!(budget.live_bytes(), 0);
    }

    #[test]
    fn a_resize_that_would_over_commit_the_envelope_is_refused() {
        let clock: Arc<dyn WallClock> = SteppingClock::new(0, 10);
        let (budget, _sink) = budget(clock);
        let mut held = reserve(&budget, ReservationClass::Context, 50 * MIB).expect("fits");
        assert!(matches!(
            held.resize(60 * MIB),
            Err(ProbeError::BudgetExhausted { .. })
        ));
        assert_eq!(held.bytes(), 50 * MIB, "a refused resize changes nothing");
    }

    #[test]
    fn a_zero_length_hold_integrates_to_zero_and_still_emits_a_fact() {
        // TTL-zero on a warm cache entry: the reservation is dropped at the
        // instant it is taken, which closes its interval at that instant.
        let clock: Arc<dyn WallClock> = SteppingClock::new(0, 0);
        let (budget, sink) = budget(clock);
        let held = reserve(&budget, ReservationClass::WarmCacheEntry, 8 * MIB).expect("fits");
        assert_eq!(held.release().expect("releases").get(), 0);
        assert_eq!(sink.take_batch(64).len(), 1);
    }
}
