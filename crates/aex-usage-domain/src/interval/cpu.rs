//! CPU reconciliation against the physical cgroup reading.
//!
//! Poll-elapsed attribution is an allocation *weight*, never unchecked billable
//! truth. When the meters attribute more than the cgroup physically consumed,
//! every meter's charge is scaled down proportionally with a floor, so the
//! charged total can never exceed the physical total. Whatever the meters did
//! not claim is platform overhead and is booked to the unbilled bucket.

use super::IntervalError;

/// How one closed interval's physical CPU was allocated.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CpuAllocation {
    /// Microseconds charged to each meter, in the order they were supplied.
    pub charged: Vec<u64>,
    /// Microseconds that no meter claimed; platform overhead, never billed.
    pub platform_us: u64,
    /// Whether proportional scaling was applied.
    pub scaled: bool,
}

impl CpuAllocation {
    /// The total charged across every meter.
    #[must_use]
    pub fn charged_us(&self) -> u64 {
        self.charged.iter().copied().sum()
    }
}

/// Allocates one interval's physical CPU across the meters that attributed to it.
///
/// When `sum(attributed) <= physical_us` every meter is charged what it
/// attributed. Otherwise meter `i` is charged
/// `attributed_i * physical_us / sum(attributed)`, computed in `u128` and
/// floored, which can never sum above `physical_us`.
///
/// # Errors
///
/// Returns [`IntervalError::Unreconcilable`] when the attributed total cannot
/// be represented, which the `u128` accumulator makes unreachable for any
/// realistic meter count and is kept as a typed refusal rather than a panic.
pub fn reconcile_cpu(physical_us: u64, attributed: &[u64]) -> Result<CpuAllocation, IntervalError> {
    let total: u128 = attributed.iter().copied().map(u128::from).sum();
    if total == 0 {
        return Ok(CpuAllocation {
            charged: vec![0; attributed.len()],
            platform_us: physical_us,
            scaled: false,
        });
    }
    let physical = u128::from(physical_us);
    let scaled = total > physical;
    let charged: Vec<u64> = if scaled {
        attributed
            .iter()
            .map(|value| {
                let share = u128::from(*value) * physical / total;
                u64::try_from(share).unwrap_or(u64::MAX)
            })
            .collect()
    } else {
        attributed.to_vec()
    };
    let charged_total: u128 = charged.iter().copied().map(u128::from).sum();
    let platform = physical
        .checked_sub(charged_total)
        .ok_or(IntervalError::Unreconcilable {
            physical_us,
            attributed_us: total,
        })?;
    Ok(CpuAllocation {
        charged,
        platform_us: u64::try_from(platform).unwrap_or(u64::MAX),
        scaled,
    })
}

#[cfg(test)]
mod tests {
    use super::reconcile_cpu;

    #[test]
    fn an_unsaturated_interval_charges_exactly_what_was_attributed() {
        let allocation = reconcile_cpu(1_000, &[100, 200, 300]).expect("reconciles");
        assert_eq!(allocation.charged, vec![100, 200, 300]);
        assert_eq!(allocation.platform_us, 400);
        assert!(!allocation.scaled);
    }

    #[test]
    fn over_attribution_scales_proportionally_and_never_exceeds_physical() {
        let allocation = reconcile_cpu(100, &[1_000, 1_000]).expect("reconciles");
        assert!(allocation.scaled);
        assert_eq!(allocation.charged, vec![50, 50]);
        assert_eq!(allocation.charged_us(), 100);
        assert_eq!(allocation.platform_us, 0);
    }

    #[test]
    fn the_pathological_ten_times_case_still_holds_the_bound() {
        let attributed = vec![300u64, 400, 300];
        let allocation = reconcile_cpu(100, &attributed).expect("reconciles");
        assert!(allocation.charged_us() <= 100);
        assert_eq!(
            allocation.charged_us() + allocation.platform_us,
            100,
            "charged + platform == physical"
        );
    }

    #[test]
    fn zero_attribution_books_everything_to_the_platform() {
        let allocation = reconcile_cpu(5_000, &[0, 0, 0]).expect("reconciles");
        assert_eq!(allocation.charged, vec![0, 0, 0]);
        assert_eq!(allocation.platform_us, 5_000);
        assert!(!allocation.scaled);
    }

    #[test]
    fn allocation_is_monotone_in_attribution() {
        let allocation = reconcile_cpu(1_000, &[100, 200, 300, 900]).expect("reconciles");
        for window in allocation.charged.windows(2) {
            assert!(window[0] <= window[1], "{:?}", allocation.charged);
        }
    }
}
