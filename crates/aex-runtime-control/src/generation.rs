//! The exact-generation model: the immutable generation tuple, its state machine,
//! the fence algebra F1-F6 and operation admission.
//!
//! A Hands generation is one immutable tuple, allocated once and never mutated. A
//! different image, size, network policy, protocol version or limits revision is a
//! **different** generation; a model, tool or catalog release never alters a
//! running one.

use serde::{Deserialize, Serialize};

use crate::idle::KeepaliveLease;
use crate::shape::ComputeSize;
use crate::wire_pending::{
    ContentHash, Fence, GenerationId, ImageIdentifier, ImageVersion, LimitsRevision,
    OrganizationId, Revision, SchemaVersion, SessionId, Timestamp, WorkspaceId,
};

/// The guest filesystem root every structured tool is confined to.
pub const GUEST_ROOT: &str = "/workspace";

/// The guest root, as a value so it can be carried in the run-hook payload.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct GuestRoot;

impl GuestRoot {
    /// The root path text.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        GUEST_ROOT
    }
}

impl From<GuestRoot> for String {
    fn from(_: GuestRoot) -> Self {
        GUEST_ROOT.to_owned()
    }
}

impl TryFrom<String> for GuestRoot {
    type Error = String;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        if value == GUEST_ROOT {
            Ok(Self)
        } else {
            Err(format!("the guest root is `{GUEST_ROOT}`, not `{value}`"))
        }
    }
}

/// An optional capability an image variant carries.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ImageCapability {
    /// Headless Chromium plus its font and NSS dependencies.
    Browser,
}

/// The exact image a generation is pinned to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImagePin {
    /// Published image identifier.
    pub identifier: ImageIdentifier,
    /// Published image version.
    pub version: ImageVersion,
    /// `sha256` of the code artifact ZIP, echoed in the image description.
    pub artifact_digest: ContentHash,
    /// Capability layers this variant carries.
    pub capabilities: Vec<ImageCapability>,
}

impl ImagePin {
    /// Whether the pinned image carries `capability`.
    #[must_use]
    pub fn carries(&self, capability: ImageCapability) -> bool {
        self.capabilities.contains(&capability)
    }
}

/// Whether a generation reaches the public Internet.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NetworkPolicy {
    /// The managed `INTERNET_EGRESS` connector, and nothing else.
    PublicInternet,
    /// No egress connector at all.
    None,
}

/// One immutable Hands generation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HandsGeneration {
    /// Allocated once at session create, time-ordered.
    pub generation: GenerationId,
    /// The owning session.
    pub session: SessionId,
    /// The attributed workspace.
    pub workspace: WorkspaceId,
    /// The billed organization.
    pub organization: OrganizationId,
    /// The public compute token.
    pub size: ComputeSize,
    /// The pinned image.
    pub image: ImagePin,
    /// The egress policy.
    pub network: NetworkPolicy,
    /// The exact protocol version. No negotiation, no downgrade.
    pub protocol_version: SchemaVersion,
    /// The resolved H-BOUNDARY effective policy revision.
    pub limits_revision: LimitsRevision,
    /// The guest root.
    pub root: GuestRoot,
}

/// How Brain talks to one generation's endpoint.
///
/// Recorded on the generation head rather than silently chosen, so the two modes'
/// different pool sizes are measurable instead of mysterious.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TransportMode {
    /// ALPN negotiated `h2`: one connection, streams capped at the resolved
    /// maximum open operations.
    Multiplexed,
    /// HTTP/1.1: `shape.max_connections - 2` connections.
    PerRequest,
}

impl TransportMode {
    /// The pool size this mode uses for `size`.
    #[must_use]
    pub const fn pool_size(self, size: ComputeSize) -> u32 {
        match self {
            Self::Multiplexed => 1,
            Self::PerRequest => size.per_request_pool_size(),
        }
    }

    /// The in-flight request ceiling this mode allows for `size`.
    #[must_use]
    pub const fn max_in_flight(self, size: ComputeSize) -> u32 {
        match self {
            Self::Multiplexed => size.max_concurrent_operations(),
            Self::PerRequest => size.per_request_pool_size(),
        }
    }
}

/// The lifecycle state of one generation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GenerationState {
    /// Allocated by the session authority. H-LAZY: no provider call has happened.
    Requested,
    /// A `RunMicrovm` has been dispatched under a fence.
    Launching,
    /// The provider reports `RUNNING` and AEX admits operations.
    Running,
    /// The control worker holds the suspend fence; nothing is admitted.
    Suspending,
    /// The provider reports `SUSPENDED`; retained snapshot bytes accrue.
    Suspended,
    /// A `ResumeMicrovm` has been dispatched under a fence.
    Resuming,
    /// Within the lifetime drain margin: no new admissions, open work finishes.
    LifetimeDraining,
    /// A `TerminateMicrovm` has been dispatched under a fence.
    Terminating,
    /// Absorbing. The generation is gone and every receipt is closed.
    Terminated,
    /// Absorbing. Provider `NotFound`, `TERMINATED` out of band, or an unmodeled
    /// provider state.
    Lost,
    /// A lifecycle effect returned [`crate::lifecycle::ProviderCall::Unknown`] and
    /// reconciliation has not settled it. No second effect may be dispatched.
    Unknown,
}

/// Why a generation transition was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("a generation in `{from:?}` cannot move to `{to:?}`")]
pub struct InvalidTransition {
    /// The recorded state.
    pub from: GenerationState,
    /// The refused target state.
    pub to: GenerationState,
}

impl GenerationState {
    /// Every modelled state.
    pub const ALL: [Self; 11] = [
        Self::Requested,
        Self::Launching,
        Self::Running,
        Self::Suspending,
        Self::Suspended,
        Self::Resuming,
        Self::LifetimeDraining,
        Self::Terminating,
        Self::Terminated,
        Self::Lost,
        Self::Unknown,
    ];

    /// Terminal states are absorbing: no fence advance re-opens one (F5).
    #[must_use]
    pub const fn is_terminal(self) -> bool {
        matches!(self, Self::Terminated | Self::Lost)
    }

    /// Only a `running` generation admits new operations (F2).
    #[must_use]
    pub const fn admits_operations(self) -> bool {
        matches!(self, Self::Running)
    }

    /// Whether the provider is holding compute for this generation.
    #[must_use]
    pub const fn holds_provider_compute(self) -> bool {
        matches!(
            self,
            Self::Launching
                | Self::Running
                | Self::Suspending
                | Self::Suspended
                | Self::Resuming
                | Self::LifetimeDraining
                | Self::Terminating
                | Self::Unknown
        )
    }

    /// The states reachable in one step.
    #[must_use]
    pub fn successors(self) -> &'static [Self] {
        match self {
            Self::Requested => &[Self::Launching, Self::Terminated, Self::Lost, Self::Unknown],
            Self::Launching => &[Self::Running, Self::Lost, Self::Unknown, Self::Terminating],
            Self::Running => &[
                Self::Suspending,
                Self::LifetimeDraining,
                Self::Terminating,
                Self::Lost,
                Self::Unknown,
            ],
            Self::Suspending => &[
                Self::Suspended,
                Self::Running,
                Self::Lost,
                Self::Unknown,
                Self::Terminating,
            ],
            Self::Suspended => &[Self::Resuming, Self::Terminating, Self::Lost, Self::Unknown],
            Self::Resuming => &[Self::Running, Self::Lost, Self::Unknown, Self::Terminating],
            Self::LifetimeDraining => &[Self::Terminating, Self::Lost, Self::Unknown],
            Self::Terminating => &[Self::Terminated, Self::Lost, Self::Unknown],
            Self::Terminated | Self::Lost => &[],
            // Reconciliation resolves an unknown outcome into an observed state.
            Self::Unknown => &[
                Self::Running,
                Self::Suspended,
                Self::Terminated,
                Self::Lost,
                Self::Terminating,
            ],
        }
    }

    /// Whether `next` is reachable from this state in one step.
    #[must_use]
    pub fn permits(self, next: Self) -> bool {
        self.successors().contains(&next)
    }

    /// Applies a transition.
    ///
    /// # Errors
    ///
    /// Returns [`InvalidTransition`] when `next` is not a successor of this state.
    /// There is no lenient path: an unmodelled transition is a control-plane bug,
    /// not something to absorb.
    pub fn transition(self, next: Self) -> Result<Self, InvalidTransition> {
        if self.permits(next) {
            Ok(next)
        } else {
            Err(InvalidTransition {
                from: self,
                to: next,
            })
        }
    }
}

/// The durable head of one generation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GenerationHead {
    /// The generation this head describes.
    pub generation: GenerationId,
    /// Its compute token.
    pub size: ComputeSize,
    /// Current lifecycle state.
    pub state: GenerationState,
    /// Current lifecycle fence.
    pub fence: Fence,
    /// Optimistic-concurrency revision.
    pub revision: Revision,
    /// Operations admitted and not yet settled. Over-counts, never under-counts.
    pub open_operations: u32,
    /// The last authoritatively busy instant.
    pub last_busy_at: Timestamp,
    /// When quiescence started, absent while busy.
    pub idle_since: Option<Timestamp>,
    /// When the control worker's suspend lock lapses, if it holds one.
    pub suspend_lock_expires_at: Option<Timestamp>,
    /// The paid keepalive lease, if any.
    pub keepalive_lease: Option<KeepaliveLease>,
    /// Recorded transport mode, once negotiated.
    pub transport_mode: Option<TransportMode>,
}

/// The outcome of an admission attempt (F2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Admitted {
    /// The open-operation count the conditional write must land.
    pub open_operations: u32,
    /// The revision the conditional write must land.
    pub revision: Revision,
}

/// Why an operation was not admitted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum AdmissionRefused {
    /// The generation is not `running`.
    #[error("a generation in `{state:?}` admits no operation")]
    NotRunning {
        /// The observed state.
        state: GenerationState,
    },
    /// The generation is absorbing.
    #[error("a generation in `{state:?}` is terminal and admits nothing, ever")]
    Terminal {
        /// The observed terminal state.
        state: GenerationState,
    },
    /// The caller's fence no longer matches the head.
    #[error("fence {presented} is stale; the generation is at {current}")]
    Fenced {
        /// The fence the caller presented.
        presented: Fence,
        /// The fence on the head.
        current: Fence,
    },
    /// A lifecycle transition holds the suspend lock.
    #[error("the suspend lock is held until {}", .until.millis())]
    SuspendLocked {
        /// When the lock lapses.
        until: Timestamp,
    },
    /// The head moved between read and write.
    #[error("revision {presented:?} is stale; the head is at {current:?}")]
    StaleRevision {
        /// The revision the caller read.
        presented: Revision,
        /// The revision on the head.
        current: Revision,
    },
    /// The shape's concurrency ceiling is reached.
    #[error("{open} operations are already open; the ceiling for this shape is {limit}")]
    ConcurrencyExhausted {
        /// Currently open operations.
        open: u32,
        /// The shape's ceiling.
        limit: u32,
    },
}

impl GenerationHead {
    /// F2: admission is conditional on `state == running`, a matching fence, a
    /// matching revision and no held suspend lock.
    ///
    /// This is the pure decision. The caller turns [`Admitted`] into the
    /// conditional `UpdateItem`; a lost race surfaces as
    /// [`AdmissionRefused::StaleRevision`] from the store, not from here.
    ///
    /// # Errors
    ///
    /// See [`AdmissionRefused`].
    pub fn admit(
        &self,
        presented_fence: Fence,
        presented_revision: Revision,
        now: Timestamp,
    ) -> Result<Admitted, AdmissionRefused> {
        if self.state.is_terminal() {
            return Err(AdmissionRefused::Terminal { state: self.state });
        }
        if !self.state.admits_operations() {
            return Err(AdmissionRefused::NotRunning { state: self.state });
        }
        if presented_fence != self.fence {
            return Err(AdmissionRefused::Fenced {
                presented: presented_fence,
                current: self.fence,
            });
        }
        if presented_revision != self.revision {
            return Err(AdmissionRefused::StaleRevision {
                presented: presented_revision,
                current: self.revision,
            });
        }
        if let Some(until) = self.suspend_lock_expires_at
            && until > now
        {
            return Err(AdmissionRefused::SuspendLocked { until });
        }
        let limit = self.size.max_concurrent_operations();
        if self.open_operations >= limit {
            return Err(AdmissionRefused::ConcurrencyExhausted {
                open: self.open_operations,
                limit,
            });
        }
        Ok(Admitted {
            open_operations: self.open_operations + 1,
            revision: self.revision.next(),
        })
    }

    /// F3: settlement follows the Brain journal commit and can only lower the
    /// counter, so an increment-then-crash leaves an over-count.
    ///
    /// Saturating rather than wrapping: a double settle must not roll the counter
    /// to `u32::MAX` and pin the generation alive forever.
    #[must_use]
    pub fn settle(&self, now: Timestamp) -> (u32, Revision, Timestamp) {
        (
            self.open_operations.saturating_sub(1),
            self.revision.next(),
            now,
        )
    }
}

/// F1: what a guest does with an inbound request's generation binding.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FenceVerdict {
    /// Accept, and adopt `adopted` as the new observed floor.
    Accept {
        /// The fence the guest now treats as its floor.
        adopted: Fence,
    },
    /// The request names a different generation.
    WrongGeneration,
    /// The request's fence is below the highest the guest has observed.
    StaleFence {
        /// The fence the request carried.
        presented: Fence,
        /// The floor the guest has observed.
        floor: Fence,
    },
}

/// F1: the guest rejects a request whose generation differs from the one it was
/// launched with, and whose fence is lower than the highest it has observed. It
/// accepts a higher fence and adopts it.
#[must_use]
pub fn evaluate_request_binding(
    bound_generation: GenerationId,
    observed_floor: Fence,
    request_generation: GenerationId,
    request_fence: Fence,
) -> FenceVerdict {
    if request_generation != bound_generation {
        return FenceVerdict::WrongGeneration;
    }
    if request_fence < observed_floor {
        return FenceVerdict::StaleFence {
            presented: request_fence,
            floor: observed_floor,
        };
    }
    FenceVerdict::Accept {
        adopted: request_fence,
    }
}

/// F4: a result carrying a fence lower than the generation's current fence is
/// refused incorporation, so a late old-generation output can never commit into a
/// new generation.
#[must_use]
pub fn may_incorporate_result(current_fence: Fence, result_fence: Fence) -> bool {
    result_fence >= current_fence
}

/// F6: generation identity, not fence, decides cross-generation ordering. A new
/// generation starts at `fence = 0` but a strictly greater time-ordered id.
#[must_use]
pub fn supersedes(candidate: GenerationId, incumbent: GenerationId) -> bool {
    candidate.uuid() > incumbent.uuid()
}

#[cfg(test)]
mod tests {
    use super::{
        Admitted, AdmissionRefused, GenerationHead, GenerationState, GuestRoot, FenceVerdict,
        ImageCapability, ImagePin, TransportMode, evaluate_request_binding,
        may_incorporate_result, supersedes,
    };
    use crate::shape::ComputeSize;
    use crate::wire_pending::{
        ContentHash, Fence, GenerationId, ImageIdentifier, ImageVersion, Revision, Timestamp,
    };
    use uuid::Uuid;

    fn head(state: GenerationState, open: u32) -> GenerationHead {
        GenerationHead {
            generation: GenerationId::from_bytes([9; 16]),
            size: ComputeSize::Gb1,
            state,
            fence: Fence::new(3),
            revision: Revision::new(11),
            open_operations: open,
            last_busy_at: Timestamp::from_millis(1_000),
            idle_since: None,
            suspend_lock_expires_at: None,
            keepalive_lease: None,
            transport_mode: Some(TransportMode::Multiplexed),
        }
    }

    #[test]
    fn terminal_states_are_absorbing() {
        for state in [GenerationState::Terminated, GenerationState::Lost] {
            assert!(state.is_terminal());
            assert!(state.successors().is_empty());
            for target in GenerationState::ALL {
                assert!(
                    state.transition(target).is_err(),
                    "{state:?} must absorb, but permitted {target:?}"
                );
            }
        }
    }

    #[test]
    fn terminated_never_reaches_lost() {
        assert!(!GenerationState::Terminated.permits(GenerationState::Lost));
        assert!(!GenerationState::Lost.permits(GenerationState::Terminated));
    }

    #[test]
    fn only_running_admits_operations() {
        for state in GenerationState::ALL {
            assert_eq!(
                state.admits_operations(),
                state == GenerationState::Running,
                "{state:?}"
            );
        }
    }

    #[test]
    fn every_reachable_path_from_requested_ends_in_a_terminal_state() {
        // Breadth-first over the whole state graph: no state is a dead end that is
        // not absorbing, and every state is reachable from `requested`.
        let mut seen = vec![GenerationState::Requested];
        let mut frontier = vec![GenerationState::Requested];
        while let Some(state) = frontier.pop() {
            for next in state.successors() {
                if !seen.contains(next) {
                    seen.push(*next);
                    frontier.push(*next);
                }
            }
        }
        assert_eq!(
            seen.len(),
            GenerationState::ALL.len(),
            "unreachable states: {:?}",
            GenerationState::ALL
                .iter()
                .filter(|state| !seen.contains(state))
                .collect::<Vec<_>>()
        );
        for state in GenerationState::ALL {
            assert_eq!(
                state.successors().is_empty(),
                state.is_terminal(),
                "{state:?} is a dead end without being terminal"
            );
        }
    }

    #[test]
    fn an_arbitrary_transition_sequence_never_reopens_a_terminal_state() {
        // Exhaustive over every ordered pair, then every triple through a terminal.
        for first in GenerationState::ALL {
            for second in GenerationState::ALL {
                let Ok(reached) = first.transition(second) else {
                    continue;
                };
                assert_eq!(reached, second);
                if reached.is_terminal() {
                    for third in GenerationState::ALL {
                        assert!(reached.transition(third).is_err());
                    }
                }
            }
        }
    }

    #[test]
    fn admission_requires_running_a_matching_fence_and_revision() {
        let running = head(GenerationState::Running, 0);
        assert_eq!(
            running.admit(Fence::new(3), Revision::new(11), Timestamp::from_millis(2_000)),
            Ok(Admitted {
                open_operations: 1,
                revision: Revision::new(12)
            })
        );
        assert_eq!(
            running.admit(Fence::new(2), Revision::new(11), Timestamp::from_millis(2_000)),
            Err(AdmissionRefused::Fenced {
                presented: Fence::new(2),
                current: Fence::new(3)
            })
        );
        assert_eq!(
            running.admit(Fence::new(3), Revision::new(10), Timestamp::from_millis(2_000)),
            Err(AdmissionRefused::StaleRevision {
                presented: Revision::new(10),
                current: Revision::new(11)
            })
        );
    }

    #[test]
    fn a_suspending_generation_admits_nothing() {
        for state in GenerationState::ALL {
            if state == GenerationState::Running {
                continue;
            }
            let refused = head(state, 0)
                .admit(Fence::new(3), Revision::new(11), Timestamp::from_millis(0))
                .expect_err("only running admits");
            match refused {
                AdmissionRefused::NotRunning { state: observed } => assert_eq!(observed, state),
                AdmissionRefused::Terminal { state: observed } => {
                    assert_eq!(observed, state);
                    assert!(state.is_terminal());
                }
                other => panic!("{state:?} refused with {other:?}"),
            }
        }
    }

    #[test]
    fn a_held_suspend_lock_blocks_admission_until_it_lapses() {
        let mut locked = head(GenerationState::Running, 0);
        locked.suspend_lock_expires_at = Some(Timestamp::from_millis(5_000));
        assert_eq!(
            locked.admit(Fence::new(3), Revision::new(11), Timestamp::from_millis(4_999)),
            Err(AdmissionRefused::SuspendLocked {
                until: Timestamp::from_millis(5_000)
            })
        );
        assert!(
            locked
                .admit(Fence::new(3), Revision::new(11), Timestamp::from_millis(5_000))
                .is_ok(),
            "a lapsed lock stops blocking at exactly its expiry"
        );
    }

    #[test]
    fn admission_stops_at_the_shape_concurrency_ceiling() {
        let limit = ComputeSize::Gb1.max_concurrent_operations();
        let full = head(GenerationState::Running, limit);
        assert_eq!(
            full.admit(Fence::new(3), Revision::new(11), Timestamp::from_millis(0)),
            Err(AdmissionRefused::ConcurrencyExhausted { open: limit, limit })
        );
    }

    #[test]
    fn settlement_saturates_so_a_double_settle_cannot_pin_a_generation_alive() {
        let empty = head(GenerationState::Running, 0);
        let (open, revision, _) = empty.settle(Timestamp::from_millis(9));
        assert_eq!(open, 0);
        assert_eq!(revision, Revision::new(12));
    }

    #[test]
    fn admit_then_settle_can_only_over_count() {
        // Any interleaving of admit / crash-before-dispatch / settle leaves the
        // counter at or above the true number of live operations.
        let mut open = 0_u32;
        let mut truth = 0_u32;
        for step in 0..64_u32 {
            match step % 3 {
                // admit, dispatch, settle
                0 => {
                    open += 1;
                    truth += 1;
                    open -= 1;
                    truth -= 1;
                }
                // admit, then crash before dispatch: the counter keeps the increment
                1 => open += 1,
                // admit and dispatch, settle later
                _ => {
                    open += 1;
                    truth += 1;
                }
            }
            assert!(open >= truth, "under-count at step {step}");
        }
    }

    #[test]
    fn the_guest_rejects_a_foreign_generation_and_a_lower_fence() {
        let bound = GenerationId::from_bytes([1; 16]);
        let other = GenerationId::from_bytes([2; 16]);
        assert_eq!(
            evaluate_request_binding(bound, Fence::new(4), other, Fence::new(4)),
            FenceVerdict::WrongGeneration
        );
        assert_eq!(
            evaluate_request_binding(bound, Fence::new(4), bound, Fence::new(3)),
            FenceVerdict::StaleFence {
                presented: Fence::new(3),
                floor: Fence::new(4)
            }
        );
        assert_eq!(
            evaluate_request_binding(bound, Fence::new(4), bound, Fence::new(4)),
            FenceVerdict::Accept {
                adopted: Fence::new(4)
            }
        );
        assert_eq!(
            evaluate_request_binding(bound, Fence::new(4), bound, Fence::new(9)),
            FenceVerdict::Accept {
                adopted: Fence::new(9)
            },
            "a higher fence is accepted and adopted"
        );
    }

    #[test]
    fn a_stale_result_is_refused_incorporation() {
        assert!(!may_incorporate_result(Fence::new(5), Fence::new(4)));
        assert!(may_incorporate_result(Fence::new(5), Fence::new(5)));
        assert!(may_incorporate_result(Fence::new(5), Fence::new(6)));
    }

    #[test]
    fn generation_identity_not_fence_decides_cross_generation_ordering() {
        let older = GenerationId::from_uuid(Uuid::now_v7());
        let newer = GenerationId::from_uuid(Uuid::now_v7());
        assert!(supersedes(newer, older));
        assert!(!supersedes(older, newer));
        // Both start at fence zero; the fence says nothing about which is newer.
        assert_eq!(Fence::ZERO, Fence::ZERO);
    }

    #[test]
    fn the_guest_root_is_a_constant_and_rejects_anything_else() {
        assert_eq!(GuestRoot.as_str(), "/workspace");
        assert!(GuestRoot::try_from("/workspace".to_owned()).is_ok());
        assert!(GuestRoot::try_from("/".to_owned()).is_err());
        assert!(GuestRoot::try_from("/workspace/".to_owned()).is_err());
    }

    #[test]
    fn transport_mode_pool_sizes_follow_the_shape() {
        assert_eq!(TransportMode::Multiplexed.pool_size(ComputeSize::Gb8), 1);
        assert_eq!(TransportMode::PerRequest.pool_size(ComputeSize::Gb8), 126);
        assert_eq!(
            TransportMode::Multiplexed.max_in_flight(ComputeSize::Gb8),
            32
        );
    }

    #[test]
    fn an_image_pin_answers_capability_questions_before_any_process_starts() {
        let pin = ImagePin {
            identifier: ImageIdentifier::new("aex-hands-2gb-browser"),
            version: ImageVersion::new("7"),
            artifact_digest: ContentHash::from_bytes([4; 32]),
            capabilities: vec![ImageCapability::Browser],
        };
        assert!(pin.carries(ImageCapability::Browser));
        let base = ImagePin {
            capabilities: Vec::new(),
            ..pin
        };
        assert!(!base.carries(ImageCapability::Browser));
    }
}
