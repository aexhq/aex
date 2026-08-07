//! Reserved memory, not hoped-for memory.
//!
//! A multi-GB export runs inside a task with a fixed working budget, so the
//! footprint has to be a **reservation** rather than an observation. The whole
//! budget is split into four named reservations — the page buffer, the encoder
//! scratch, one multipart part buffer and one row-group buffer — and every one
//! of them is acquired **before the producing loop starts**.
//!
//! That ordering is the point. A reservation that cannot be met is a typed
//! `export_capacity` refusal the launcher can retry against a larger task,
//! whereas the failure mode it replaces is an OOM kill discovered halfway
//! through a two-hour export, after the customer has already been told the
//! export is generating.
//!
//! Because the four reservations depend only on the configured page limit, part
//! size and row-group size, the total is independent of how many pages the
//! export streams: a one-page export and a ten-thousand-page export hold exactly
//! the same bytes.

use aex_observation_domain::limits::OBSERVATION_NORMALIZED_MAX;
use aex_otlp_admission::wire_pending::PendingErrorCode;
use aex_otlp_admission::{MemoryBudget, MemoryLease};

/// The per-observation ceiling one page slot is sized at.
///
/// A normalized observation can never exceed this, so `page limit x slot` is a
/// true upper bound on one page rather than an average.
pub const PAGE_SLOT_BYTES: usize = OBSERVATION_NORMALIZED_MAX;

/// The encoder's fixed scratch reservation.
///
/// Sixteen per-observation ceilings: the canonical record being fed, the framing
/// around it and the running digest state. It is a constant rather than a
/// fraction of the budget so that raising the task's memory never silently
/// raises what the encoder is allowed to hold.
pub const ENCODER_SCRATCH_BYTES: usize = 16 * PAGE_SLOT_BYTES;

/// The name of the page-buffer reservation.
pub const PAGE_BUFFER: &str = "page_buffer";
/// The name of the encoder-scratch reservation.
pub const ENCODER_SCRATCH: &str = "encoder_scratch";
/// The name of the multipart part-buffer reservation.
pub const PART_BUFFER: &str = "part_buffer";
/// The name of the row-group-buffer reservation.
pub const ROW_GROUP_BUFFER: &str = "row_group_buffer";

/// The wire code a capacity refusal is reported as.
pub const CAPACITY_CODE: &str = PendingErrorCode::ExportCapacity.as_str();

/// Why an export could not reserve the memory it needs.
#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
#[error(
    "the `{reservation}` reservation of {requested} bytes does not fit the {capacity}-byte export budget"
)]
pub struct CapacityError {
    /// Which reservation could not be met.
    pub reservation: &'static str,
    /// How many bytes it asked for.
    pub requested: usize,
    /// The whole working budget.
    pub capacity: usize,
}

impl CapacityError {
    /// The pending wire code every capacity refusal is published as.
    ///
    /// `503 export_capacity` is retryable, so a launcher may re-run the export
    /// against a larger task instead of failing the customer's request.
    pub const CODE: PendingErrorCode = PendingErrorCode::ExportCapacity;
}

/// One named reservation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Reservation {
    /// What the bytes are held for.
    pub name: &'static str,
    /// How many bytes are held.
    pub bytes: usize,
}

/// The whole working budget, split into four explicit reservations.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MemoryPlan {
    reservations: [Reservation; 4],
}

impl MemoryPlan {
    /// How many reservations one export holds.
    pub const RESERVATIONS: usize = 4;

    /// The plan implied by a page limit, a part size and a row-group size.
    #[must_use]
    pub const fn new(page_limit: usize, part_bytes: usize, rowgroup_bytes: usize) -> Self {
        Self {
            reservations: [
                Reservation {
                    name: PAGE_BUFFER,
                    bytes: page_limit.saturating_mul(PAGE_SLOT_BYTES),
                },
                Reservation {
                    name: ENCODER_SCRATCH,
                    bytes: ENCODER_SCRATCH_BYTES,
                },
                Reservation {
                    name: PART_BUFFER,
                    bytes: part_bytes,
                },
                Reservation {
                    name: ROW_GROUP_BUFFER,
                    bytes: rowgroup_bytes,
                },
            ],
        }
    }

    /// Every reservation, in acquisition order.
    #[must_use]
    pub const fn reservations(&self) -> &[Reservation] {
        &self.reservations
    }

    /// The bytes reserved for one page of observations.
    #[must_use]
    pub const fn page_buffer_bytes(&self) -> usize {
        self.reservations[0].bytes
    }

    /// The bytes reserved for encoder scratch.
    #[must_use]
    pub const fn encoder_scratch_bytes(&self) -> usize {
        self.reservations[1].bytes
    }

    /// The bytes reserved for one multipart part.
    #[must_use]
    pub const fn part_buffer_bytes(&self) -> usize {
        self.reservations[2].bytes
    }

    /// The bytes reserved for one row group.
    #[must_use]
    pub const fn rowgroup_buffer_bytes(&self) -> usize {
        self.reservations[3].bytes
    }

    /// Everything the export holds, for the whole export.
    #[must_use]
    pub const fn total_bytes(&self) -> usize {
        let mut total = 0usize;
        let mut index = 0usize;
        while index < self.reservations.len() {
            total = total.saturating_add(self.reservations[index].bytes);
            index += 1;
        }
        total
    }

    /// The plan, spelled out for a refusal message.
    #[must_use]
    pub fn describe(&self) -> String {
        self.reservations
            .iter()
            .map(|reservation| format!("{} {} bytes", reservation.name, reservation.bytes))
            .collect::<Vec<_>>()
            .join(", ")
    }

    /// Acquires every reservation up front.
    ///
    /// # Errors
    ///
    /// Returns [`CapacityError`] naming the first reservation the budget cannot
    /// cover. Nothing is streamed, encoded or uploaded until this succeeds, so a
    /// capacity failure is never discovered as an OOM kill.
    pub fn acquire(&self, budget: &MemoryBudget) -> Result<Reserved, CapacityError> {
        let mut leases = Vec::with_capacity(self.reservations.len());
        for reservation in &self.reservations {
            let lease = budget
                .try_reserve(reservation.bytes)
                .map_err(|_| CapacityError {
                    reservation: reservation.name,
                    requested: reservation.bytes,
                    capacity: budget.capacity(),
                })?;
            leases.push(lease);
        }
        Ok(Reserved {
            leases,
            bytes: self.total_bytes(),
        })
    }
}

/// Proof that every reservation is held for the life of the export.
///
/// Dropping it releases all four at once, which is what keeps a failed export
/// from leaking its budget into the next one inside the same process.
#[derive(Debug)]
pub struct Reserved {
    leases: Vec<MemoryLease>,
    bytes: usize,
}

impl Reserved {
    /// How many bytes are held.
    #[must_use]
    pub const fn bytes(&self) -> usize {
        self.bytes
    }

    /// How many individual reservations are held.
    #[must_use]
    pub fn count(&self) -> usize {
        self.leases.len()
    }

    /// Whether the held leases cover one exact reservation.
    #[must_use]
    pub fn covers(&self, bytes: usize) -> bool {
        self.leases.iter().any(|lease| lease.covers(bytes))
    }
}

#[cfg(test)]
mod tests {
    use aex_otlp_admission::MemoryBudget;
    use aex_otlp_admission::wire_pending::PendingErrorCode;

    use super::{
        CAPACITY_CODE, CapacityError, ENCODER_SCRATCH, ENCODER_SCRATCH_BYTES, MemoryPlan,
        PAGE_BUFFER, PAGE_SLOT_BYTES, PART_BUFFER, ROW_GROUP_BUFFER,
    };

    fn plan() -> MemoryPlan {
        MemoryPlan::new(500, 16 * 1024 * 1024, 64 * 1024 * 1024)
    }

    #[test]
    fn the_four_reservations_are_named_and_sum_to_the_total() {
        let plan = plan();
        let names: Vec<&str> = plan
            .reservations()
            .iter()
            .map(|reservation| reservation.name)
            .collect();
        assert_eq!(
            names,
            vec![PAGE_BUFFER, ENCODER_SCRATCH, PART_BUFFER, ROW_GROUP_BUFFER]
        );
        assert_eq!(plan.page_buffer_bytes(), 500 * PAGE_SLOT_BYTES);
        assert_eq!(plan.encoder_scratch_bytes(), ENCODER_SCRATCH_BYTES);
        assert_eq!(plan.part_buffer_bytes(), 16 * 1024 * 1024);
        assert_eq!(plan.rowgroup_buffer_bytes(), 64 * 1024 * 1024);
        assert_eq!(
            plan.total_bytes(),
            plan.reservations()
                .iter()
                .map(|reservation| reservation.bytes)
                .sum::<usize>()
        );
        assert_eq!(plan.reservations().len(), MemoryPlan::RESERVATIONS);
    }

    #[test]
    fn a_budget_that_covers_the_total_acquires_every_reservation_up_front() {
        let plan = plan();
        let budget = MemoryBudget::new(plan.total_bytes());
        let reserved = plan.acquire(&budget).expect("the whole plan is reserved");
        assert_eq!(reserved.count(), MemoryPlan::RESERVATIONS);
        assert_eq!(reserved.bytes(), plan.total_bytes());
        assert_eq!(budget.reserved(), plan.total_bytes());
        assert!(reserved.covers(plan.part_buffer_bytes()));
    }

    #[test]
    fn a_budget_one_byte_short_refuses_with_the_export_capacity_code() {
        let plan = plan();
        let budget = MemoryBudget::new(plan.total_bytes() - 1);
        let error = plan.acquire(&budget).expect_err("a short budget refuses");
        assert_eq!(CapacityError::CODE, PendingErrorCode::ExportCapacity);
        assert_eq!(CapacityError::CODE.as_str(), CAPACITY_CODE);
        assert_eq!(CapacityError::CODE.status(), 503);
        assert!(CapacityError::CODE.retryable());
        assert_eq!(error.capacity, plan.total_bytes() - 1);
        assert_eq!(
            error.reservation, ROW_GROUP_BUFFER,
            "the last reservation is the one that does not fit"
        );
    }

    #[test]
    fn the_refusal_names_the_first_reservation_that_cannot_be_met() {
        let plan = plan();
        let budget = MemoryBudget::new(1);
        let error = plan.acquire(&budget).expect_err("nothing fits");
        assert_eq!(error.reservation, PAGE_BUFFER);
        assert_eq!(error.requested, plan.page_buffer_bytes());
        assert!(error.to_string().contains(PAGE_BUFFER), "{error}");
    }

    #[test]
    fn a_failed_acquisition_leaves_the_budget_untouched() {
        let plan = plan();
        let budget = MemoryBudget::new(plan.total_bytes() - 1);
        assert!(plan.acquire(&budget).is_err());
        assert_eq!(
            budget.reserved(),
            0,
            "the partial reservations are released on the way out"
        );
    }

    #[test]
    fn the_reserved_total_depends_on_nothing_but_the_three_configured_bounds() {
        // Streaming is not an input to the plan, which is what makes the memory
        // profile of a multi-GB export flat.
        let first = MemoryPlan::new(500, 16 * 1024 * 1024, 64 * 1024 * 1024);
        let second = MemoryPlan::new(500, 16 * 1024 * 1024, 64 * 1024 * 1024);
        assert_eq!(first, second);
        assert_eq!(first.total_bytes(), second.total_bytes());
        // Only a configuration change moves it.
        let wider = MemoryPlan::new(1_000, 16 * 1024 * 1024, 64 * 1024 * 1024);
        assert_eq!(
            wider.total_bytes(),
            first.total_bytes() + 500 * PAGE_SLOT_BYTES
        );
    }

    #[test]
    fn releasing_the_reservations_returns_the_whole_budget() {
        let plan = plan();
        let budget = MemoryBudget::new(plan.total_bytes());
        {
            let _reserved = plan.acquire(&budget).expect("reserved");
            assert_eq!(budget.reserved(), plan.total_bytes());
        }
        assert_eq!(budget.reserved(), 0);
        plan.acquire(&budget).expect("the next export reserves too");
    }
}
