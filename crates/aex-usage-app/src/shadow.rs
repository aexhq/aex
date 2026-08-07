//! The `METER-03` shadow close: what a qualification run reports and asserts.
//!
//! Shadow mode admits, projects and publishes real facts to a shadow rating
//! queue while finance posts `Microusd::ZERO`. Nothing leaves shadow until the
//! gates in plan 12 §8 pass, and this module is what a close hands to whoever
//! signs them off.
//!
//! Three things the report exists to make unavoidable:
//!
//! 1. **`M-STOR-CLOSE` is declared, not discovered.** Storage interiors floor to
//!    the minute and the hard-delete close ceils, so a customer charge can
//!    exceed physical residence by at most `bytes x 1 minute` per residence.
//!    [`ShadowReport::ceil_bound_holds`] asserts that bound and every close
//!    reports the total, so finance signs an exact number rather than a
//!    principle.
//! 2. **Attribution never exceeds physics.** `charged_us <= physical_us` and
//!    `charged_us + platform_us == physical_us` are reported per close, so the
//!    cgroup reconciliation can be checked against a real bill the moment one
//!    exists.
//! 3. **Shadow rates nothing.** A report carrying a non-zero rated total is a
//!    failed close, not a surprising one.

use std::collections::BTreeMap;

use aex_usage_domain::frontier::Frontier;
use aex_usage_domain::meter::Meter;
use serde::{Deserialize, Serialize};

use crate::projection::{ProjectionTransaction, quantity_total};
use crate::worker::BillingMode;

/// The report schema identifier.
pub const SHADOW_REPORT_SCHEMA: &str = "aex.usage.shadow-close.v1";

/// Why a shadow close did not qualify.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ShadowError {
    /// A close ran with the charging gate active.
    #[error("a shadow close cannot be taken in `{mode}` mode")]
    NotShadow {
        /// The mode the run was in.
        mode: &'static str,
    },
    /// Shadow rated something.
    #[error("a shadow close rated {rated_microusd} microUSD; shadow posts zero")]
    RatedInShadow {
        /// What was rated.
        rated_microusd: i128,
    },
    /// The declared storage transformation exceeded its own bound.
    #[error(
        "the storage ceil close charged {charged} byte-minutes over {residences} \
         residences, above the declared bound of {bound}"
    )]
    CeilBoundExceeded {
        /// What the close charged above the floored interior total.
        charged: u128,
        /// How many residences closed.
        residences: u64,
        /// `sum(bytes) x 1 minute`, the declared ceiling.
        bound: u128,
    },
    /// Charged CPU exceeded the physical reading.
    #[error("charged {charged_us} CPU microseconds against {physical_us} physical")]
    OverPhysical {
        /// What was charged.
        charged_us: u64,
        /// What the cgroup reported.
        physical_us: u64,
    },
    /// The platform bucket did not absorb the remainder exactly.
    #[error("charged {charged_us} + platform {platform_us} != physical {physical_us}")]
    UnbalancedCpu {
        /// What was charged.
        charged_us: u64,
        /// What was booked to platform overhead.
        platform_us: u64,
        /// What the cgroup reported.
        physical_us: u64,
    },
    /// An interval was still open at the end of a close.
    #[error("{leaked} interval(s) were still open when the close was taken")]
    LeakedInterval {
        /// How many.
        leaked: u64,
    },
}

/// The `M-STOR-CLOSE` accounting transformation, as one close saw it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StorageCeilSummary {
    /// How many residences reached a hard delete in this close.
    pub residences: u64,
    /// The byte-minutes the terminal ceil added over a floored close.
    pub ceil_byte_minutes: u128,
    /// `sum(bytes_at_close)`, which times one minute is the declared bound.
    pub closing_bytes: u128,
}

impl StorageCeilSummary {
    /// Whether the charged excess stayed inside `bytes x 1 minute` per residence.
    ///
    /// # Errors
    ///
    /// Returns [`ShadowError::CeilBoundExceeded`] when it did not, which would
    /// mean the transformation is not the one that was signed off.
    pub const fn holds(&self) -> Result<(), ShadowError> {
        if self.ceil_byte_minutes > self.closing_bytes {
            return Err(ShadowError::CeilBoundExceeded {
                charged: self.ceil_byte_minutes,
                residences: self.residences,
                bound: self.closing_bytes,
            });
        }
        Ok(())
    }
}

/// What the cgroup reconciler saw across one close.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CpuAttributionSummary {
    /// The cgroup reading.
    pub physical_us: u64,
    /// What the activations claimed before scaling.
    pub attributed_us: u64,
    /// What was charged after scaling.
    pub charged_us: u64,
    /// What was booked to unbilled platform overhead.
    pub platform_us: u64,
    /// How many intervals had to be scaled down.
    pub scaled_intervals: u64,
}

impl CpuAttributionSummary {
    /// Whether charged CPU stayed inside physics and the remainder balanced.
    ///
    /// # Errors
    ///
    /// [`ShadowError::OverPhysical`] or [`ShadowError::UnbalancedCpu`].
    pub const fn holds(&self) -> Result<(), ShadowError> {
        if self.charged_us > self.physical_us {
            return Err(ShadowError::OverPhysical {
                charged_us: self.charged_us,
                physical_us: self.physical_us,
            });
        }
        if self.charged_us + self.platform_us != self.physical_us {
            return Err(ShadowError::UnbalancedCpu {
                charged_us: self.charged_us,
                platform_us: self.platform_us,
                physical_us: self.physical_us,
            });
        }
        Ok(())
    }
}

/// How far behind central delivery is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OutboxLag {
    /// Rows still undelivered.
    pub pending: u64,
    /// The oldest undelivered row's age.
    pub oldest_age_ms: u64,
    /// Rows the sweep had to republish.
    pub republished: u64,
}

/// One workspace-category frontier position, copied into the report.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FrontierPosition {
    /// The workspace.
    pub workspace: String,
    /// The authority.
    pub category: String,
    /// Last admitted.
    pub accepted: u64,
    /// Last folded.
    pub projected: u64,
    /// Last delivered.
    pub published: u64,
    /// Last settled.
    pub settled: u64,
    /// Whether the frontier is parked.
    pub quarantined: bool,
}

impl FrontierPosition {
    /// Copies one frontier.
    #[must_use]
    pub fn of(frontier: &Frontier) -> Self {
        Self {
            workspace: frontier.workspace.to_string(),
            category: frontier.category.id().to_owned(),
            accepted: frontier.accepted.get(),
            projected: frontier.projected.get(),
            published: frontier.published.get(),
            settled: frontier.settled.get(),
            quarantined: frontier.is_quarantined(),
        }
    }
}

/// The machine-readable receipt one shadow close emits.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShadowReport {
    /// The report schema.
    pub schema: String,
    /// The mode the close was taken in. Always `shadow`.
    pub mode: String,
    /// Facts folded.
    pub fact_count: u64,
    /// Workspaces covered.
    pub workspace_count: u64,
    /// Per-meter quantity totals, keyed by meter identifier.
    pub per_meter: BTreeMap<String, i128>,
    /// Zero-dollar observability counts, keyed by observability meter.
    pub observability: BTreeMap<String, u64>,
    /// The declared storage transformation.
    pub storage_ceil: StorageCeilSummary,
    /// The cgroup reconciliation.
    pub cpu: CpuAttributionSummary,
    /// Delivery lag.
    pub outbox: OutboxLag,
    /// What finance rated. Always zero in shadow.
    pub rated_microusd: i128,
    /// Intervals still open at close. Always zero in a passing close.
    pub open_intervals: u64,
    /// Every frontier, copied.
    pub frontiers: Vec<FrontierPosition>,
}

impl ShadowReport {
    /// An empty shadow close.
    #[must_use]
    pub fn new() -> Self {
        Self {
            schema: SHADOW_REPORT_SCHEMA.to_owned(),
            mode: BillingMode::Shadow.id().to_owned(),
            fact_count: 0,
            workspace_count: 0,
            per_meter: BTreeMap::new(),
            observability: BTreeMap::new(),
            storage_ceil: StorageCeilSummary::default(),
            cpu: CpuAttributionSummary::default(),
            outbox: OutboxLag::default(),
            rated_microusd: 0,
            open_intervals: 0,
            frontiers: Vec::new(),
        }
    }

    /// Accumulates the per-meter totals a set of folded transactions moved.
    pub fn observe(&mut self, transactions: &[ProjectionTransaction]) {
        self.fact_count += transactions.len() as u64;
        for (meter, total) in quantity_total(transactions) {
            *self.per_meter.entry(meter).or_default() += total;
        }
    }

    /// Records the frontiers this close covered.
    pub fn cover(&mut self, frontiers: &[Frontier]) {
        self.workspace_count = frontiers.len() as u64;
        self.frontiers = frontiers.iter().map(FrontierPosition::of).collect();
    }

    /// Every gate this close can decide without a physical receipt.
    ///
    /// The gates it cannot decide — per-service `AWS` reconciliation, cgroup
    /// accuracy against a real bill, Hands transmit-receipt existence, the
    /// Lambda vCPU convention, delivery-receipt formats — are named in
    /// `references/rewrite/usage.md` rather than silently passed here.
    ///
    /// # Errors
    ///
    /// The first [`ShadowError`] that fails.
    pub fn qualifies(&self) -> Result<(), ShadowError> {
        if self.mode != BillingMode::Shadow.id() {
            return Err(ShadowError::NotShadow { mode: "active" });
        }
        if self.rated_microusd != 0 {
            return Err(ShadowError::RatedInShadow {
                rated_microusd: self.rated_microusd,
            });
        }
        self.storage_ceil.holds()?;
        self.cpu.holds()?;
        if self.open_intervals != 0 {
            return Err(ShadowError::LeakedInterval {
                leaked: self.open_intervals,
            });
        }
        Ok(())
    }

    /// Whether the declared storage transformation stayed inside its bound.
    ///
    /// # Errors
    ///
    /// [`ShadowError::CeilBoundExceeded`].
    pub const fn ceil_bound_holds(&self) -> Result<(), ShadowError> {
        self.storage_ceil.holds()
    }

    /// Every priced meter that produced nothing in this close.
    ///
    /// Reported rather than omitted: "we measured nothing on this meter" and
    /// "this meter was never wired up" look identical in an absent row.
    #[must_use]
    pub fn silent_meters(&self) -> Vec<&'static str> {
        Meter::ALL
            .into_iter()
            .filter(|meter| !self.per_meter.contains_key(meter.id()))
            .map(Meter::id)
            .collect()
    }

    /// The report as canonical JSON.
    ///
    /// # Errors
    ///
    /// Returns the `serde_json` failure, which a report of integers and strings
    /// never produces.
    pub fn render(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string_pretty(self)
    }
}

impl Default for ShadowReport {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests;
