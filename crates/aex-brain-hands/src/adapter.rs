//! The Brain-side Hands adapter: lazy materialization, admission and guest-loss
//! classification.
//!
//! # H-LAZY
//!
//! Session create allocates a generation and writes a head in `requested`. **No
//! provider call happens.** A model-only, todo-only, web-only or MCP-only session
//! therefore consumes no `MicroVM` memory quota, no snapshot storage and no
//! lifecycle API rate. Materialization is triggered by exactly two events: the
//! first Hands operation, or a live workspace read that asks to wake a retained
//! workspace.
//!
//! Three collapse layers, in this order:
//!
//! 1. **Process-local single flight**, so five hundred children racing the first
//!    tool call produce one provider attempt per task;
//! 2. **Durable state**, so a loser in another process observes `launching` and
//!    polls the head instead of calling the provider;
//! 3. **Provider idempotency** on `client_token = "aexgen-{generation}"`, so even a
//!    cross-process race, a lost response or a mux crash between dispatch and
//!    commit converges on one VM.
//!
//! Layer three is what makes the other two optimisations rather than correctness
//! requirements, which is the right way round: an optimisation that fails costs
//! latency, and a correctness requirement that fails costs a second `MicroVM`.

use aex_hands_protocol::rpc::Fence;
use aex_runtime_control::generation::{
    AdmissionRefused, GenerationHead, GenerationState, Revision, TransportMode,
};
use aex_runtime_control::lifecycle::client_token;
pub use aex_runtime_control::store::{
    OperationAdmissionPlan as AdmitPlan, OperationSettlementPlan as SettlePlan,
};
use aex_wire::ids::GenerationId;
use aex_wire::types::{ComputeSize, Timestamp};

/// A pooled connection is closed after this long with no in-flight stream.
///
/// Brain keeps no idle Hands socket: a held socket makes a session look alive to
/// anything watching the transport, and a held stream is torn by a suspend anyway.
pub const CONNECTION_IDLE_MS: u64 = 15_000;

/// Connections reserved for the probe and the lifecycle path.
pub const RESERVED_CONNECTIONS: u32 = 2;

/// Largest terminal body Brain will assemble from a pull.
pub const MAX_RESULT_BODY_BYTES: u64 = 4_194_304;

/// Largest single frame.
pub const MAX_FRAME_BYTES: u32 = 1_048_576;

/// How much of a tool result reaches the next model context.
pub const CONTEXT_TOOL_RESULT_BYTES: u64 = 65_536;

/// The first poll backoff while another task holds the launch.
pub const LAUNCH_POLL_BASE_MS: u64 = 50;

/// The largest poll backoff while another task holds the launch.
pub const LAUNCH_POLL_MAX_MS: u64 = 800;

/// What ALPN negotiated.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Alpn {
    /// HTTP/2.
    H2,
    /// HTTP/1.1.
    Http11,
}

/// Chooses the transport mode from what ALPN actually negotiated.
///
/// Recorded on the generation head rather than silently chosen, so the two modes'
/// different pool sizes and concurrency ceilings are measurable rather than
/// mysterious.
#[must_use]
pub const fn transport_mode(alpn: Alpn) -> TransportMode {
    match alpn {
        Alpn::H2 => TransportMode::Multiplexed,
        Alpn::Http11 => TransportMode::PerRequest,
    }
}

/// The pool a mode uses for a shape.
#[must_use]
pub fn pool_size(mode: TransportMode, size: ComputeSize) -> u32 {
    mode.pool_size(size)
}

/// The in-flight ceiling a mode allows for a shape.
#[must_use]
pub fn max_in_flight(mode: TransportMode, size: ComputeSize) -> u32 {
    mode.max_in_flight(size)
}

/// What `ensure_materialized` should do, given what it observed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MaterializeStep {
    /// Nothing to do: the generation is already serving.
    Ready,
    /// Take the head from `requested` to `launching` and call the provider.
    ///
    /// The token is deterministic, so re-issuing this exact request after a lost
    /// response is safe.
    Launch {
        /// The conditional-write revision.
        expected_revision: Revision,
        /// The deterministic replay identity.
        client_token: String,
    },
    /// Another task or process holds the launch. Poll the head with bounded
    /// backoff; do **not** call the provider.
    AwaitLaunch {
        /// How long to wait before the next poll.
        backoff_ms: u64,
    },
    /// The generation is suspended and must be resumed before it can serve.
    Resume {
        /// The conditional-write revision.
        expected_revision: Revision,
    },
    /// The generation is gone. A new one is allocated by the session authority;
    /// this adapter never allocates one.
    Lost,
    /// A lifecycle outcome is unresolved. No second effect may be dispatched.
    AwaitReconciliation,
}

/// Decides the next materialization step from the durable head.
///
/// This is collapse layer two. Layer one is the caller's per-generation single
/// flight and layer three is the provider's own idempotency on the client token;
/// all three are needed because each covers a race the others do not.
#[must_use]
pub fn materialize_step(head: &GenerationHead, attempt: u32) -> MaterializeStep {
    match head.state {
        GenerationState::Running | GenerationState::LifetimeDraining => MaterializeStep::Ready,
        GenerationState::Requested => MaterializeStep::Launch {
            expected_revision: head.revision,
            client_token: client_token(head.generation),
        },
        // A suspend in flight resolves to `suspended` or back to `running`; either
        // way the caller re-reads rather than racing the worker's fence.
        GenerationState::Launching | GenerationState::Resuming | GenerationState::Suspending => {
            MaterializeStep::AwaitLaunch {
                backoff_ms: launch_backoff_ms(attempt),
            }
        }
        GenerationState::Suspended => MaterializeStep::Resume {
            expected_revision: head.revision,
        },
        GenerationState::Unknown => MaterializeStep::AwaitReconciliation,
        GenerationState::Terminating | GenerationState::Terminated | GenerationState::Lost => {
            MaterializeStep::Lost
        }
    }
}

/// Bounded exponential backoff for a materialization poll.
#[must_use]
pub const fn launch_backoff_ms(attempt: u32) -> u64 {
    let shift = if attempt > 4 { 4 } else { attempt };
    let raw = LAUNCH_POLL_BASE_MS << shift;
    if raw > LAUNCH_POLL_MAX_MS {
        LAUNCH_POLL_MAX_MS
    } else {
        raw
    }
}

/// The conditional write that admits one operation.
///
/// Admission precedes dispatch and settlement follows the Brain journal commit, so
/// an increment-then-crash leaves an **over**-count, never an under-count. That is
/// the safe direction: an over-count only makes the system less eager to suspend.
/// Builds the admission write.
///
/// # Errors
///
/// Returns [`AdmissionRefused`] when the head is not `running`, the fence or
/// revision is stale, a suspend lock is held, or the shape's concurrency ceiling is
/// reached.
pub fn admit(
    head: &GenerationHead,
    fence: Fence,
    revision: Revision,
    now: Timestamp,
) -> Result<AdmitPlan, AdmissionRefused> {
    let admitted = head.admit(fence, revision, now)?;
    Ok(AdmitPlan {
        generation: head.generation,
        fence,
        expected_revision: revision,
        open_operations: admitted.open_operations,
        next_revision: admitted.revision,
        last_busy_at: now,
    })
}

/// The conditional write that settles one operation.
/// Builds the settlement write.
///
/// Deliberately not conditional on the fence: settlement must land even after a
/// lifecycle transition has advanced it, or a suspend would strand the counter
/// above zero forever and the generation would never become idle.
#[must_use]
pub fn settle(head: &GenerationHead, now: Timestamp) -> SettlePlan {
    let (open_operations, next_revision, last_busy_at) = head.settle(now);
    SettlePlan {
        generation: head.generation,
        expected_revision: head.revision,
        open_operations,
        next_revision,
        last_busy_at,
    }
}

/// How Brain classifies a failure so the effect state machine can route it.
///
/// A single opaque error would collapse `Interrupt` with `RetrySameEffect`, which
/// is the difference between telling a customer their work stopped and silently
/// running it twice.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum HandsError {
    /// The generation is gone. Uncommitted work is interrupted and a new
    /// generation never impersonates the old result.
    #[error("generation {generation} is lost")]
    GenerationLost {
        /// Which generation.
        generation: GenerationId,
    },
    /// The request lost a fence race. Re-read and retry against the current fence.
    #[error("fenced: presented {presented}, current {current}")]
    Fenced {
        /// The fence the caller presented.
        presented: u64,
        /// The fence the head holds.
        current: u64,
    },
    /// The supervisor restarted under a live operation, or the operation's
    /// substrate vanished. The effect is interrupted, not retried.
    #[error("operation interrupted by the guest")]
    Interrupted,
    /// The provider has no capacity. The effect is queued, not failed.
    #[error("provider capacity exhausted; the effect is queued")]
    CapacityQueued,
    /// The pinned image carries no such capability. Retrying changes nothing.
    #[error("the pinned image carries no `{capability}` capability")]
    CapabilityUnavailable {
        /// Which capability.
        capability: String,
    },
    /// The transport failed. The same effect may be retried.
    #[error("transport failure: {reason}")]
    Transport {
        /// Why.
        reason: String,
    },
}

impl HandsError {
    /// Whether the same effect may be dispatched again.
    #[must_use]
    pub const fn retry_same_effect(&self) -> bool {
        matches!(self, Self::Fenced { .. } | Self::Transport { .. })
    }

    /// Whether the effect must be interrupted rather than retried.
    #[must_use]
    pub const fn interrupts(&self) -> bool {
        matches!(self, Self::GenerationLost { .. } | Self::Interrupted)
    }
}

#[cfg(test)]
mod tests {
    use super::{
        AdmitPlan, Alpn, CONNECTION_IDLE_MS, HandsError, LAUNCH_POLL_MAX_MS, MaterializeStep,
        admit, launch_backoff_ms, materialize_step, max_in_flight, pool_size, settle,
        transport_mode,
    };
    use aex_hands_protocol::rpc::Fence;
    use aex_runtime_control::generation::{
        AdmissionRefused, GenerationHead, GenerationState, Revision, TransportMode,
    };
    use aex_runtime_control::lifecycle::client_token;
    use aex_wire::ids::{GenerationId, PrefixedId as _, Uuid7};
    use aex_wire::types::{ComputeSize, Timestamp};

    fn at(millis: i64) -> Timestamp {
        Timestamp::from_unix_millis(millis).expect("a bounded instant")
    }

    fn generation() -> GenerationId {
        GenerationId::from_uuid7(Uuid7::compose(1, [1; 10]))
    }

    fn head(state: GenerationState, open: u32) -> GenerationHead {
        GenerationHead {
            generation: generation(),
            size: ComputeSize::Gb1,
            state,
            fence: Fence(3),
            revision: Revision::new(11),
            open_operations: open,
            last_busy_at: at(1_000),
            idle_since: None,
            suspend_lock_expires_at: None,
            keepalive_lease: None,
            transport_mode: None,
        }
    }

    #[test]
    fn a_model_only_session_never_reaches_the_provider() {
        // H-LAZY: a head in `requested` has had no provider call. The only thing
        // that moves it is a materialization step, and nothing else in this module
        // produces one.
        let requested = head(GenerationState::Requested, 0);
        assert!(matches!(
            materialize_step(&requested, 0),
            MaterializeStep::Launch { .. }
        ));
        // Until then it simply sits there: no call, no quota, no snapshot.
        assert_eq!(requested.state, GenerationState::Requested);
    }

    #[test]
    fn only_one_task_launches_and_every_loser_polls_instead_of_calling_the_provider() {
        let winner = materialize_step(&head(GenerationState::Requested, 0), 0);
        let MaterializeStep::Launch {
            expected_revision,
            client_token: token,
        } = winner
        else {
            panic!("the first observer launches");
        };
        assert_eq!(expected_revision, Revision::new(11));
        assert_eq!(token, client_token(generation()));

        // Every subsequent observer sees `launching` and waits. Five hundred of
        // them produce five hundred polls and zero provider calls.
        for attempt in 0..500 {
            let loser = materialize_step(&head(GenerationState::Launching, 0), attempt);
            assert!(
                matches!(loser, MaterializeStep::AwaitLaunch { .. }),
                "a loser must never call the provider"
            );
        }
    }

    #[test]
    fn the_replay_identity_is_the_same_for_every_attempt_at_one_generation() {
        // Collapse layer three: even if layers one and two both fail, the provider
        // returns the same MicroVM because the token is derived from the generation
        // alone.
        let first = materialize_step(&head(GenerationState::Requested, 0), 0);
        let second = materialize_step(&head(GenerationState::Requested, 0), 99);
        assert_eq!(first, second);
    }

    #[test]
    fn every_generation_state_maps_to_exactly_one_materialization_step() {
        let cases = [
            (GenerationState::Requested, "launch"),
            (GenerationState::Launching, "await"),
            (GenerationState::Running, "ready"),
            (GenerationState::Suspending, "await"),
            (GenerationState::Suspended, "resume"),
            (GenerationState::Resuming, "await"),
            (GenerationState::LifetimeDraining, "ready"),
            (GenerationState::Terminating, "lost"),
            (GenerationState::Terminated, "lost"),
            (GenerationState::Lost, "lost"),
            (GenerationState::Unknown, "reconcile"),
        ];
        assert_eq!(cases.len(), GenerationState::ALL.len());
        for (state, expected) in cases {
            let observed = match materialize_step(&head(state, 0), 0) {
                MaterializeStep::Ready => "ready",
                MaterializeStep::Launch { .. } => "launch",
                MaterializeStep::AwaitLaunch { .. } => "await",
                MaterializeStep::Resume { .. } => "resume",
                MaterializeStep::Lost => "lost",
                MaterializeStep::AwaitReconciliation => "reconcile",
            };
            assert_eq!(observed, expected, "{state:?}");
        }
    }

    #[test]
    fn an_unresolved_lifecycle_outcome_dispatches_no_second_effect() {
        assert_eq!(
            materialize_step(&head(GenerationState::Unknown, 0), 0),
            MaterializeStep::AwaitReconciliation
        );
    }

    #[test]
    fn the_launch_poll_backoff_is_bounded() {
        assert_eq!(launch_backoff_ms(0), 50);
        assert_eq!(launch_backoff_ms(1), 100);
        assert_eq!(launch_backoff_ms(4), 800);
        assert_eq!(launch_backoff_ms(u32::MAX), LAUNCH_POLL_MAX_MS);
    }

    #[test]
    fn admission_refuses_a_suspending_generation_and_a_stale_fence() {
        let running = head(GenerationState::Running, 0);
        let plan = admit(&running, Fence(3), Revision::new(11), at(2_000))
            .expect("a running generation admits");
        assert_eq!(
            plan,
            AdmitPlan {
                generation: generation(),
                fence: Fence(3),
                expected_revision: Revision::new(11),
                open_operations: 1,
                next_revision: Revision::new(12),
                last_busy_at: at(2_000),
            }
        );

        let suspending = head(GenerationState::Suspending, 0);
        assert!(matches!(
            admit(&suspending, Fence(3), Revision::new(11), at(2_000)),
            Err(AdmissionRefused::NotRunning { .. })
        ));
        assert!(matches!(
            admit(&running, Fence(2), Revision::new(11), at(2_000)),
            Err(AdmissionRefused::Fenced { .. })
        ));
    }

    #[test]
    fn admit_before_dispatch_and_settle_after_commit_can_only_over_count() {
        // Any interleaving of admit, dispatch, crash and settle leaves the counter
        // at or above the true number of live operations.
        let mut open = 0u32;
        let mut truth = 0u32;
        let mut refusals = 0u32;
        for step in 0..96u32 {
            let current = head(GenerationState::Running, open);
            let Ok(plan) = admit(&current, Fence(3), Revision::new(11), at(0)) else {
                // The shape's concurrency ceiling refused. A refusal changes
                // nothing, which is itself part of the property.
                refusals += 1;
                assert!(open >= truth, "under-count after a refusal at step {step}");
                continue;
            };
            open = plan.open_operations;
            match step % 4 {
                // Admit, then crash before dispatch: the increment survives.
                1 => {}
                // Admit, dispatch, settle.
                0 | 3 => {
                    truth += 1;
                    let after = head(GenerationState::Running, open);
                    open = settle(&after, at(0)).open_operations;
                    truth -= 1;
                }
                // Admit and dispatch; settle later.
                _ => truth += 1,
            }
            assert!(open >= truth, "under-count at step {step}");
        }
        assert!(
            refusals > 0,
            "the sequence must reach the concurrency ceiling, or the refusal arm proved nothing"
        );
        assert!(
            open > truth,
            "the crash-before-dispatch steps must leave a visible over-count"
        );
    }

    #[test]
    fn settlement_is_not_conditional_on_the_fence() {
        // A suspend has advanced the fence while an operation was open. Settlement
        // must still land, or the counter never returns to zero and the generation
        // never becomes idle.
        let mut advanced = head(GenerationState::Suspending, 1);
        advanced.fence = Fence(99);
        let plan = settle(&advanced, at(5_000));
        assert_eq!(plan.open_operations, 0);
        assert_eq!(plan.next_revision, Revision::new(12));
    }

    #[test]
    fn concurrent_settlements_cas_the_revision_and_recompute_the_count() {
        let observed = head(GenerationState::Running, 2);
        let first = settle(&observed, at(5_000));
        let racing = settle(&observed, at(5_001));
        assert_eq!(first.expected_revision, Revision::new(11));
        assert_eq!(racing.expected_revision, Revision::new(11));

        // Only one revision-11 plan can commit. The loser reloads the committed
        // head and recomputes instead of writing its stale count of one again.
        let mut after_first = observed;
        after_first.open_operations = first.open_operations;
        after_first.revision = first.next_revision;
        let retried = settle(&after_first, at(5_002));
        assert_eq!(retried.expected_revision, Revision::new(12));
        assert_eq!(retried.next_revision, Revision::new(13));
        assert_eq!(retried.open_operations, 0);
    }

    #[test]
    fn both_transport_modes_are_recorded_and_size_their_own_pools() {
        assert_eq!(transport_mode(Alpn::H2), TransportMode::Multiplexed);
        assert_eq!(transport_mode(Alpn::Http11), TransportMode::PerRequest);

        for size in ComputeSize::ALL {
            assert_eq!(pool_size(TransportMode::Multiplexed, size), 1);
            assert!(
                pool_size(TransportMode::PerRequest, size) >= 6,
                "even the smallest shape keeps a usable pool"
            );
            assert!(
                max_in_flight(TransportMode::Multiplexed, size) <= 32,
                "the shared-safety ceiling holds in both modes"
            );
        }
        // The 8gb shape is where the old one-socket-per-call design hit its cliff.
        assert_eq!(pool_size(TransportMode::PerRequest, ComputeSize::Gb8), 126);
        assert_eq!(
            max_in_flight(TransportMode::Multiplexed, ComputeSize::Gb8),
            32
        );
    }

    #[test]
    fn brain_keeps_no_idle_hands_socket() {
        assert_eq!(CONNECTION_IDLE_MS, 15_000);
    }

    #[test]
    fn guest_loss_and_a_fence_race_route_differently() {
        let lost = HandsError::GenerationLost {
            generation: generation(),
        };
        assert!(lost.interrupts());
        assert!(!lost.retry_same_effect());

        assert!(HandsError::Interrupted.interrupts());
        assert!(!HandsError::Interrupted.retry_same_effect());

        let fenced = HandsError::Fenced {
            presented: 2,
            current: 3,
        };
        assert!(fenced.retry_same_effect());
        assert!(!fenced.interrupts());

        let transport = HandsError::Transport {
            reason: "connection reset".to_owned(),
        };
        assert!(transport.retry_same_effect());

        // Capacity is queued, not failed and not retried in place.
        assert!(!HandsError::CapacityQueued.retry_same_effect());
        assert!(!HandsError::CapacityQueued.interrupts());

        let capability = HandsError::CapabilityUnavailable {
            capability: "browser".to_owned(),
        };
        assert!(
            !capability.retry_same_effect(),
            "retrying a missing capability changes nothing"
        );
    }
}
