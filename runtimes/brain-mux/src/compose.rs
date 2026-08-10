//! The composition itself: three runtimes, one health thread, one drain.
//!
//! Everything here is wiring. The behaviour lives in the library crates, and the point of
//! this module is that the *shape* — which scheduler runs what, what is bounded by what,
//! what happens on `SIGTERM` — is one readable thing rather than scattered across a `main`.

use crate::admission::{ActivationResources, Admission, AdmissionBounds};
use crate::cache::{CachePolicy, ConfiguredFoldCache};
use crate::control::HealthState;
use crate::drain::Stage;
use crate::runtime::{ComputeLane, RuntimeShape};
use crate::scale::ScaleBounds;
use aex_brain_app::kernel::{ActivationRegistry, DrainGate, PermitKind, PermitSet};
use std::collections::BTreeMap;
use std::sync::Arc;

/// The bound above which the reactor is considered wedged.
///
/// The load gate is p99 at or below 50 ms; liveness fails above it, because a reactor that
/// late is not going to answer anything else either.
pub const REACTOR_DELAY_BOUND_MS: u32 = 50;

/// How often the reactor's lateness is sampled.
pub const REACTOR_TICK: core::time::Duration = core::time::Duration::from_millis(100);

/// Every bounded resource one task holds, with the ceilings it was configured with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Envelope {
    /// The task's whole memory envelope.
    pub task_memory_bytes: u64,
    /// Bytes reserved for hydrated context.
    pub context_bytes: u64,
    /// Bytes reserved for stream buffers.
    pub stream_buffer_bytes: u64,
    /// Bytes reserved for the warm cache, budgeted separately from accepted work.
    pub warm_cache_bytes: u64,
    /// Bytes left as emergency headroom, reserved to nobody.
    pub headroom_bytes: u64,
    /// Concurrently open provider streams.
    pub provider_streams: u64,
    /// Concurrent outbound tool network calls, weighted by the tool's declared
    /// concurrency weight.
    pub network_lane: u64,
    /// Concurrent Hands RPCs per session generation.
    pub hands_rpcs: u64,
}

impl Envelope {
    /// The build-bound launch shape, split into independently bounded pools.
    ///
    /// The 3 GiB context pool proves 48 simultaneous 64 MiB worst-case restores. Stream
    /// buffers retain their independent 128 MiB pool; cache cannot borrow admitted memory;
    /// 384 MiB remains unavailable to all permits as process/allocator failure headroom.
    ///
    /// The network lane is 128 units, derived rather than chosen: the safety cap admits 32
    /// activations and the heaviest declared tool weight is 4, so 128 admits one outstanding
    /// network call per admitted activation — the whole of what a serial driver can reach.
    /// It is deliberately not the authoritative bound on tool spend. That belongs where a
    /// refusal can be per-workspace and answered rather than silently deferred; this is a
    /// coarse backstop against this task's own memory, which is what a permit should be.
    #[must_use]
    pub const fn candidate_launch() -> Self {
        const MIB: u64 = 1_024 * 1_024;
        Self {
            task_memory_bytes: crate::task_shape::task_memory_bytes(),
            context_bytes: 3_072 * MIB,
            stream_buffer_bytes: 128 * MIB,
            warm_cache_bytes: 512 * MIB,
            headroom_bytes: 384 * MIB,
            provider_streams: 48,
            network_lane: 128,
            hands_rpcs: 48,
        }
    }

    /// Whether the split fits inside the envelope.
    ///
    /// # Errors
    ///
    /// Returns the reason when the reservations plus the headroom exceed the task's memory.
    /// An envelope that over-commits would let every pool be granted in full and the task
    /// still be killed by the kernel, which is the one failure the reservations exist to
    /// prevent.
    pub const fn validate(&self) -> Result<(), &'static str> {
        let committed = self
            .context_bytes
            .saturating_add(self.stream_buffer_bytes)
            .saturating_add(self.warm_cache_bytes)
            .saturating_add(self.headroom_bytes);
        if committed > self.task_memory_bytes {
            return Err("the memory pools plus headroom exceed the task envelope");
        }
        if self.headroom_bytes == 0 {
            return Err("an envelope with no headroom has nothing left to fail into");
        }
        Ok(())
    }

    /// The bytes the accepted-work pools may grant.
    ///
    /// The warm cache is excluded: its budget is separate on purpose, so the cache can never
    /// borrow memory that admitted work has already been promised.
    #[must_use]
    pub const fn accepted_work_bytes(&self) -> u64 {
        self.context_bytes.saturating_add(self.stream_buffer_bytes)
    }
}

/// Everything the process holds for its whole life.
#[derive(Debug)]
pub struct Composition {
    /// Every bounded resource.
    pub permits: Arc<PermitSet>,
    /// The keyed single-flight map over activations.
    pub registry: Arc<ActivationRegistry>,
    /// The one-way admission stop.
    pub drain: Arc<DrainGate>,
    /// The admission controller.
    pub admission: Arc<Admission>,
    /// The bounded compute lane.
    pub lane: Arc<ComputeLane>,
    /// What the health responder reads.
    pub health: Arc<HealthState>,
    /// The runtimes' shape.
    pub shape: RuntimeShape,
    /// The memory envelope.
    pub envelope: Envelope,
    /// The warm cache's lifecycle.
    pub cache: CachePolicy,
    /// The exact-revision process-local fold cache.
    pub fold_cache: Arc<ConfiguredFoldCache>,
    /// The scale-out bounds.
    pub scale: ScaleBounds,
    /// The activation policy whose demand was proven against this envelope.
    pub policy: aex_brain_app::activation::ActivationPolicy,
}

/// Why the composition was refused.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("brain-mux cannot compose: {reason}")]
pub struct CompositionError {
    /// What was wrong.
    pub reason: &'static str,
}

impl Composition {
    /// Builds the composition, refusing anything that cannot hold together.
    ///
    /// # Errors
    ///
    /// [`CompositionError`] when the admission bands are inverted or the memory split
    /// over-commits the envelope. Both fail startup rather than at the moment the
    /// over-commitment matters, which is always the worst moment.
    pub fn build(
        bounds: AdmissionBounds,
        envelope: Envelope,
        cache: CachePolicy,
        scale: ScaleBounds,
        shape: RuntimeShape,
        policy: aex_brain_app::activation::ActivationPolicy,
    ) -> Result<Self, CompositionError> {
        bounds
            .validate()
            .map_err(|reason| CompositionError { reason })?;
        envelope
            .validate()
            .map_err(|reason| CompositionError { reason })?;

        let resources = ActivationResources {
            context_bytes: policy.restore_resident_bytes,
            stream_buffer_bytes: u64::try_from(policy.stream_buffer_bytes).unwrap_or(u64::MAX),
            provider_streams: 1,
            network_lane: 1,
            hands_rpcs: 1,
        };
        resources
            .validate()
            .map_err(|reason| CompositionError { reason })?;
        validate_target_capacity(
            bounds.target,
            envelope,
            resources,
            policy.max_concurrent_drives,
        )?;

        let permits = Arc::new(PermitSet::new(BTreeMap::from([
            (PermitKind::Activation, u64::from(bounds.safety_cap)),
            (PermitKind::ProviderStream, envelope.provider_streams),
            (PermitKind::NetworkLane, envelope.network_lane),
            (PermitKind::HandsRpc, envelope.hands_rpcs),
            (PermitKind::ComputeLane, shape.compute_permits),
            (PermitKind::ContextBytes, envelope.context_bytes),
            (PermitKind::StreamBufferBytes, envelope.stream_buffer_bytes),
            (PermitKind::WarmCacheBytes, envelope.warm_cache_bytes),
        ])));
        let drain = Arc::new(DrainGate::new());
        let fold_cache = Arc::new(ConfiguredFoldCache::new(cache));
        Ok(Self {
            admission: Arc::new(Admission::new(
                bounds,
                Arc::clone(&permits),
                Arc::clone(&drain),
                resources,
            )),
            lane: Arc::new(ComputeLane::new(Arc::clone(&permits))),
            registry: Arc::new(ActivationRegistry::new()),
            health: HealthState::starting(REACTOR_DELAY_BOUND_MS, bounds.safety_cap),
            permits,
            drain,
            shape,
            envelope,
            cache,
            fold_cache,
            scale,
            policy,
        })
    }

    /// Starts drain, in the order [`Stage::ORDER`] declares.
    ///
    /// Readiness fails first, so the load balancer stops sending work before anything is
    /// abandoned. Liveness is deliberately untouched: failing it would have the orchestrator
    /// kill the task along with the non-replayable effects it is trying to finish.
    #[must_use]
    pub fn begin_drain(&self) -> Stage {
        self.health.start_drain();
        self.drain.start_drain();
        Stage::FailReadiness
    }

    /// Whether the process may exit: draining and nothing in flight.
    #[must_use]
    pub fn is_quiesced(&self) -> bool {
        self.drain.is_quiesced()
    }
}

fn validate_target_capacity(
    target: u32,
    envelope: Envelope,
    resources: ActivationResources,
    scheduler_width: usize,
) -> Result<(), CompositionError> {
    let target = u64::from(target);
    if resources.context_bytes.saturating_mul(target) > envelope.context_bytes {
        return Err(CompositionError {
            reason: "the admission target exceeds reserved context capacity",
        });
    }
    if resources.stream_buffer_bytes.saturating_mul(target) > envelope.stream_buffer_bytes {
        return Err(CompositionError {
            reason: "the admission target exceeds reserved stream-buffer capacity",
        });
    }
    if resources.provider_streams.saturating_mul(target) > envelope.provider_streams {
        return Err(CompositionError {
            reason: "the admission target exceeds provider-stream capacity",
        });
    }
    if resources.network_lane.saturating_mul(target) > envelope.network_lane {
        return Err(CompositionError {
            reason: "the admission target exceeds network-lane capacity",
        });
    }
    if resources.hands_rpcs.saturating_mul(target) > envelope.hands_rpcs {
        return Err(CompositionError {
            reason: "the admission target exceeds Hands RPC capacity",
        });
    }
    if u64::try_from(scheduler_width).unwrap_or(u64::MAX) < target {
        return Err(CompositionError {
            reason: "the scheduler width is below the admission target",
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{
        Composition, Envelope, REACTOR_DELAY_BOUND_MS, REACTOR_TICK, validate_target_capacity,
    };
    use crate::admission::{ActivationResources, AdmissionBounds};
    use crate::cache::CachePolicy;
    use crate::drain::Stage;
    use crate::health::{LIVE_PATH, READY_PATH};
    use crate::runtime::RuntimeShape;
    use crate::scale::ScaleBounds;
    use aex_brain_app::kernel::PermitKind;

    fn scale() -> ScaleBounds {
        ScaleBounds {
            min_tasks: 1,
            max_tasks: 10,
            target_work_seconds_per_task: 10.0,
        }
    }

    fn composition() -> Composition {
        let policy = launch_policy();
        Composition::build(
            AdmissionBounds::default(),
            Envelope::candidate_launch(),
            CachePolicy::default(),
            scale(),
            RuntimeShape::for_parallelism(2),
            policy,
        )
        .expect("the candidate launch shape composes")
    }

    fn launch_policy() -> aex_brain_app::activation::ActivationPolicy {
        aex_brain_app::activation::ActivationPolicy {
            receive_batch: 1,
            max_concurrent_drives: 48,
            ..aex_brain_app::activation::ActivationPolicy::default()
        }
    }

    /// The candidate shape is the one PERF-02 will confirm or replace, so it is pinned.
    #[test]
    fn the_candidate_launch_envelope_is_the_recorded_split() {
        const MIB: u64 = 1_024 * 1_024;
        let envelope = Envelope::candidate_launch();
        assert_eq!(envelope.task_memory_bytes, 4_096 * MIB);
        assert_eq!(envelope.context_bytes, 3_072 * MIB);
        assert_eq!(envelope.stream_buffer_bytes, 128 * MIB);
        assert_eq!(envelope.warm_cache_bytes, 512 * MIB);
        assert_eq!(envelope.headroom_bytes, 384 * MIB);
        assert_eq!(envelope.provider_streams, 48);
        assert_eq!(envelope.network_lane, 128);
        assert_eq!(envelope.hands_rpcs, 48);
        assert!(envelope.validate().is_ok());
    }

    /// An over-committed envelope would let every pool be granted in full and the task still
    /// be killed by the kernel, which is the one failure reservations exist to prevent.
    #[test]
    fn an_over_committed_envelope_is_refused_at_startup() {
        let envelope = Envelope {
            context_bytes: Envelope::candidate_launch().task_memory_bytes,
            ..Envelope::candidate_launch()
        };
        assert!(envelope.validate().is_err());
        assert!(
            Composition::build(
                AdmissionBounds::default(),
                envelope,
                CachePolicy::default(),
                scale(),
                RuntimeShape::for_parallelism(2),
                launch_policy()
            )
            .is_err()
        );
    }

    #[test]
    fn an_envelope_with_no_headroom_is_refused() {
        let envelope = Envelope {
            headroom_bytes: 0,
            ..Envelope::candidate_launch()
        };
        assert!(envelope.validate().is_err());
    }

    #[test]
    fn inverted_admission_bands_are_refused_at_startup() {
        assert!(
            Composition::build(
                AdmissionBounds {
                    target: 200,
                    safety_cap: 100,
                    offered_ceiling: 500
                },
                Envelope::candidate_launch(),
                CachePolicy::default(),
                scale(),
                RuntimeShape::for_parallelism(2),
                launch_policy()
            )
            .is_err()
        );
    }

    /// The scheduler may run only the target proven simultaneously by every resource pool.
    /// Raising a single number cannot silently overrun context, stream, provider or Hands.
    #[test]
    fn launch_target_is_the_minimum_composite_capacity() {
        const MIB: u64 = 1_024 * 1_024;
        let envelope = Envelope::candidate_launch();
        let resources = ActivationResources {
            context_bytes: 64 * MIB,
            stream_buffer_bytes: MIB,
            provider_streams: 1,
            network_lane: 1,
            hands_rpcs: 1,
        };
        assert_eq!(envelope.context_bytes / resources.context_bytes, 48);
        assert_eq!(
            envelope.stream_buffer_bytes / resources.stream_buffer_bytes,
            128
        );
        assert_eq!(envelope.provider_streams / resources.provider_streams, 48);
        assert_eq!(envelope.network_lane / resources.network_lane, 128);
        assert_eq!(envelope.hands_rpcs / resources.hands_rpcs, 48);
        assert!(validate_target_capacity(48, envelope, resources, 48).is_ok());
        assert_eq!(
            validate_target_capacity(49, envelope, resources, 49)
                .expect_err("the 49th worst-case restore is not reserved")
                .reason,
            "the admission target exceeds reserved context capacity"
        );
        assert_eq!(
            validate_target_capacity(48, envelope, resources, 47)
                .expect_err("47 scheduler lanes cannot honestly advertise target 48")
                .reason,
            "the scheduler width is below the admission target"
        );
        for (resources, reason) in [
            (
                ActivationResources {
                    context_bytes: 1,
                    stream_buffer_bytes: 3 * MIB,
                    provider_streams: 1,
                    network_lane: 1,
                    hands_rpcs: 1,
                },
                "the admission target exceeds reserved stream-buffer capacity",
            ),
            (
                ActivationResources {
                    context_bytes: 1,
                    stream_buffer_bytes: 1,
                    provider_streams: 2,
                    network_lane: 1,
                    hands_rpcs: 1,
                },
                "the admission target exceeds provider-stream capacity",
            ),
            (
                ActivationResources {
                    context_bytes: 1,
                    stream_buffer_bytes: 1,
                    provider_streams: 1,
                    network_lane: 3,
                    hands_rpcs: 1,
                },
                "the admission target exceeds network-lane capacity",
            ),
            (
                ActivationResources {
                    context_bytes: 1,
                    stream_buffer_bytes: 1,
                    provider_streams: 1,
                    network_lane: 1,
                    hands_rpcs: 2,
                },
                "the admission target exceeds Hands RPC capacity",
            ),
        ] {
            assert_eq!(
                validate_target_capacity(48, envelope, resources, 48)
                    .expect_err("the targeted resource is underprovisioned")
                    .reason,
                reason
            );
        }
        let composition = composition();
        assert_eq!(
            composition.admission.bounds().target,
            AdmissionBounds::default().target,
            "the composed target is the approved alpha profile, not the envelope's ceiling"
        );
        assert_eq!(composition.policy.max_concurrent_drives, 48);
        assert_eq!(composition.policy.receive_batch, 1);
    }

    /// The cache's byte budget is a separate permit from accepted work's, so the cache can
    /// never borrow memory an admitted activation has already been promised.
    #[test]
    fn the_cache_budget_is_separate_from_accepted_work() {
        let composition = composition();
        let envelope = composition.envelope;
        assert_eq!(
            composition.permits.limit(PermitKind::WarmCacheBytes),
            envelope.warm_cache_bytes
        );
        assert_eq!(
            composition.permits.limit(PermitKind::ContextBytes)
                + composition.permits.limit(PermitKind::StreamBufferBytes),
            envelope.accepted_work_bytes()
        );
    }

    #[test]
    fn every_bounded_resource_is_configured() {
        let composition = composition();
        for kind in [
            PermitKind::Activation,
            PermitKind::ProviderStream,
            PermitKind::NetworkLane,
            PermitKind::HandsRpc,
            PermitKind::ComputeLane,
            PermitKind::ContextBytes,
            PermitKind::StreamBufferBytes,
            PermitKind::WarmCacheBytes,
        ] {
            assert!(
                composition.permits.limit(kind) > 0,
                "{kind:?} is unconfigured, so it would be unavailable rather than unbounded"
            );
        }
    }

    /// Readiness fails the instant drain starts; liveness keeps passing so the task is not
    /// killed along with the effects it is finishing.
    #[test]
    fn drain_fails_readiness_first_and_never_touches_liveness() {
        let composition = composition();
        composition.health.bindings_validated();
        composition.health.catalog_verified();
        composition.health.store_reachable(true);
        composition.health.schema_matched();
        assert_eq!(composition.health.respond("GET", READY_PATH).status, 200);

        assert_eq!(composition.begin_drain(), Stage::FailReadiness);
        assert_eq!(composition.health.respond("GET", READY_PATH).status, 503);
        assert_eq!(composition.health.respond("GET", LIVE_PATH).status, 200);
        assert!(composition.drain.is_draining());
        assert!(composition.is_quiesced(), "nothing was in flight");
    }

    #[test]
    fn a_process_with_work_in_flight_is_not_quiesced() {
        let composition = composition();
        let permit = composition.drain.try_admit().expect("not draining yet");
        let _ = composition.begin_drain();
        assert!(!composition.is_quiesced());
        drop(permit);
        assert!(composition.is_quiesced());
    }

    #[test]
    fn the_reactor_bound_matches_the_declared_budget() {
        assert_eq!(REACTOR_DELAY_BOUND_MS, 50);
        assert_eq!(REACTOR_TICK.as_millis(), 100);
    }
}
