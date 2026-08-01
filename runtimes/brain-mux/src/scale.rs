//! Predictive scale-out signals.
//!
//! Native `SQS` step scaling on `ApproximateNumberOfMessagesVisible` is rejected as a
//! *measured* failure, not a stylistic one: in the pool spike its desired count reached 14
//! after every unit of work had already finished. The queue depth says how much work
//! arrived, not how much is left or how long it takes.
//!
//! What is published instead is nine signals at 10-second resolution, and a desired count
//! derived from work-seconds. `task_start_seconds` is 50 from measurement (31–51 s observed),
//! which is why the request has to go out at 60–70 % of the safe envelope rather than at the
//! pressure bands.

/// How long a new task takes to become useful, from measurement.
pub const TASK_START_SECONDS: f64 = 50.0;

/// The resolution scaling signals are published at.
pub const PUBLISH_INTERVAL: core::time::Duration = core::time::Duration::from_secs(10);

/// Everything one task publishes each interval.
///
/// Never CPU alone: a task can be at 20 % CPU with a 40-second backlog of provider streams,
/// and a task at 90 % CPU may be about to finish everything it holds.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ScaleSignals {
    /// Work-seconds of runnable backlog.
    pub runnable_backlog_work_seconds: f64,
    /// How long the oldest runnable item has waited.
    pub oldest_runnable_age_ms: u64,
    /// Activations running.
    pub active_activations: u32,
    /// Provider streams open.
    pub active_provider_streams: u32,
    /// Bytes reserved against the task's envelope.
    pub reserved_bytes: u64,
    /// The envelope itself.
    pub envelope_bytes: u64,
    /// The reactor's observed p99 scheduling lateness.
    pub reactor_delay_p99_ms: u32,
    /// Jobs waiting for a compute-lane slot.
    pub compute_lane_queue_depth: usize,
    /// The compute lane's utilization, in hundredths.
    pub compute_lane_utilization: u32,
    /// Bytes the warm cache holds.
    pub warm_cache_bytes: u64,
    /// The cache's own budget.
    pub cache_budget_bytes: u64,
    /// How many units have been shed.
    pub shed_total: u64,
    /// How many have been deferred.
    pub deferred_total: u64,
}

impl ScaleSignals {
    /// The names published, in a fixed order.
    ///
    /// Published as a constant so the operations area can bind alarms to names rather than
    /// to positions in a struct that will grow.
    pub const NAMES: [&'static str; 12] = [
        "runnable_backlog_work_seconds",
        "oldest_runnable_age_ms",
        "active_activations",
        "active_provider_streams",
        "reserved_bytes_ratio",
        "reactor_delay_p99_ms",
        "compute_lane_queue_depth",
        "compute_lane_utilization",
        "provider_permit_saturation",
        "warm_cache_ratio",
        "shed_total",
        "deferred_total",
    ];

    /// The fraction of the memory envelope reserved, in hundredths.
    #[must_use]
    pub fn reserved_ratio_percent(&self) -> u32 {
        ratio_percent(self.reserved_bytes, self.envelope_bytes)
    }

    /// The fraction of the cache's own budget in use, in hundredths.
    #[must_use]
    pub fn warm_cache_ratio_percent(&self) -> u32 {
        ratio_percent(self.warm_cache_bytes, self.cache_budget_bytes)
    }
}

fn ratio_percent(numerator: u64, denominator: u64) -> u32 {
    if denominator == 0 {
        // An unconfigured envelope reads as full rather than as empty: reporting 0 % for a
        // budget nobody set would hide the misconfiguration until the task ran out.
        return 100;
    }
    u32::try_from(numerator.saturating_mul(100) / denominator).unwrap_or(100)
}

/// The bounds a desired count is clamped into.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ScaleBounds {
    /// The fewest tasks the service ever runs.
    pub min_tasks: u32,
    /// The most, from the tenant and provider quota.
    pub max_tasks: u32,
    /// How many work-seconds one healthy task is expected to absorb per interval.
    pub target_work_seconds_per_task: f64,
}

/// The desired task count.
///
/// ```text
/// desired = ceil( (backlog + arrival_rate * task_start_seconds * mean_work_seconds)
///                 / target_work_seconds_per_task )
/// ```
///
/// The arrival term is what makes it predictive: by the time a task exists,
/// `task_start_seconds` of further work has arrived, and sizing for the backlog alone
/// guarantees the new task is already behind when it starts.
#[must_use]
pub fn desired_tasks(
    signals: &ScaleSignals,
    arrival_rate_per_second: f64,
    mean_work_seconds: f64,
    bounds: ScaleBounds,
) -> u32 {
    if bounds.target_work_seconds_per_task <= 0.0 {
        return bounds.max_tasks;
    }
    let anticipated = arrival_rate_per_second * TASK_START_SECONDS * mean_work_seconds;
    let total = signals.runnable_backlog_work_seconds.max(0.0) + anticipated.max(0.0);
    let raw = (total / bounds.target_work_seconds_per_task).ceil();
    // Counted up rather than cast down from the float: a `f64 as u32` of a non-finite or
    // out-of-range value is a saturating cast whose behaviour a reader has to look up, and a
    // desired task count is not the place to make anyone look that up. The loop runs at most
    // `max_tasks` times, which is a quota, not a magnitude.
    if !raw.is_finite() || raw <= 0.0 {
        return bounds.min_tasks;
    }
    let mut count = bounds.min_tasks;
    while count < bounds.max_tasks && f64::from(count) < raw {
        count += 1;
    }
    count
}

/// Whether a change from `current` to `desired` should be acted on.
///
/// Hysteresis exists because `task_start_seconds` is 50: a scale decision that flaps costs
/// nearly a minute of capacity each way, so a one-task oscillation is more expensive than
/// the imbalance it corrects.
#[must_use]
pub const fn should_act(current: u32, desired: u32, hysteresis: u32) -> bool {
    if desired > current {
        desired - current > hysteresis
    } else if current > desired {
        current - desired > hysteresis
    } else {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::{
        PUBLISH_INTERVAL, ScaleBounds, ScaleSignals, TASK_START_SECONDS, desired_tasks, should_act,
    };

    fn signals(backlog: f64) -> ScaleSignals {
        ScaleSignals {
            runnable_backlog_work_seconds: backlog,
            oldest_runnable_age_ms: 0,
            active_activations: 0,
            active_provider_streams: 0,
            reserved_bytes: 0,
            envelope_bytes: 1_000,
            reactor_delay_p99_ms: 0,
            compute_lane_queue_depth: 0,
            compute_lane_utilization: 0,
            warm_cache_bytes: 0,
            cache_budget_bytes: 1_000,
            shed_total: 0,
            deferred_total: 0,
        }
    }

    fn bounds() -> ScaleBounds {
        ScaleBounds {
            min_tasks: 1,
            max_tasks: 20,
            target_work_seconds_per_task: 10.0,
        }
    }

    #[test]
    fn the_published_names_are_stable_and_distinct() {
        let mut names = ScaleSignals::NAMES.to_vec();
        let total = names.len();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), total);
        assert_eq!(PUBLISH_INTERVAL.as_secs(), 10);
    }

    /// The measured start latency is what makes the calculation predictive rather than
    /// reactive, so it is pinned here.
    #[test]
    fn the_start_latency_is_the_measured_one() {
        assert!((TASK_START_SECONDS - 50.0).abs() < f64::EPSILON);
    }

    /// The arrival term is the whole point: sizing for the backlog alone guarantees the new
    /// task is already behind by the time it exists.
    #[test]
    fn the_desired_count_anticipates_work_that_arrives_while_a_task_starts() {
        let reactive = desired_tasks(&signals(100.0), 0.0, 1.0, bounds());
        let predictive = desired_tasks(&signals(100.0), 1.0, 1.0, bounds());
        assert!(
            predictive > reactive,
            "{predictive} should exceed {reactive}"
        );
        assert_eq!(reactive, 10);
        assert_eq!(predictive, 15, "100 backlog plus 50 anticipated over 10");
    }

    /// The measured failure this replaces: an empty backlog must produce the floor, not a
    /// desired count derived from how many messages once arrived.
    #[test]
    fn an_empty_backlog_produces_the_floor_rather_than_a_stale_count() {
        assert_eq!(desired_tasks(&signals(0.0), 0.0, 1.0, bounds()), 1);
    }

    #[test]
    fn the_desired_count_is_clamped_into_its_quota() {
        assert_eq!(desired_tasks(&signals(100_000.0), 0.0, 1.0, bounds()), 20);
        assert_eq!(desired_tasks(&signals(-5.0), 0.0, 1.0, bounds()), 1);
    }

    #[test]
    fn a_zero_target_falls_back_to_the_quota_rather_than_dividing_by_zero() {
        let bounds = ScaleBounds {
            target_work_seconds_per_task: 0.0,
            ..bounds()
        };
        assert_eq!(desired_tasks(&signals(10.0), 0.0, 1.0, bounds), 20);
    }

    /// A scale decision that flaps costs nearly a minute of capacity each way.
    #[test]
    fn hysteresis_absorbs_a_one_task_oscillation() {
        assert!(!should_act(10, 11, 1));
        assert!(!should_act(11, 10, 1));
        assert!(should_act(10, 12, 1));
        assert!(should_act(12, 10, 1));
        assert!(!should_act(10, 10, 0));
    }

    /// An unconfigured envelope reads as full. Reporting 0 % for a budget nobody set would
    /// hide the misconfiguration until the task ran out of memory.
    #[test]
    fn an_unconfigured_envelope_reads_as_full_rather_than_as_empty() {
        let mut signals = signals(0.0);
        signals.envelope_bytes = 0;
        signals.cache_budget_bytes = 0;
        assert_eq!(signals.reserved_ratio_percent(), 100);
        assert_eq!(signals.warm_cache_ratio_percent(), 100);
    }

    #[test]
    fn the_ratios_report_the_fraction_in_use() {
        let mut signals = signals(0.0);
        signals.reserved_bytes = 700;
        signals.warm_cache_bytes = 250;
        assert_eq!(signals.reserved_ratio_percent(), 70);
        assert_eq!(signals.warm_cache_ratio_percent(), 25);
    }
}
