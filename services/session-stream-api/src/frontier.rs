//! The production reader of the projection frontier.
//!
//! `FeedFrontier.covered_through` has been written by `central-control-worker`
//! and read by nothing since it landed: `read_frontier` had no production caller
//! at all. That made the one number describing how long a pause, a revocation or
//! a limit change takes to reach a region unmeasured, while every cluster that
//! assumes "it takes effect" depends on it.
//!
//! This is that caller. It publishes one series, on a schedule, and refuses
//! nothing — see [`aex_regional_http::frontier`] for why breaching the ceiling
//! pages rather than closing the door.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use aex_regional_http::frontier::FrontierLag;
use aex_session_dynamodb::projection::AuthorizationProjection;
use aex_wire::types::Timestamp;

/// How often the frontier is measured.
///
/// Two ticks inside the target bound, so a breach is visible before it is
/// interesting rather than one sample after it stopped being.
pub const MEASURE_INTERVAL: Duration = Duration::from_secs(15);

/// How often the loop wakes to notice a drain it should stop for.
const DRAIN_CHECK: Duration = Duration::from_millis(250);

/// The plane, region and deployable every measurement is attributed to.
#[derive(Debug, Clone)]
pub struct FrontierResource {
    /// `dev` or `prd`.
    pub plane: String,
    /// The launch region.
    pub region: String,
    /// The deployable name.
    pub deployable: &'static str,
}

/// Measures once and emits, if the projection answered.
///
/// A frontier read that fails emits **nothing** rather than a zero. A gap in a
/// series is visibly a gap; a zero is a claim that the projection is perfectly
/// current, which is the one thing a broken reader must not be able to say.
#[must_use]
pub fn measure_once(now: Timestamp, covered_through: Timestamp) -> i64 {
    FrontierLag::measure(now, covered_through).millis
}

/// The wall clock, as the wire timestamp.
///
/// `None` for a host clock outside the representable range, which is a broken
/// machine rather than a customer condition — and still not a reason to publish
/// a number, so the sample is skipped rather than clamped.
fn wall_clock_now() -> Option<Timestamp> {
    let millis = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?
        .as_millis();
    Timestamp::from_unix_millis(i64::try_from(millis).ok()?).ok()
}

/// Reads and publishes the frontier lag until the process drains.
pub async fn publish(
    projection: Arc<dyn AuthorizationProjection>,
    resource: FrontierResource,
    interval: Duration,
    draining: Arc<AtomicBool>,
) {
    let mut due = tokio::time::Instant::now();
    loop {
        tokio::time::sleep(DRAIN_CHECK.min(interval)).await;
        if draining.load(Ordering::Acquire) {
            return;
        }
        if tokio::time::Instant::now() < due {
            continue;
        }
        due = tokio::time::Instant::now() + interval;
        let Ok(frontier) = projection.read_frontier().await else {
            // Deliberately silent in the series and loud nowhere else: a
            // projection this process cannot read is already visible through
            // every request that fails against it, and a fabricated sample here
            // would corrupt the one number this task exists to publish.
            continue;
        };
        let Some(now) = wall_clock_now() else {
            continue;
        };
        tracing::info!(
            target: "aex::diagnostics",
            event = "projection_frontier_measured",
            plane = %resource.plane,
            region = %resource.region,
            deployable = resource.deployable,
            lag_ms = measure_once(now, frontier.covered_through),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::measure_once;
    use aex_wire::types::Timestamp;

    fn at(millis: i64) -> Timestamp {
        Timestamp::from_unix_millis(millis).expect("a representable instant")
    }

    /// The published sample is the lag in milliseconds, attributed to the plane,
    /// region and deployable an alarm has to select on.
    #[test]
    fn one_measurement_reports_the_lag_in_milliseconds() {
        assert_eq!(measure_once(at(1_000_100_000), at(1_000_000_000)), 100_000);
    }
}
