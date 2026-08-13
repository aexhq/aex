//! The readiness dependency probe.
//!
//! Readiness starts false and stays false until this loop has proved, with a real signed
//! request, that the Brain's `DynamoDB` tables and its `SQS` wake queue answer. Nothing else
//! in the process ever reports the store reachable, which is the defect this module closes:
//! before it existed the only positive callers were tests, so a deployed task answered
//! `/internal/readyz` with 503 for its whole life.
//!
//! Liveness is deliberately untouched. A task whose dependencies are down is still alive and
//! still owns non-replayable effects it is trying to settle; failing liveness would have the
//! orchestrator kill it along with them. It simply stops being a candidate for new work.

use crate::control::HealthState;
use aex_brain_app::ports::{BoxFuture, StoreError};
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::time::Instant;

/// How often reachability is re-proved.
///
/// A background cadence rather than a check on the probe path: proving a dependency costs a
/// signed round trip to `DynamoDB` and to `SQS`, and paying that inside `/internal/readyz`
/// would put the load balancer's health check on the critical path of the two services it is
/// asking about — and make an overloaded dependency the reason the task is replaced. Ten
/// seconds is the alpha cadence the accepted plan pins; it is a schedule, not a measurement,
/// and the load evidence in W10 is what would move it.
pub const PROBE_INTERVAL: core::time::Duration = core::time::Duration::from_secs(10);

/// The sentinel [`DependencyProbe::last_success`] uses before the first successful round.
///
/// An [`Instant`] has no representable "never", and an `Option<Instant>` is not something
/// this process can publish without a lock. The offset from the probe's own start is stored
/// instead, which is a value an atomic can hold.
const NEVER: u64 = u64::MAX;

/// One dependency the task cannot serve work without.
///
/// Named rather than an anonymous list of futures, so a failed round says which authority
/// refused instead of leaving an operator to infer it from a 503 body that says only "store
/// unreachable".
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Dependency {
    /// The `session-authority` and `regional-work` tables.
    BrainStore,
    /// The queue Brain wakes are delivered on.
    WakeQueue,
}

impl Dependency {
    /// The name this dependency is reported under.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::BrainStore => "brain-store",
            Self::WakeQueue => "wake-queue",
        }
    }
}

/// Something whose reachability readiness depends on.
///
/// A trait rather than the concrete adapters, because the loop's own rules — first round
/// before ready, both directions published, every target attempted — are what has to be
/// asserted, and asserting them must not require an `AWS` account.
pub trait Reachable: Send + Sync + core::fmt::Debug {
    /// Which dependency this target proves.
    fn dependency(&self) -> Dependency;

    /// Performs one real request against it.
    ///
    /// # Errors
    ///
    /// Whatever the adapter's own transport returned. A constructed client is never a
    /// success here; only an answered request is.
    fn reach(&self) -> BoxFuture<'_, Result<(), StoreError>>;
}

impl Reachable for aex_brain_store_dynamodb::BrainStore {
    fn dependency(&self) -> Dependency {
        Dependency::BrainStore
    }

    fn reach(&self) -> BoxFuture<'_, Result<(), StoreError>> {
        Box::pin(self.probe())
    }
}

impl Reachable for aex_brain_store_dynamodb::SqsWakeQueue {
    fn dependency(&self) -> Dependency {
        Dependency::WakeQueue
    }

    fn reach(&self) -> BoxFuture<'_, Result<(), StoreError>> {
        Box::pin(self.probe())
    }
}

/// Proves every dependency reachable and publishes the answer to readiness.
#[derive(Debug)]
pub struct DependencyProbe {
    health: Arc<HealthState>,
    targets: Vec<Arc<dyn Reachable>>,
    interval: core::time::Duration,
    started: Instant,
    last_success_offset_ms: AtomicU64,
    consecutive_failures: AtomicU32,
}

impl DependencyProbe {
    /// Binds the probe to the readiness state it publishes to.
    #[must_use]
    pub fn new(health: Arc<HealthState>, targets: Vec<Arc<dyn Reachable>>) -> Self {
        Self::with_interval(health, targets, PROBE_INTERVAL)
    }

    /// As [`DependencyProbe::new`], at a cadence the caller chooses.
    ///
    /// Only the composition root and this module's own assertions call this. Production uses
    /// [`PROBE_INTERVAL`], which is why it is a constant rather than configuration.
    #[must_use]
    pub fn with_interval(
        health: Arc<HealthState>,
        targets: Vec<Arc<dyn Reachable>>,
        interval: core::time::Duration,
    ) -> Self {
        Self {
            health,
            targets,
            interval,
            started: Instant::now(),
            last_success_offset_ms: AtomicU64::new(NEVER),
            consecutive_failures: AtomicU32::new(0),
        }
    }

    /// Every dependency this probe proves, in the order it proves them.
    #[must_use]
    pub fn dependencies(&self) -> Vec<Dependency> {
        self.targets
            .iter()
            .map(|target| target.dependency())
            .collect()
    }

    /// When the last round in which every dependency answered completed.
    #[must_use]
    pub fn last_success(&self) -> Option<Instant> {
        match self.last_success_offset_ms.load(Ordering::SeqCst) {
            NEVER => None,
            offset => Some(self.started + core::time::Duration::from_millis(offset)),
        }
    }

    /// Rounds that have failed since the last one in which every dependency answered.
    #[must_use]
    pub fn consecutive_failures(&self) -> u32 {
        self.consecutive_failures.load(Ordering::SeqCst)
    }

    /// Proves every dependency once and publishes the answer, returning what refused.
    ///
    /// Every target is attempted even after one refuses, so a single round reports every
    /// authority that is down rather than only the first one in the list.
    pub async fn round(&self) -> Vec<Dependency> {
        let mut refused = Vec::new();
        for target in &self.targets {
            if let Err(error) = target.reach().await {
                eprintln!(
                    "brain-mux: {} is unreachable: {error}",
                    target.dependency().as_str()
                );
                refused.push(target.dependency());
            }
        }
        // Published in both directions. A probe that only ever set readiness true would
        // leave a task advertising itself long after the authority behind it went away.
        self.health.store_reachable(refused.is_empty());
        if refused.is_empty() {
            self.consecutive_failures.store(0, Ordering::SeqCst);
            self.last_success_offset_ms.store(
                u64::try_from(self.started.elapsed().as_millis()).unwrap_or(u64::MAX - 1),
                Ordering::SeqCst,
            );
        } else {
            self.consecutive_failures.fetch_add(1, Ordering::SeqCst);
        }
        refused
    }

    /// Proves every dependency now, then again every [`PROBE_INTERVAL`] until aborted.
    ///
    /// The first round runs before the first sleep, so readiness is decided by evidence
    /// rather than by whichever happens first, an interval or a load-balancer probe. The
    /// loop never returns: it is a child of the composition root's structured shutdown
    /// scope, which aborts and joins it as part of drain.
    pub async fn run(self: Arc<Self>) {
        loop {
            let _ = self.round().await;
            tokio::time::sleep(self.interval).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Dependency, DependencyProbe, PROBE_INTERVAL, Reachable};
    use crate::control::HealthState;
    use crate::health::{LIVE_PATH, READY_PATH};
    use aex_brain_app::ports::{BoxFuture, StoreError};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

    /// A dependency whose reachability the test moves, and which counts what it was asked.
    #[derive(Debug)]
    struct Fake {
        dependency: Dependency,
        reachable: AtomicBool,
        attempts: AtomicU32,
    }

    impl Fake {
        fn new(dependency: Dependency) -> Arc<Self> {
            Arc::new(Self {
                dependency,
                reachable: AtomicBool::new(true),
                attempts: AtomicU32::new(0),
            })
        }

        fn set(&self, reachable: bool) {
            self.reachable.store(reachable, Ordering::SeqCst);
        }

        fn attempts(&self) -> u32 {
            self.attempts.load(Ordering::SeqCst)
        }
    }

    impl Reachable for Fake {
        fn dependency(&self) -> Dependency {
            self.dependency
        }

        fn reach(&self) -> BoxFuture<'_, Result<(), StoreError>> {
            Box::pin(async move {
                self.attempts.fetch_add(1, Ordering::SeqCst);
                if self.reachable.load(Ordering::SeqCst) {
                    Ok(())
                } else {
                    Err(StoreError::Transport {
                        reason: format!("{} refused", self.dependency.as_str()),
                        retryable: true,
                    })
                }
            })
        }
    }

    /// A process in which everything except reachability has already validated, so a 503
    /// can only ever mean the dependency the round is about.
    fn probe(store: &Arc<Fake>, queue: &Arc<Fake>) -> (Arc<HealthState>, Arc<DependencyProbe>) {
        let health = HealthState::starting(50, 200);
        health.bindings_validated();
        health.schema_matched();
        let dependencies = DependencyProbe::new(
            Arc::clone(&health),
            vec![
                Arc::clone(store) as Arc<dyn Reachable>,
                Arc::clone(queue) as Arc<dyn Reachable>,
            ],
        );
        (health, Arc::new(dependencies))
    }

    /// The defect this module closes: everything else about the process validated, and the
    /// task still refused work because nothing had proved the store answers.
    #[tokio::test(flavor = "current_thread")]
    async fn a_task_whose_dependencies_have_not_answered_is_never_ready() {
        let store = Fake::new(Dependency::BrainStore);
        let queue = Fake::new(Dependency::WakeQueue);
        store.set(false);
        let (health, dependencies) = probe(&store, &queue);

        assert_eq!(health.respond("GET", READY_PATH).status, 503);
        assert_eq!(
            dependencies.round().await,
            vec![Dependency::BrainStore],
            "the failing authority names itself"
        );
        assert_eq!(health.respond("GET", READY_PATH).status, 503);
        assert!(
            health
                .respond("GET", READY_PATH)
                .body
                .contains("store unreachable")
        );
        assert_eq!(dependencies.consecutive_failures(), 1);
        assert_eq!(dependencies.last_success(), None);
    }

    /// Readiness has to move in both directions. A probe that only ever set it true would
    /// leave a task advertising itself long after its authority went away.
    #[tokio::test(flavor = "current_thread")]
    async fn readiness_follows_the_dependency_in_both_directions_and_recovers() {
        let store = Fake::new(Dependency::BrainStore);
        let queue = Fake::new(Dependency::WakeQueue);
        let (health, dependencies) = probe(&store, &queue);

        assert!(dependencies.round().await.is_empty());
        assert_eq!(health.respond("GET", READY_PATH).status, 200);
        let first_success = dependencies.last_success().expect("a round succeeded");

        queue.set(false);
        assert_eq!(dependencies.round().await, vec![Dependency::WakeQueue]);
        assert_eq!(health.respond("GET", READY_PATH).status, 503);
        assert_eq!(dependencies.round().await, vec![Dependency::WakeQueue]);
        assert_eq!(dependencies.consecutive_failures(), 2);
        assert_eq!(
            dependencies.last_success(),
            Some(first_success),
            "a failing round never advances the last-success instant"
        );

        queue.set(true);
        assert!(dependencies.round().await.is_empty());
        assert_eq!(health.respond("GET", READY_PATH).status, 200);
        assert_eq!(dependencies.consecutive_failures(), 0);
        assert!(dependencies.last_success().expect("recovered") >= first_success);
    }

    /// Failing liveness for a dependency would have the orchestrator kill a task that is
    /// working perfectly, along with the non-replayable effects it is trying to settle.
    #[tokio::test(flavor = "current_thread")]
    async fn dependency_loss_never_touches_liveness() {
        let store = Fake::new(Dependency::BrainStore);
        let queue = Fake::new(Dependency::WakeQueue);
        let (health, dependencies) = probe(&store, &queue);

        for reachable in [true, false, false, true] {
            store.set(reachable);
            queue.set(reachable);
            let _ = dependencies.round().await;
            assert_eq!(
                health.respond("GET", LIVE_PATH).status,
                200,
                "a task that cannot reach its store is still alive"
            );
        }
    }

    /// One round has to report every authority that is down. Stopping at the first refusal
    /// would hide a second outage behind the one already being fixed.
    #[tokio::test(flavor = "current_thread")]
    async fn every_dependency_is_exercised_even_after_one_refuses() {
        let store = Fake::new(Dependency::BrainStore);
        let queue = Fake::new(Dependency::WakeQueue);
        let (_health, dependencies) = probe(&store, &queue);
        assert_eq!(
            dependencies.dependencies(),
            vec![Dependency::BrainStore, Dependency::WakeQueue]
        );

        store.set(false);
        queue.set(false);
        assert_eq!(
            dependencies.round().await,
            vec![Dependency::BrainStore, Dependency::WakeQueue]
        );
        assert_eq!(store.attempts(), 1);
        assert_eq!(
            queue.attempts(),
            1,
            "the queue is probed on its own account"
        );
    }

    /// The loop proves reachability before its first sleep, so readiness is decided by
    /// evidence rather than by whichever arrives first, an interval or a probe.
    #[tokio::test(flavor = "current_thread", start_paused = true)]
    async fn the_loop_probes_once_before_it_ever_waits() {
        let store = Fake::new(Dependency::BrainStore);
        let queue = Fake::new(Dependency::WakeQueue);
        let (health, dependencies) = probe(&store, &queue);

        let running = tokio::spawn(Arc::clone(&dependencies).run());
        tokio::task::yield_now().await;
        assert_eq!(store.attempts(), 1);
        assert_eq!(health.respond("GET", READY_PATH).status, 200);

        tokio::time::advance(PROBE_INTERVAL).await;
        tokio::task::yield_now().await;
        assert!(store.attempts() >= 2, "{}", store.attempts());

        running.abort();
        let _ = running.await;
    }

    #[test]
    fn the_alpha_cadence_is_a_background_schedule_rather_than_a_request_path_check() {
        assert_eq!(PROBE_INTERVAL.as_secs(), 10);
    }
}
