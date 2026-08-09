//! The one place the container stop timeout and the process drain deadline meet.
//!
//! A drain deadline is only a deadline if it fires before the runtime sends
//! `SIGKILL`. Until this module existed, that relationship was a sentence in a
//! Terraform validation message and a `< 30_000` assertion in one test fixture:
//! nothing refused a deployment that set the deadline above the stop timeout, so
//! the "deadline" would simply never be reached and the task would be killed
//! mid-request instead of exiting on its own terms.
//!
//! The relationship is now a bound the configuration reader enforces at
//! start-up, derived here from the same [`FARGATE_STOP_TIMEOUT_S`] the task
//! definition pins. A deployment that disagrees refuses to start rather than
//! discovering the disagreement as truncated requests during a rollout.

/// The stop timeout every long-lived regional Fargate task is given, in seconds.
///
/// `ECS` sends `SIGTERM`, waits this long, then sends `SIGKILL`. The value is
/// pinned by an `ecs-service` variable validation on the deployable name; this
/// constant is the process-side half of that same pin.
pub const FARGATE_STOP_TIMEOUT_S: u64 = 30;

/// The window a task keeps for itself after it abandons a drain.
///
/// Abandoning the drain is not the end of the shutdown: the process still has to
/// unwind the listener and flush telemetry, and a flush that is cut off by
/// `SIGKILL` loses exactly the records explaining why the drain overran.
pub const DRAIN_EXIT_MARGIN_MS: u64 = 5_000;

/// The shortest drain deadline that is worth having.
pub const MIN_DRAIN_DEADLINE_MS: u64 = 1_000;

/// The longest drain deadline that can still fire before `SIGKILL`.
///
/// `30 s` stop timeout less the `5 s` exit margin.
pub const MAX_DRAIN_DEADLINE_MS: u64 = FARGATE_STOP_TIMEOUT_S * 1_000 - DRAIN_EXIT_MARGIN_MS;

// These are compile-time assertions rather than tests on purpose. The whole
// point of this module is that the relationship between the stop timeout and the
// drain deadline stops being something a reader has to notice; a `const` block
// fails the build, which is one step earlier than failing a test.
const _: () = assert!(
    MIN_DRAIN_DEADLINE_MS < MAX_DRAIN_DEADLINE_MS,
    "the admitted drain window is empty"
);
const _: () = assert!(
    MAX_DRAIN_DEADLINE_MS < FARGATE_STOP_TIMEOUT_S * 1_000,
    "the longest admitted deadline must still fire before SIGKILL"
);
const _: () = assert!(
    FARGATE_STOP_TIMEOUT_S * 1_000 - MAX_DRAIN_DEADLINE_MS == DRAIN_EXIT_MARGIN_MS,
    "the margin is what the task keeps to unwind and flush"
);
