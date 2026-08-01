//! The per-run spend budget.
//!
//! Every provider call and every deliberately expensive AWS action consults
//! [`Budget::charge`] first. Exceeding the soft budget stops the run and
//! produces an `incomplete` receipt; exceeding the kill threshold - twice the
//! soft budget - produces `failed`. A budget stop is never a partial green,
//! which is why [`Charge`] has no variant that means "carry on anyway".

use std::sync::atomic::{AtomicU64, Ordering};

/// What a run may do after a charge.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Charge {
    /// The charge fitted inside the soft budget; continue.
    Allowed,
    /// The soft budget is exhausted. Stop the run and emit an `incomplete`
    /// receipt.
    SoftExceeded,
    /// Twice the soft budget is exhausted. Stop the run and emit a `failed`
    /// receipt.
    Killed,
}

impl Charge {
    /// Whether the run may issue further chargeable work.
    #[must_use]
    pub const fn may_continue(self) -> bool {
        matches!(self, Self::Allowed)
    }
}

/// Why a budget could not be built.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum BudgetError {
    /// A live, load or soak descriptor left `budget_micro_usd` unset.
    #[error(
        "workload `{workload}` declares no budget_micro_usd; a run with no spend ceiling cannot be stopped"
    )]
    Unset {
        /// The descriptor that omitted the value.
        workload: String,
    },
}

/// A monotone spend counter with a soft ceiling and a hard kill threshold.
#[derive(Debug)]
pub struct Budget {
    soft_micro_usd: u64,
    spent_micro_usd: AtomicU64,
}

impl Budget {
    /// The kill threshold is this multiple of the soft budget.
    pub const KILL_MULTIPLE: u64 = 2;

    /// A budget with a soft ceiling of `soft_micro_usd`.
    #[must_use]
    pub const fn new(soft_micro_usd: u64) -> Self {
        Self {
            soft_micro_usd,
            spent_micro_usd: AtomicU64::new(0),
        }
    }

    /// The soft ceiling.
    #[must_use]
    pub const fn soft_micro_usd(&self) -> u64 {
        self.soft_micro_usd
    }

    /// The hard kill threshold.
    #[must_use]
    pub const fn kill_micro_usd(&self) -> u64 {
        self.soft_micro_usd.saturating_mul(Self::KILL_MULTIPLE)
    }

    /// What has been charged so far.
    #[must_use]
    pub fn spent_micro_usd(&self) -> u64 {
        self.spent_micro_usd.load(Ordering::SeqCst)
    }

    /// Records `micro_usd` of spend and says what the run may do next.
    ///
    /// The counter saturates rather than wrapping, so a run that blows far past
    /// its ceiling still reports [`Charge::Killed`] instead of appearing cheap
    /// again.
    pub fn charge(&self, micro_usd: u64) -> Charge {
        let spent = self
            .spent_micro_usd
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |current| {
                Some(current.saturating_add(micro_usd))
            })
            .unwrap_or(0)
            .saturating_add(micro_usd);
        self.verdict(spent)
    }

    /// The verdict for the current spend without charging anything.
    #[must_use]
    pub fn verdict_now(&self) -> Charge {
        self.verdict(self.spent_micro_usd())
    }

    const fn verdict(&self, spent: u64) -> Charge {
        if spent > self.kill_micro_usd() {
            Charge::Killed
        } else if spent > self.soft_micro_usd {
            Charge::SoftExceeded
        } else {
            Charge::Allowed
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Budget, BudgetError, Charge};

    #[test]
    fn a_charge_inside_the_soft_budget_is_allowed() {
        let budget = Budget::new(100);
        assert_eq!(budget.charge(40), Charge::Allowed);
        assert_eq!(budget.charge(60), Charge::Allowed);
        assert_eq!(budget.spent_micro_usd(), 100);
        assert!(budget.verdict_now().may_continue());
    }

    #[test]
    fn crossing_the_soft_budget_stops_the_run_without_killing_it() {
        let budget = Budget::new(100);
        assert_eq!(budget.charge(101), Charge::SoftExceeded);
        assert!(!budget.verdict_now().may_continue());
    }

    #[test]
    fn crossing_twice_the_soft_budget_kills_the_run() {
        let budget = Budget::new(100);
        assert_eq!(budget.charge(201), Charge::Killed);
    }

    #[test]
    fn the_counter_saturates_rather_than_wrapping_back_to_cheap() {
        let budget = Budget::new(100);
        assert_eq!(budget.charge(u64::MAX), Charge::Killed);
        assert_eq!(budget.charge(u64::MAX), Charge::Killed);
        assert_eq!(budget.spent_micro_usd(), u64::MAX);
    }

    #[test]
    fn an_unset_budget_is_a_typed_error_naming_the_workload() {
        let error = BudgetError::Unset {
            workload: "brain-100-mixed".to_owned(),
        };
        assert_eq!(
            error.to_string(),
            "workload `brain-100-mixed` declares no budget_micro_usd; a run with no spend ceiling cannot be stopped"
        );
    }
}
