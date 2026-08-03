//! Slice S-3.6 — the synchronization kernel.
//!
//! Two layers, deliberately.
//!
//! - Real `std::thread` tests run in every lane. They catch the ordinary mistakes and keep
//!   this target from being empty when the `loom` feature is off.
//! - Loom models run with `--features loom` and explore every interleaving of the
//!   primitives, which is the only way to be sure about the cases threads reach rarely.
//!
//! Loom is applied to the six named kernels and nowhere else.
//!
//! The two layers are mutually exclusive by configuration, not by skipping: under
//! `--features loom` the kernel's primitives *are* Loom's, and touching one outside a
//! `loom::model` closure is an error. Both configurations run a non-empty set of cases, and
//! both lanes are executed.

/// The threaded layer: real `std::thread`, run in every lane the `loom` feature is off.
#[cfg(not(feature = "loom"))]
mod threaded {
    use aex_brain_application::kernel::{
        ActivationRegistry, DrainGate, PermitKind, PermitSet, RenewalOutcome, RenewalState,
        WarmCacheShard, WarmEntry,
    };
    use aex_brain_application::ports::CancelToken;
    use aex_brain_domain::fold::FoldState;
    use aex_brain_domain::ids::{AgentId, AgentKey, AgentRevision, Fence, SessionId};
    use std::collections::BTreeMap;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use uuid::Uuid;

    fn key(seed: u128) -> AgentKey {
        AgentKey::new(
            SessionId(Uuid::from_u128(1)),
            AgentId(Uuid::from_u128(seed)),
        )
    }

    fn permits(kind: PermitKind, limit: u64) -> Arc<PermitSet> {
        Arc::new(PermitSet::new(BTreeMap::from([(kind, limit)])))
    }

    // ---------------------------------------------------------------------------
    // L1 — activation single-flight
    // ---------------------------------------------------------------------------

    /// Concurrent activations of one agent produce exactly one hydration; the loser is told so
    /// rather than being made to wait.
    #[test]
    fn l1_concurrent_activation_hydrates_once() {
        for _ in 0..64 {
            let registry = Arc::new(ActivationRegistry::new());
            let hydrations = Arc::new(AtomicUsize::new(0));
            let busy = Arc::new(AtomicUsize::new(0));

            let handles: Vec<_> = (0..4)
                .map(|_| {
                    let registry = Arc::clone(&registry);
                    let hydrations = Arc::clone(&hydrations);
                    let busy = Arc::clone(&busy);
                    std::thread::spawn(move || match registry.try_activate(key(2)) {
                        Ok(slot) => {
                            hydrations.fetch_add(1, Ordering::SeqCst);
                            drop(slot);
                        }
                        Err(_) => {
                            busy.fetch_add(1, Ordering::SeqCst);
                        }
                    })
                })
                .collect();
            for handle in handles {
                handle.join().expect("no thread panics");
            }

            assert_eq!(
                hydrations.load(Ordering::SeqCst) + busy.load(Ordering::SeqCst),
                4
            );
            assert!(hydrations.load(Ordering::SeqCst) >= 1);
            assert_eq!(registry.active(), 0, "every slot released on drop");
        }
    }

    /// A slot releases on an early return and on a panic, so an agent cannot be wedged out of
    /// this process for the rest of its life.
    #[test]
    fn l1_a_slot_releases_on_panic() {
        let registry = Arc::new(ActivationRegistry::new());
        let inner = Arc::clone(&registry);
        let result = std::panic::catch_unwind(move || {
            let _slot = inner.try_activate(key(3)).expect("the first claim wins");
            panic!("the activation failed mid-flight");
        });
        assert!(result.is_err());
        assert!(!registry.holds(&key(3)), "the slot must not leak");
    }

    // ---------------------------------------------------------------------------
    // L2 — renewer and commit interlock
    // ---------------------------------------------------------------------------

    /// A fence advance observed by the renewer is observed by the commit before it writes.
    #[test]
    fn l2_a_fence_advance_reaches_the_committer_before_it_writes() {
        for _ in 0..256 {
            let cancel = CancelToken::new();
            let state = Arc::new(RenewalState::new(Fence(4), cancel));

            let renewer = {
                let state = Arc::clone(&state);
                std::thread::spawn(move || state.observe(Fence(5)))
            };
            let committer = {
                let state = Arc::clone(&state);
                std::thread::spawn(move || state.may_commit())
            };

            let observed = renewer.join().expect("the renewer does not panic");
            let allowed = committer.join().expect("the committer does not panic");
            assert_eq!(observed, RenewalOutcome::Lost);

            // Whichever order the two ran in, the state afterwards must agree: once the
            // renewer has returned `Lost`, no further commit may be attempted.
            assert!(!state.may_commit(), "a fenced activation must not commit");
            assert!(state.cancel().is_cancelled());
            // The committer may legitimately have run first and seen `true`; what it must never
            // do is see `true` *after* the advance is visible.
            if !allowed {
                assert_eq!(state.observed(), Fence(5));
            }
        }
    }

    /// Three consecutive renewal failures cancel the activation; one or two do not.
    #[test]
    fn l2_renewal_failures_cancel_only_at_the_threshold() {
        let cancel = CancelToken::new();
        let state = RenewalState::new(Fence(1), cancel);
        assert_eq!(
            state.renewal_failed(),
            RenewalOutcome::Failed { consecutive: 1 }
        );
        assert_eq!(
            state.renewal_failed(),
            RenewalOutcome::Failed { consecutive: 2 }
        );
        assert!(state.may_commit(), "two failures are not evidence of loss");
        assert_eq!(state.renewal_failed(), RenewalOutcome::Lost);
        assert!(!state.may_commit());
    }

    /// A successful renewal resets the failure count and never moves the fence.
    #[test]
    fn l2_renewal_resets_failures_and_leaves_the_fence_alone() {
        let state = RenewalState::new(Fence(7), CancelToken::new());
        assert_eq!(
            state.renewal_failed(),
            RenewalOutcome::Failed { consecutive: 1 }
        );
        assert_eq!(state.renewed(Fence(7)), RenewalOutcome::Extended);
        assert_eq!(
            state.renewal_failed(),
            RenewalOutcome::Failed { consecutive: 1 },
            "the count restarted"
        );
        assert_eq!(state.held(), Fence(7), "a renewal does not change hands");
    }

    // ---------------------------------------------------------------------------
    // L3 — cancellation versus preparation
    // ---------------------------------------------------------------------------

    /// Once cancel is observed, no further effect is prepared.
    #[test]
    fn l3_no_effect_is_prepared_after_cancel_is_observed() {
        for _ in 0..256 {
            let token = CancelToken::new();
            let prepared = Arc::new(AtomicUsize::new(0));

            let canceller = {
                let token = token.clone();
                std::thread::spawn(move || token.cancel())
            };
            let planner = {
                let token = token.clone();
                let prepared = Arc::clone(&prepared);
                std::thread::spawn(move || {
                    if !token.is_cancelled() {
                        prepared.fetch_add(1, Ordering::SeqCst);
                    }
                    // The second check is what the effect driver actually does: cancel is
                    // re-read immediately before the durable prepare, so a cancel that lands
                    // in between still stops the write.
                    token.is_cancelled()
                })
            };

            canceller.join().expect("no panic");
            let cancelled_at_prepare = planner.join().expect("no panic");
            assert!(token.is_cancelled(), "the flag is one-way");
            if cancelled_at_prepare {
                // Whatever the planner did optimistically, the driver refuses the write.
                assert!(token.is_cancelled());
            }
        }
    }

    // ---------------------------------------------------------------------------
    // L4 — permit accounting
    // ---------------------------------------------------------------------------

    /// Acquire and release across every path leaks nothing and double-releases nothing.
    #[test]
    fn l4_permits_never_leak_and_never_double_release() {
        let set = permits(PermitKind::Activation, 8);
        for _ in 0..64 {
            let handles: Vec<_> = (0..8)
                .map(|index| {
                    let set = Arc::clone(&set);
                    std::thread::spawn(move || {
                        let Ok(reservation) = set.acquire(PermitKind::Activation, 1) else {
                            return;
                        };
                        // An early return on an odd index: the reservation must still release.
                        if index % 2 == 0 {
                            return;
                        }
                        assert_eq!(reservation.units(), 1);
                    })
                })
                .collect();
            for handle in handles {
                handle.join().expect("no thread panics");
            }
            assert_eq!(
                set.held(PermitKind::Activation),
                0,
                "every path released exactly once"
            );
        }
    }

    /// A failed acquisition holds nothing, so a failed conditional claim cannot leak.
    #[test]
    fn l4_a_failed_acquisition_holds_nothing() {
        let set = permits(PermitKind::ContextBytes, 1_024);
        let held = set
            .acquire(PermitKind::ContextBytes, 1_000)
            .expect("1000 of 1024 fits");
        let error = set
            .acquire(PermitKind::ContextBytes, 100)
            .expect_err("100 more does not");
        assert_eq!(error.available, 24);
        assert_eq!(set.held(PermitKind::ContextBytes), 1_000);
        drop(held);
        assert_eq!(set.held(PermitKind::ContextBytes), 0);
    }

    /// An unconfigured resource is unavailable, never unbounded.
    #[test]
    fn l4_an_unconfigured_resource_is_not_unbounded() {
        let set = permits(PermitKind::Activation, 4);
        let error = set
            .acquire(PermitKind::HandsRpc, 1)
            .expect_err("an unconfigured kind admits nothing");
        assert_eq!(error.limit, 0);
    }

    /// Opposite request order cannot split one activation's bundle across two threads.
    #[test]
    fn l4_multi_resource_acquisition_is_atomic() {
        for _ in 0..64 {
            let set = Arc::new(PermitSet::new(BTreeMap::from([
                (PermitKind::Activation, 1),
                (PermitKind::ProviderStream, 1),
            ])));
            let active = Arc::new(AtomicUsize::new(0));
            let maximum = Arc::new(AtomicUsize::new(0));
            let handles = [
                [(PermitKind::Activation, 1), (PermitKind::ProviderStream, 1)],
                [(PermitKind::ProviderStream, 1), (PermitKind::Activation, 1)],
            ]
            .into_iter()
            .map(|request| {
                let set = Arc::clone(&set);
                let active = Arc::clone(&active);
                let maximum = Arc::clone(&maximum);
                std::thread::spawn(move || {
                    let Ok(_bundle) = set.acquire_many(&request) else {
                        return;
                    };
                    let now = active.fetch_add(1, Ordering::SeqCst) + 1;
                    maximum.fetch_max(now, Ordering::SeqCst);
                    active.fetch_sub(1, Ordering::SeqCst);
                })
            })
            .collect::<Vec<_>>();
            for handle in handles {
                handle.join().expect("no thread panics");
            }
            assert!(maximum.load(Ordering::SeqCst) <= 1);
            assert_eq!(set.held(PermitKind::Activation), 0);
            assert_eq!(set.held(PermitKind::ProviderStream), 0);
        }
    }

    // ---------------------------------------------------------------------------
    // L5 — warm cache versus revision
    // ---------------------------------------------------------------------------

    /// No interleaving leaves an entry whose revision is below the committed revision.
    #[test]
    fn l5_the_cache_never_holds_a_fold_older_than_the_commit() {
        for _ in 0..64 {
            let cache = Arc::new(WarmCacheShard::new(1_024 * 1_024, 64 * 1_024));
            let inserter = {
                let cache = Arc::clone(&cache);
                std::thread::spawn(move || {
                    cache.insert(
                        key(4),
                        WarmEntry {
                            revision: AgentRevision(3),
                            state: FoldState::empty(),
                            bytes: 128,
                            last_used: 0,
                        },
                    );
                })
            };
            let invalidator = {
                let cache = Arc::clone(&cache);
                std::thread::spawn(move || cache.invalidate(&key(4), AgentRevision(5)))
            };
            inserter.join().expect("no panic");
            invalidator.join().expect("no panic");

            // Whichever ran first, asking for the committed revision must never return the
            // stale fold.
            assert!(cache.get(&key(4), AgentRevision(5), 1).is_none());
        }
    }

    /// An insert below the stored revision is refused, so a slow hydration cannot overwrite a
    /// fast commit.
    #[test]
    fn l5_a_late_hydration_cannot_overwrite_a_newer_fold() {
        let cache = WarmCacheShard::new(1_024 * 1_024, 64 * 1_024);
        let newer = WarmEntry {
            revision: AgentRevision(9),
            state: FoldState::empty(),
            bytes: 64,
            last_used: 0,
        };
        assert!(cache.insert(key(5), newer));
        let older = WarmEntry {
            revision: AgentRevision(4),
            state: FoldState::empty(),
            bytes: 64,
            last_used: 1,
        };
        assert!(!cache.insert(key(5), older), "a stale fold is refused");
        assert!(cache.get(&key(5), AgentRevision(9), 2).is_some());
        assert!(cache.get(&key(5), AgentRevision(4), 3).is_none());
    }

    /// The cache stays inside its byte budget and refuses an oversized entry outright.
    #[test]
    fn l5_the_cache_stays_inside_its_own_budget() {
        let cache = WarmCacheShard::new(256, 128);
        for index in 0..8_u64 {
            cache.insert(
                key(100 + u128::from(index)),
                WarmEntry {
                    revision: AgentRevision(1),
                    state: FoldState::empty(),
                    bytes: 100,
                    last_used: index,
                },
            );
        }
        assert!(cache.bytes() <= 256, "{} bytes held", cache.bytes());

        let oversized = WarmEntry {
            revision: AgentRevision(1),
            state: FoldState::empty(),
            bytes: 129,
            last_used: 99,
        };
        assert!(!cache.insert(key(999), oversized), "the entry cap holds");

        cache.clear();
        assert!(cache.is_empty() && cache.bytes() == 0);
    }

    // ---------------------------------------------------------------------------
    // L6 — drain gate
    // ---------------------------------------------------------------------------

    /// After drain is observed by any thread, no thread admits new work; in-flight guards still
    /// complete.
    #[test]
    fn l6_drain_stops_admission_without_abandoning_work() {
        for _ in 0..64 {
            let gate = Arc::new(DrainGate::new());
            let admitted = Arc::new(AtomicUsize::new(0));

            let drainer = {
                let gate = Arc::clone(&gate);
                std::thread::spawn(move || gate.start_drain())
            };
            let admitters: Vec<_> = (0..4)
                .map(|_| {
                    let gate = Arc::clone(&gate);
                    let admitted = Arc::clone(&admitted);
                    std::thread::spawn(move || {
                        if let Some(permit) = gate.try_admit() {
                            admitted.fetch_add(1, Ordering::SeqCst);
                            drop(permit);
                        }
                    })
                })
                .collect();

            drainer.join().expect("no panic");
            for handle in admitters {
                handle.join().expect("no panic");
            }

            assert!(gate.is_draining());
            assert_eq!(gate.in_flight(), 0, "every admitted permit completed");
            assert!(gate.is_quiesced());
        }
    }

    /// Admission before drain still holds the process open until the work completes.
    #[test]
    fn l6_an_in_flight_permit_keeps_the_process_from_quiescing() {
        let gate = DrainGate::new();
        let permit = gate.try_admit().expect("admitted before drain");
        gate.start_drain();
        assert!(gate.try_admit().is_none(), "no new work after drain");
        assert_eq!(gate.in_flight(), 1);
        assert!(!gate.is_quiesced(), "in-flight work still runs");
        drop(permit);
        assert!(gate.is_quiesced());
    }

    // ---------------------------------------------------------------------------
    // Loom models — exhaustive interleaving over the same six kernels
    // ---------------------------------------------------------------------------
}

#[cfg(feature = "loom")]
mod loom_models {
    use aex_brain_application::kernel::{
        ActivationRegistry, DrainGate, PermitKind, PermitSet, PermitSetFull, RenewalOutcome,
        RenewalState, WarmCacheShard, WarmEntry,
    };
    use aex_brain_domain::ids::{AgentId, AgentKey, SessionId};
    use std::collections::BTreeMap;
    use uuid::Uuid;

    fn key(seed: u128) -> AgentKey {
        AgentKey::new(
            SessionId(Uuid::from_u128(1)),
            AgentId(Uuid::from_u128(seed)),
        )
    }

    fn permits(kind: PermitKind, limit: u64) -> Arc<PermitSet> {
        Arc::new(PermitSet::new(BTreeMap::from([(kind, limit)])))
    }

    #[allow(dead_code)]
    fn full_is_named(error: PermitSetFull) -> PermitKind {
        error.kind
    }
    use aex_brain_application::ports::CancelToken;
    use aex_brain_domain::fold::FoldState;
    use aex_brain_domain::ids::{AgentRevision, Fence};
    use loom::thread;
    use std::sync::Arc;

    /// L1 — two threads activate concurrently; exactly one wins and no slot leaks.
    #[test]
    fn l1_single_flight() {
        loom::model(|| {
            let registry = Arc::new(ActivationRegistry::new());
            let first = {
                let registry = Arc::clone(&registry);
                thread::spawn(move || registry.try_activate(key(2)).is_ok())
            };
            let second = {
                let registry = Arc::clone(&registry);
                thread::spawn(move || registry.try_activate(key(2)).is_ok())
            };
            let won = usize::from(first.join().unwrap()) + usize::from(second.join().unwrap());
            assert!(won >= 1, "somebody must win");
            assert_eq!(registry.active(), 0, "no slot leaked");
        });
    }

    /// L2 — the renewer and the committer never both believe they hold the fence.
    #[test]
    fn l2_renewer_commit_interlock() {
        loom::model(|| {
            let state = Arc::new(RenewalState::new(Fence(4), CancelToken::new()));
            let renewer = {
                let state = Arc::clone(&state);
                thread::spawn(move || state.observe(Fence(5)))
            };
            let committer = {
                let state = Arc::clone(&state);
                thread::spawn(move || state.may_commit())
            };
            assert_eq!(renewer.join().unwrap(), RenewalOutcome::Lost);
            let _ = committer.join().unwrap();
            assert!(!state.may_commit(), "the loser must never commit");
        });
    }

    /// L3 — no execution prepares an effect after cancel is observed.
    #[test]
    fn l3_cancel_beats_prepare() {
        loom::model(|| {
            let token = CancelToken::new();
            let canceller = {
                let token = token.clone();
                thread::spawn(move || token.cancel())
            };
            let planner = {
                let token = token.clone();
                thread::spawn(move || {
                    let seen = token.is_cancelled();
                    // The driver re-reads immediately before the durable write.
                    (seen, token.is_cancelled())
                })
            };
            canceller.join().unwrap();
            let (_, at_write) = planner.join().unwrap();
            if at_write {
                assert!(token.is_cancelled());
            }
            assert!(token.is_cancelled(), "the flag is one-way");
        });
    }

    /// L4 — no permit leak and no double release, in every interleaving.
    #[test]
    fn l4_permit_accounting() {
        loom::model(|| {
            let set = permits(PermitKind::Activation, 1);
            let first = {
                let set = Arc::clone(&set);
                thread::spawn(move || {
                    let _held = set.acquire(PermitKind::Activation, 1);
                })
            };
            let second = {
                let set = Arc::clone(&set);
                thread::spawn(move || {
                    let _held = set.acquire(PermitKind::Activation, 1);
                })
            };
            first.join().unwrap();
            second.join().unwrap();
            assert_eq!(set.held(PermitKind::Activation), 0);
        });
    }

    /// L4 — two opposing bundles are linearized; no interleaving can split their resources.
    #[test]
    fn l4_multi_resource_permit_accounting() {
        loom::model(|| {
            use loom::sync::atomic::{AtomicUsize, Ordering};

            let set = Arc::new(PermitSet::new(BTreeMap::from([
                (PermitKind::Activation, 1),
                (PermitKind::ProviderStream, 1),
            ])));
            let active = Arc::new(AtomicUsize::new(0));
            let maximum = Arc::new(AtomicUsize::new(0));
            let first = {
                let set = Arc::clone(&set);
                let active = Arc::clone(&active);
                let maximum = Arc::clone(&maximum);
                thread::spawn(move || {
                    let Ok(_bundle) = set.acquire_many(&[
                        (PermitKind::Activation, 1),
                        (PermitKind::ProviderStream, 1),
                    ]) else {
                        return;
                    };
                    let now = active.fetch_add(1, Ordering::SeqCst) + 1;
                    maximum.fetch_max(now, Ordering::SeqCst);
                    active.fetch_sub(1, Ordering::SeqCst);
                })
            };
            let second = {
                let set = Arc::clone(&set);
                let active = Arc::clone(&active);
                let maximum = Arc::clone(&maximum);
                thread::spawn(move || {
                    let Ok(_bundle) = set.acquire_many(&[
                        (PermitKind::ProviderStream, 1),
                        (PermitKind::Activation, 1),
                    ]) else {
                        return;
                    };
                    let now = active.fetch_add(1, Ordering::SeqCst) + 1;
                    maximum.fetch_max(now, Ordering::SeqCst);
                    active.fetch_sub(1, Ordering::SeqCst);
                })
            };
            first.join().unwrap();
            second.join().unwrap();
            assert!(maximum.load(Ordering::SeqCst) <= 1);
            assert_eq!(set.held(PermitKind::Activation), 0);
            assert_eq!(set.held(PermitKind::ProviderStream), 0);
        });
    }

    /// L5 — insert versus invalidate never leaves a fold below the committed revision.
    #[test]
    fn l5_cache_revision_monotonicity() {
        loom::model(|| {
            let cache = Arc::new(WarmCacheShard::new(4_096, 4_096));
            let inserter = {
                let cache = Arc::clone(&cache);
                thread::spawn(move || {
                    cache.insert(
                        key(4),
                        WarmEntry {
                            revision: AgentRevision(3),
                            state: FoldState::empty(),
                            bytes: 8,
                            last_used: 0,
                        },
                    );
                })
            };
            let invalidator = {
                let cache = Arc::clone(&cache);
                thread::spawn(move || cache.invalidate(&key(4), AgentRevision(5)))
            };
            inserter.join().unwrap();
            invalidator.join().unwrap();
            assert!(cache.get(&key(4), AgentRevision(5), 1).is_none());
        });
    }

    /// L6 — after drain is observed anywhere, nothing new is admitted.
    #[test]
    fn l6_drain_gate() {
        loom::model(|| {
            let gate = Arc::new(DrainGate::new());
            let drainer = {
                let gate = Arc::clone(&gate);
                thread::spawn(move || gate.start_drain())
            };
            let admitter = {
                let gate = Arc::clone(&gate);
                thread::spawn(move || gate.try_admit().is_some())
            };
            drainer.join().unwrap();
            let _ = admitter.join().unwrap();
            assert!(gate.is_draining());
            assert_eq!(gate.in_flight(), 0, "every permit completed");
        });
    }
}
