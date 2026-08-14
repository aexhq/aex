//! Concrete Brain-to-Hands composition.
//!
//! Durable generation state remains in `runtime-activity`; process-local state is limited to
//! bounded generation-partitioned endpoint/token leases and weak single-flight locks. Provider
//! idempotency on `aexgen-{generation}` is the final launch-collapse layer, so losing these maps on
//! restart can add latency but cannot create a second `MicroVM`.

use core::time::Duration;
use std::collections::HashMap;
use std::num::NonZeroUsize;
use std::sync::{Arc, Weak};
use std::time::{SystemTime, UNIX_EPOCH};

use aex_brain_app::ports::{
    BoxFuture, DispatchTicket, HandsAccepted, HandsEndpoint, HandsError, HandsOperationStart,
    HandsOperationStatus, HandsResult, HandsSandboxFile, ProviderFailureKind, RedactedDetail,
    ResultBounds,
};
use aex_brain_domain::effect::{DispatchProof, DispatchStage};
use aex_brain_domain::ids::{
    ContentHash as BrainContentHash, Fence as BrainFence, HandsOperationId as BrainOperationId,
    SessionId as BrainSessionId, Timestamp as BrainTimestamp,
};
use aex_hands_control_aws::{
    AGENT_PORT, EndpointToken, MAX_DURATION_SECONDS, MicrovmControlApi, MicrovmDescription,
    RunHookBounds, RunHookPayload, RunRequest, TOKEN_TTL_SECONDS,
};
use aex_hands_protocol::files::{FileRequest, FileResponse};
use aex_hands_protocol::lifecycle::ProviderRequestId;
use aex_hands_protocol::operation::{
    DeliveryMode, OperationBounds, OperationExit, OperationRequest, TerminalMetadata, TerminalState,
};
use aex_hands_protocol::rpc::{
    AttachResponse, CallHash, CancelReason, CancelRequest, CancelResponse, HandsOperationId,
    HelloRequest, HelloResponse, ResultChunk, ResultRequest, ResultResponse, StartRequest,
    StartResponse, StatusRequest, StatusResponse, Verb,
};
use aex_runtime_control::generation::{
    AdmissionRefused, GenerationState, HandsGeneration, ImageCapability, next_fence,
};
use aex_runtime_control::idle::TRUE_IDLE_THRESHOLD_MS;
use aex_runtime_control::lifecycle::{
    IntentState, LifecycleAction, LifecycleIntentId, MicrovmId, ProviderCall, ProviderState,
    client_token,
};
use aex_runtime_control::shape::ShapeCapacity as _;
use aex_runtime_control::store::{
    GenerationAccountingPlan, GenerationPlan, GenerationView, LifecycleIntentPlan,
    LifecycleReceiptPlan, LifecycleRequestPlan, ReadConsistency, RuntimeActivityStore,
    RuntimeStoreError,
};
use aex_runtime_control_aws::{
    AWAIT_BUDGET_MS, CommandOutcome, RuntimeControl, Settled, intent_id,
};
use aex_wire::ids::{ContentHash, GenerationId, PrefixedId as _, SessionId, Uuid7};
use aex_wire::types::Timestamp;
use tokio::sync::Mutex;

use crate::lease::{EndpointLeaseCache, LeaseHandle, LeaseIdentity};
use crate::live_file::{LiveFileBackend, LiveFileReply, LiveGenerationReady};
use crate::{
    AuthenticatedGuestEndpoint, HttpGuestTransport, MAX_FRAME_BYTES, MAX_RESULT_BODY_BYTES,
    ResultAssembly, admit, admit_native_resume, launch_backoff_ms, settle,
};

mod attached;

pub use attached::delivery_for;

const MATERIALIZE_ATTEMPTS: u32 = 64;
const STORE_ATTEMPTS: usize = 16;
const STATUS_TIMEOUT: Duration = Duration::from_secs(5);
/// Opening or closing an exact descriptor may hash the full five-GiB file once
/// on the smallest offered CPU. This stays below the public ALB's 1,200-second
/// idle ceiling and bounds a guest call after its edge request disappears.
const FILE_RPC_TIMEOUT: Duration = Duration::from_mins(10);
const FILE_BATCH_MAX_REQUESTS: usize = 10;
const LIVE_RESULT_PREVIEW_BYTES: usize = 65_536;
const RESULT_CHUNK_BYTES: u64 = aex_hands_protocol::rpc::MAX_RESULT_CHUNK_BYTES;
const RESULT_PULL_ATTEMPTS: usize = 32;
const ENDPOINT_LEASE_CACHE_CAPACITY: usize = 128;

struct ActivityAdmission {
    view: GenerationView,
    endpoint: LeaseHandle<AuthenticatedGuestEndpoint>,
    native_resume: bool,
}

/// The first poll interval after a start or for a young operation.
const POLL_FLOOR_MS: u64 = 250;

/// The poll ceiling a long-running operation grows to.
///
/// The growth must come from this guest-side hint: the application floor
/// (`DETACHED_POLL_FLOOR`) only takes the maximum of floor and hint, so a
/// hardcoded 250 ms here polled every detached operation four times a second
/// for its whole life.
const POLL_CEILING_MS: u64 = 4_000;

/// The requested poll interval for an operation of the given age.
///
/// Grows linearly from the floor — an eighth of the elapsed age — to the
/// ceiling: a shell command is polled tightly through its first seconds, a
/// half-hour build settles at one poll every four seconds.
fn poll_after(age_ms: u64) -> Duration {
    Duration::from_millis(
        POLL_FLOOR_MS
            .saturating_add(age_ms / 8)
            .min(POLL_CEILING_MS),
    )
}

/// Production implementation of Brain's lower Hands backend.
pub struct ProductionHandsBackend {
    store: Arc<dyn RuntimeActivityStore>,
    provider: Arc<dyn MicrovmControlApi>,
    runtime: Arc<RuntimeControl>,
    guest: HttpGuestTransport,
    flights: Mutex<HashMap<GenerationId, Weak<Mutex<()>>>>,
    lease_flights: Mutex<HashMap<GenerationId, Weak<Mutex<()>>>>,
    leases: Mutex<EndpointLeaseCache<AuthenticatedGuestEndpoint>>,
}

impl ProductionHandsBackend {
    /// Composes the backend from the shared runtime authority, provider adapter,
    /// and runtime-control engine used for billing-correct resume transitions.
    ///
    /// # Errors
    ///
    /// Returns a pre-dispatch transport error when the bounded HTTPS client
    /// cannot be constructed.
    pub fn new(
        store: Arc<dyn RuntimeActivityStore>,
        provider: Arc<dyn MicrovmControlApi>,
        runtime: Arc<RuntimeControl>,
    ) -> Result<Self, HandsError> {
        let lease_capacity = NonZeroUsize::new(ENDPOINT_LEASE_CACHE_CAPACITY).ok_or_else(|| {
            pre_dispatch(
                ProviderFailureKind::InvalidRequest,
                "the endpoint lease cache capacity must be positive",
            )
        })?;
        Ok(Self {
            store,
            provider,
            runtime,
            guest: HttpGuestTransport::new()?,
            flights: Mutex::new(HashMap::new()),
            lease_flights: Mutex::new(HashMap::new()),
            leases: Mutex::new(EndpointLeaseCache::new(lease_capacity)),
        })
    }

    async fn flight(&self, generation: GenerationId) -> Arc<Mutex<()>> {
        let mut flights = self.flights.lock().await;
        flights.retain(|_, lock| lock.strong_count() != 0);
        if let Some(lock) = flights.get(&generation).and_then(Weak::upgrade) {
            return lock;
        }
        let lock = Arc::new(Mutex::new(()));
        flights.insert(generation, Arc::downgrade(&lock));
        lock
    }

    async fn lease_flight(&self, generation: GenerationId) -> Arc<Mutex<()>> {
        let mut flights = self.lease_flights.lock().await;
        flights.retain(|_, lock| lock.strong_count() != 0);
        if let Some(lock) = flights.get(&generation).and_then(Weak::upgrade) {
            return lock;
        }
        let lock = Arc::new(Mutex::new(()));
        flights.insert(generation, Arc::downgrade(&lock));
        lock
    }

    async fn load_view(
        &self,
        generation: GenerationId,
        consistency: ReadConsistency,
    ) -> Result<GenerationView, HandsError> {
        self.store
            .load_generation_view(generation, consistency)
            .await
            .map_err(store_error)?
            .ok_or(HandsError::GenerationLost { generation })
    }

    fn require_view(
        view: &GenerationView,
        session: SessionId,
        generation: GenerationId,
    ) -> Result<(), HandsError> {
        if view.head.generation != generation || view.definition.generation != generation {
            return Err(HandsError::GenerationMismatch {
                expected: generation,
                found: view.head.generation,
            });
        }
        if view.session != session || view.definition.session != session {
            return Err(pre_dispatch(
                ProviderFailureKind::Authentication,
                "the generation does not belong to the requested session",
            ));
        }
        Ok(())
    }

    async fn prepare_generation(
        &self,
        session: SessionId,
        generation: GenerationId,
    ) -> Result<GenerationView, HandsError> {
        let flight = self.flight(generation).await;
        let _guard = flight.lock().await;
        for attempt in 0..MATERIALIZE_ATTEMPTS {
            let view = self.load_view(generation, ReadConsistency::Strong).await?;
            Self::require_view(&view, session, generation)?;
            match view.head.state {
                GenerationState::Running
                | GenerationState::Suspended
                | GenerationState::LifetimeDraining => return Ok(view),
                GenerationState::Requested => {
                    // A completed launch hands back the running description and
                    // an endpoint token, so the lease is built without another
                    // provider probe. `None` means another writer holds the
                    // launch; loop and observe it.
                    if let Some((description, token)) = self.launch(&view).await?
                        && let Some((ready, _)) = self
                            .lease_from_launch(session, generation, description, token)
                            .await?
                    {
                        return Ok(ready);
                    }
                }
                GenerationState::Launching | GenerationState::Unknown
                    if view
                        .open_intent
                        .as_ref()
                        .is_some_and(|intent| intent.action == LifecycleAction::Launch) =>
                {
                    let (description, token) = self.recover_launch(&view).await?;
                    if let Some((ready, _)) = self
                        .lease_from_launch(session, generation, description, token)
                        .await?
                    {
                        return Ok(ready);
                    }
                }
                GenerationState::Resuming
                    if view
                        .open_intent
                        .as_ref()
                        .is_some_and(|intent| intent.action == LifecycleAction::NativeResume) =>
                {
                    self.reconcile_native_resume(&view).await?;
                }
                GenerationState::Launching
                | GenerationState::Resuming
                | GenerationState::Suspending => {
                    tokio::time::sleep(Duration::from_millis(launch_backoff_ms(attempt))).await;
                }
                GenerationState::Terminating
                | GenerationState::Terminated
                | GenerationState::Lost => {
                    self.leases.lock().await.invalidate(generation);
                    return Err(HandsError::GenerationLost { generation });
                }
                GenerationState::Unknown => {
                    return Err(dispatched(
                        ProviderFailureKind::Transport,
                        "the generation has an unresolved lifecycle outcome",
                    ));
                }
            }
        }
        Err(dispatched(
            ProviderFailureKind::Timeout,
            "the generation did not become reachable within the bounded materialization wait",
        ))
    }

    async fn materialize(
        &self,
        session: SessionId,
        generation: GenerationId,
    ) -> Result<(GenerationView, LeaseHandle<AuthenticatedGuestEndpoint>), HandsError> {
        let view = self.prepare_generation(session, generation).await?;
        if view.head.state != GenerationState::Running {
            return Err(pre_dispatch(
                ProviderFailureKind::InvalidRequest,
                "a suspended generation requires a durable activity identity before native resume",
            ));
        }
        let endpoint = self.connect(&view).await?;
        Ok((view, endpoint))
    }

    async fn launch(
        &self,
        view: &GenerationView,
    ) -> Result<Option<(MicrovmDescription, EndpointToken)>, HandsError> {
        let request = launch_request(&view.definition)?;
        let fence = next_fence(view.head.fence);
        let intent_id = intent_id(view.head.generation, LifecycleAction::Launch, fence);
        let commit = match self
            .store
            .record_intent(&LifecycleIntentPlan {
                intent_id: intent_id.clone(),
                generation: view.head.generation,
                microvm: None,
                action: LifecycleAction::Launch,
                fence,
                expected_state: view.head.state,
                expected_fence: view.head.fence,
                expected_revision: view.head.revision,
                next_state: GenerationState::Launching,
                dispatched_at: now()?,
            })
            .await
        {
            Ok(commit) => commit,
            Err(
                RuntimeStoreError::RevisionConflict { .. } | RuntimeStoreError::IntentOpen { .. },
            ) => return Ok(None),
            Err(error) => return Err(store_error(error)),
        };
        match self.provider.run(&request).await {
            Ok(description) => self
                .finish_launch(view, &commit.generation.head, &intent_id, description)
                .await
                .map(Some),
            Err(call) => {
                self.close_failed_launch(view, &commit.generation.head, &intent_id, &call)
                    .await?;
                Err(provider_error(&call, view.head.generation, true))
            }
        }
    }

    async fn recover_launch(
        &self,
        view: &GenerationView,
    ) -> Result<(MicrovmDescription, EndpointToken), HandsError> {
        let intent = view.open_intent.as_ref().ok_or_else(|| {
            pre_dispatch(
                ProviderFailureKind::ProtocolViolation,
                "a transitional launch head has no lifecycle intent",
            )
        })?;
        if intent.action != LifecycleAction::Launch {
            return Err(dispatched(
                ProviderFailureKind::ProtocolViolation,
                "the unresolved lifecycle intent is not a launch",
            ));
        }
        if let (Some(microvm), Some(request)) = (&intent.microvm, &intent.provider_request_id) {
            let description = self
                .await_running(view.head.generation, microvm, None)
                .await?;
            // The settlement write and the token mint are independent, so they
            // run concurrently; the settle outcome is checked first because a
            // token for an unsettled launch is worthless.
            let (settled, token) = tokio::join!(
                self.settle_launch(
                    view,
                    &intent.intent_id,
                    microvm,
                    request,
                    description.launched_at,
                ),
                self.provider
                    .auth_token(microvm, TOKEN_TTL_SECONDS, &[AGENT_PORT])
            );
            settled?;
            let token = token.map_err(|call| provider_error(&call, view.head.generation, false))?;
            return Ok((description, token));
        }
        let description = self
            .provider
            .run(&launch_request(&view.definition)?)
            .await
            .map_err(|call| provider_error(&call, view.head.generation, true))?;
        self.finish_launch(view, &view.head, &intent.intent_id, description)
            .await
    }

    async fn finish_launch(
        &self,
        view: &GenerationView,
        current: &aex_runtime_control::generation::GenerationHead,
        intent_id: &LifecycleIntentId,
        description: MicrovmDescription,
    ) -> Result<(MicrovmDescription, EndpointToken), HandsError> {
        let request = description.request_id.clone().ok_or_else(|| {
            dispatched(
                ProviderFailureKind::ProtocolViolation,
                "RunMicrovm returned no provider request identity",
            )
        })?;
        self.store
            .record_provider_request(&LifecycleRequestPlan {
                intent_id: intent_id.clone(),
                generation: view.head.generation,
                microvm: description.microvm.clone(),
                provider_request_id: request.clone(),
            })
            .await
            .map_err(possibly_sent_store)?;
        let microvm = description.microvm.clone();
        let running = self
            .await_running(view.head.generation, &microvm, Some(description))
            .await?;
        let recovered = GenerationView {
            head: current.clone(),
            microvm: Some(running.microvm.clone()),
            ..view.clone()
        };
        // The settlement write and the token mint are independent, so they run
        // concurrently; the settle outcome is checked first because a token
        // for an unsettled launch is worthless.
        let (settled, token) = tokio::join!(
            self.settle_launch(
                &recovered,
                intent_id,
                &running.microvm,
                &request,
                running.launched_at,
            ),
            self.provider
                .auth_token(&running.microvm, TOKEN_TTL_SECONDS, &[AGENT_PORT])
        );
        settled?;
        let token = token.map_err(|call| provider_error(&call, view.head.generation, false))?;
        Ok((running, token))
    }

    async fn await_running(
        &self,
        generation: GenerationId,
        microvm: &MicrovmId,
        initial: Option<MicrovmDescription>,
    ) -> Result<MicrovmDescription, HandsError> {
        let mut elapsed = 0_u64;
        let mut observed = initial;
        loop {
            let description = match observed.take() {
                Some(description) => description,
                None => self
                    .provider
                    .get(microvm)
                    .await
                    .map_err(|call| provider_error(&call, generation, false))?,
            };
            if description.microvm != *microvm {
                return Err(dispatched(
                    ProviderFailureKind::ProtocolViolation,
                    "GetMicrovm answered for a different provider identity",
                ));
            }
            match description.state {
                ProviderState::Running => return Ok(description),
                ProviderState::Pending if elapsed < AWAIT_BUDGET_MS => {
                    let interval = aex_hands_control_aws::poll_interval_ms(LifecycleAction::Launch);
                    tokio::time::sleep(Duration::from_millis(interval)).await;
                    elapsed = elapsed.saturating_add(interval);
                }
                ProviderState::Terminated | ProviderState::Terminating => {
                    return Err(HandsError::GenerationLost { generation });
                }
                _ => {
                    return Err(dispatched(
                        ProviderFailureKind::ProtocolViolation,
                        "the provider reported an impossible launch state",
                    ));
                }
            }
            if elapsed >= AWAIT_BUDGET_MS {
                return Err(dispatched(
                    ProviderFailureKind::Timeout,
                    "RunMicrovm did not reach running within the bounded await",
                ));
            }
        }
    }

    async fn settle_launch(
        &self,
        view: &GenerationView,
        intent_id: &LifecycleIntentId,
        microvm: &MicrovmId,
        request: &ProviderRequestId,
        launched_at: Option<Timestamp>,
    ) -> Result<(), HandsError> {
        let launched_at = launched_at.ok_or_else(|| {
            dispatched(
                ProviderFailureKind::ProtocolViolation,
                "the running MicroVM has no provider launch instant",
            )
        })?;
        let at = now()?;
        self.store
            .settle_intent(&LifecycleReceiptPlan {
                intent_id: intent_id.clone(),
                generation: view.head.generation,
                next_intent_state: IntentState::Settled,
                provider_request_id: Some(request.clone()),
                observed_state: Some(ProviderState::Running),
                snapshot: None,
                usage: Vec::new(),
                generation_commit: Some(GenerationPlan {
                    generation: view.head.generation,
                    expected_state: view.head.state,
                    expected_fence: view.head.fence,
                    expected_revision: view.head.revision,
                    next_state: GenerationState::Running,
                    next_fence: view.head.fence,
                    microvm: Some(microvm.clone()),
                    transport_mode: view.head.transport_mode,
                    accounting: Some(GenerationAccountingPlan {
                        accounted_from: launched_at,
                        suspended_at: None,
                        snapshot_ordinal: view.snapshot_ordinal,
                        lifetime_started_at: Some(launched_at),
                    }),
                    at,
                }),
                settled_at: at,
            })
            .await
            .map(|_| ())
            .map_err(possibly_sent_store)
    }

    async fn close_failed_launch(
        &self,
        view: &GenerationView,
        current: &aex_runtime_control::generation::GenerationHead,
        intent_id: &LifecycleIntentId,
        call: &ProviderCall,
    ) -> Result<(), HandsError> {
        let at = now()?;
        let unknown = matches!(call, ProviderCall::Unknown { .. });
        self.store
            .settle_intent(&LifecycleReceiptPlan {
                intent_id: intent_id.clone(),
                generation: view.head.generation,
                next_intent_state: if unknown {
                    IntentState::Unknown
                } else {
                    IntentState::Settled
                },
                provider_request_id: None,
                observed_state: None,
                snapshot: None,
                usage: Vec::new(),
                generation_commit: Some(GenerationPlan {
                    generation: current.generation,
                    expected_state: current.state,
                    expected_fence: current.fence,
                    expected_revision: current.revision,
                    next_state: if unknown {
                        GenerationState::Unknown
                    } else {
                        GenerationState::Requested
                    },
                    next_fence: current.fence,
                    microvm: None,
                    transport_mode: current.transport_mode,
                    accounting: None,
                    at,
                }),
                settled_at: at,
            })
            .await
            .map(|_| ())
            .map_err(possibly_sent_store)
    }

    async fn reconcile_native_resume(&self, view: &GenerationView) -> Result<(), HandsError> {
        let microvm = view.microvm.as_ref().ok_or_else(|| {
            dispatched(
                ProviderFailureKind::ProtocolViolation,
                "a native-resuming generation has no provider identity",
            )
        })?;
        let description = self
            .provider
            .get(microvm)
            .await
            .map_err(|call| provider_error(&call, view.head.generation, false))?;
        if description.microvm != *microvm {
            return Err(dispatched(
                ProviderFailureKind::ProtocolViolation,
                "native resume observation answered for a different MicroVM",
            ));
        }
        match self
            .runtime
            .observe_native_resume(view, description.state, now()?)
            .await
        {
            CommandOutcome::Settled(Settled::Resumed | Settled::Raced) => Ok(()),
            CommandOutcome::Settled(Settled::Lost | Settled::AlreadyTerminal { .. }) => {
                Err(HandsError::GenerationLost {
                    generation: view.head.generation,
                })
            }
            CommandOutcome::Retry { .. } => Err(pre_dispatch(
                ProviderFailureKind::ServerError,
                "native resume observation lost its bounded authority race",
            )),
            CommandOutcome::Poison { .. } => Err(pre_dispatch(
                ProviderFailureKind::ProtocolViolation,
                "runtime control rejected native resume evidence",
            )),
            CommandOutcome::Settled(_) => Err(dispatched(
                ProviderFailureKind::ProtocolViolation,
                "runtime control returned an incoherent native resume outcome",
            )),
        }
    }

    async fn settle_native_activity(&self, view: &GenerationView) -> Result<(), HandsError> {
        match self
            .runtime
            .observe_native_resume(view, ProviderState::Running, now()?)
            .await
        {
            CommandOutcome::Settled(Settled::Resumed | Settled::Raced) => Ok(()),
            CommandOutcome::Settled(Settled::Lost | Settled::AlreadyTerminal { .. }) => {
                Err(HandsError::GenerationLost {
                    generation: view.head.generation,
                })
            }
            CommandOutcome::Retry { .. } => Err(dispatched(
                ProviderFailureKind::ServerError,
                "native resume usage settlement lost its bounded authority race",
            )),
            CommandOutcome::Poison { .. } => Err(dispatched(
                ProviderFailureKind::ProtocolViolation,
                "native resume response could not settle its durable evidence",
            )),
            CommandOutcome::Settled(_) => Err(dispatched(
                ProviderFailureKind::ProtocolViolation,
                "native resume response produced an incoherent settlement",
            )),
        }
    }

    async fn connect(
        &self,
        view: &GenerationView,
    ) -> Result<LeaseHandle<AuthenticatedGuestEndpoint>, HandsError> {
        let microvm = view.microvm.as_ref().ok_or_else(|| {
            dispatched(
                ProviderFailureKind::ProtocolViolation,
                "a running generation has no provider identity",
            )
        })?;
        let identity = LeaseIdentity {
            generation: view.head.generation,
            fence: view.head.fence,
            microvm: microvm.clone(),
        };
        if let Some(lease) = self.leases.lock().await.get(&identity, now()?) {
            return Ok(lease);
        }
        let flight = self.lease_flight(view.head.generation).await;
        let _guard = flight.lock().await;
        if let Some(lease) = self.leases.lock().await.get(&identity, now()?) {
            return Ok(lease);
        }
        // The description probe and the token mint are independent provider
        // calls, so a cold connect pays one round trip instead of two in
        // series. A token minted for a VM the probe then disqualifies simply
        // expires unused.
        let (described, token) = tokio::join!(
            self.provider.get(microvm),
            self.provider
                .auth_token(microvm, TOKEN_TTL_SECONDS, &[AGENT_PORT])
        );
        let description =
            described.map_err(|call| provider_error(&call, view.head.generation, false))?;
        if description.microvm != *microvm || description.state != ProviderState::Running {
            self.leases.lock().await.invalidate(view.head.generation);
            return Err(dispatched(
                ProviderFailureKind::ProtocolViolation,
                "the provider does not report the exact generation as running",
            ));
        }
        let endpoint = description.endpoint.ok_or_else(|| {
            dispatched(
                ProviderFailureKind::ProtocolViolation,
                "the running MicroVM has no authenticated endpoint",
            )
        })?;
        let token = token.map_err(|call| provider_error(&call, view.head.generation, false))?;
        let expires_at = token.expires_at;
        let endpoint = AuthenticatedGuestEndpoint::new(
            view.head.generation,
            view.head.fence,
            endpoint,
            token,
        )?;
        Ok(self
            .leases
            .lock()
            .await
            .insert(identity, expires_at, endpoint))
    }

    /// Builds the endpoint lease straight from a launch this caller just
    /// completed, skipping the probe `connect` would repeat.
    ///
    /// One fresh strong read anchors the lease identity: recording the launch
    /// intent advanced the fence, so a lease built from the pre-launch view
    /// would disagree with every subsequent request and grind through
    /// endpoint-lease invalidations. `None` means the head moved again while
    /// the launch settled; the materialize loop observes the new state.
    async fn lease_from_launch(
        &self,
        session: SessionId,
        generation: GenerationId,
        description: MicrovmDescription,
        token: EndpointToken,
    ) -> Result<Option<(GenerationView, LeaseHandle<AuthenticatedGuestEndpoint>)>, HandsError> {
        let view = self.load_view(generation, ReadConsistency::Strong).await?;
        Self::require_view(&view, session, generation)?;
        if !matches!(
            view.head.state,
            GenerationState::Running | GenerationState::LifetimeDraining
        ) || view.microvm.as_ref() != Some(&description.microvm)
        {
            return Ok(None);
        }
        // A RunMicrovm response that reached Running without an endpoint is
        // not an error here: the loop's Running arm connects through the
        // ordinary probe, which requires one of a GET response.
        let Some(endpoint) = description.endpoint else {
            return Ok(None);
        };
        let identity = LeaseIdentity {
            generation,
            fence: view.head.fence,
            microvm: description.microvm.clone(),
        };
        let expires_at = token.expires_at;
        let endpoint =
            AuthenticatedGuestEndpoint::new(generation, view.head.fence, endpoint, token)?;
        let lease = self
            .leases
            .lock()
            .await
            .insert(identity, expires_at, endpoint);
        Ok(Some((view, lease)))
    }

    async fn call_guest<Request, Response>(
        &self,
        lease: &LeaseHandle<AuthenticatedGuestEndpoint>,
        verb: Verb,
        request: &Request,
        timeout: Duration,
    ) -> Result<Response, HandsError>
    where
        Request: serde::Serialize + ?Sized,
        Response: serde::de::DeserializeOwned,
    {
        match self.guest.call(&lease.value, verb, request, timeout).await {
            Ok(reply) => Ok(reply),
            Err(error) => {
                if should_invalidate_lease(&error) {
                    self.leases.lock().await.invalidate_lease(lease);
                }
                Err(error)
            }
        }
    }

    /// Executes one ordered bounded guest batch as one durable HTTP activity.
    ///
    /// Live multipart transfer may require several guest calls, but its
    /// public request still owns exactly one [`HandsOperationId`]. This method
    /// admits that identity once, reuses one exact-generation lease, observes
    /// every bounded reply, and settles runtime activity/accounting only
    /// after the complete batch succeeds. A failure after any dispatched call
    /// deliberately leaves the activity open so an exact retry can reconcile
    /// the guest's idempotent transfer manifest under the same identity.
    pub(crate) async fn call_native_activity_batch<Request, Response>(
        &self,
        session: SessionId,
        generation: GenerationId,
        activity: HandsOperationId,
        verb: Verb,
        requests: &[Request],
        timeout: Duration,
    ) -> Result<(Vec<Response>, bool), HandsError>
    where
        Request: serde::Serialize,
        Response: serde::de::DeserializeOwned,
    {
        if requests.is_empty() {
            return Err(pre_dispatch(
                ProviderFailureKind::InvalidRequest,
                "a native guest activity batch must contain at least one request",
            ));
        }
        let prepared = self.prepare_generation(session, generation).await?;
        let admitted = self.admit_operation(generation, activity, prepared).await?;
        let mut replies = Vec::with_capacity(requests.len());
        for request in requests {
            let reply = match self
                .call_guest(&admitted.endpoint, verb, request, timeout)
                .await
            {
                Ok(reply) => reply,
                Err(
                    error @ HandsError::Transport {
                        proof: DispatchProof::NotSent,
                        ..
                    },
                ) if replies.is_empty() => {
                    if admitted.native_resume {
                        self.reconcile_native_resume(&admitted.view).await?;
                    }
                    self.settle_operation(generation, activity).await?;
                    return Err(error);
                }
                Err(error) => return Err(error),
            };
            replies.push(reply);
        }
        if admitted.native_resume {
            self.settle_native_activity(&admitted.view).await?;
        }
        self.settle_operation(generation, activity).await?;
        Ok((replies, admitted.native_resume))
    }

    async fn endpoint_for(
        &self,
        generation: GenerationId,
        operation: HandsOperationId,
    ) -> Result<ActivityAdmission, HandsError> {
        let first = self.load_view(generation, ReadConsistency::Strong).await?;
        let prepared = self.prepare_generation(first.session, generation).await?;
        if prepared.head.state == GenerationState::LifetimeDraining {
            let endpoint = self.connect(&prepared).await?;
            return Ok(ActivityAdmission {
                view: prepared,
                endpoint,
                native_resume: false,
            });
        }
        self.admit_operation(generation, operation, prepared).await
    }

    async fn observe_provider_for_activity(
        &self,
        view: &GenerationView,
    ) -> Result<(MicrovmDescription, EndpointToken), HandsError> {
        let microvm = view.microvm.as_ref().ok_or_else(|| {
            dispatched(
                ProviderFailureKind::ProtocolViolation,
                "a materialized generation has no provider identity",
            )
        })?;
        let (described, token) = tokio::join!(
            self.provider.get(microvm),
            self.provider
                .auth_token(microvm, TOKEN_TTL_SECONDS, &[AGENT_PORT])
        );
        let description =
            described.map_err(|call| provider_error(&call, view.head.generation, false))?;
        if description.microvm != *microvm {
            return Err(dispatched(
                ProviderFailureKind::ProtocolViolation,
                "activity observation answered for a different provider identity",
            ));
        }
        let token = token.map_err(|call| provider_error(&call, view.head.generation, false))?;
        Ok((description, token))
    }

    fn native_suspended_at(
        view: &GenerationView,
        observed_at: Timestamp,
    ) -> Result<Timestamp, HandsError> {
        let mut suspended_at = match (view.head.state, view.suspended_at, view.head.idle_since) {
            (GenerationState::Suspended, Some(suspended_at), _) => suspended_at,
            (GenerationState::Running, None, Some(idle_since)) => {
                aex_runtime_control::clock::plus_millis(idle_since, TRUE_IDLE_THRESHOLD_MS)
            }
            (GenerationState::Running, None, None) => aex_runtime_control::clock::plus_millis(
                view.head.last_busy_at,
                TRUE_IDLE_THRESHOLD_MS,
            ),
            _ => {
                return Err(dispatched(
                    ProviderFailureKind::ProtocolViolation,
                    "provider suspension has no durable idle or suspended boundary",
                ));
            }
        };
        if let Some(lifetime) = view.lifetime {
            let expires_at = lifetime.expires_at();
            if observed_at >= expires_at {
                return Err(HandsError::GenerationLost {
                    generation: view.head.generation,
                });
            }
            if suspended_at > expires_at {
                suspended_at = expires_at;
            }
        }
        if suspended_at > observed_at {
            return Err(dispatched(
                ProviderFailureKind::ProtocolViolation,
                "provider suspended before the immutable 180-second idle boundary",
            ));
        }
        Ok(suspended_at)
    }

    fn lease_from_observation(
        view: &GenerationView,
        description: MicrovmDescription,
        token: EndpointToken,
    ) -> Result<AuthenticatedGuestEndpoint, HandsError> {
        if !matches!(
            description.state,
            ProviderState::Running | ProviderState::Suspended
        ) {
            return Err(dispatched(
                ProviderFailureKind::ProtocolViolation,
                "activity endpoint is neither running nor provider-suspended",
            ));
        }
        let endpoint = description.endpoint.ok_or_else(|| {
            dispatched(
                ProviderFailureKind::ProtocolViolation,
                "the exact MicroVM has no authenticated endpoint",
            )
        })?;
        AuthenticatedGuestEndpoint::new(view.head.generation, view.head.fence, endpoint, token)
    }

    #[expect(
        clippy::too_many_lines,
        reason = "operation admission owns one retry-bounded exact-generation and native-resume fence"
    )]
    async fn admit_operation(
        &self,
        generation: GenerationId,
        operation: HandsOperationId,
        first: GenerationView,
    ) -> Result<ActivityAdmission, HandsError> {
        // The first attempt reuses the view the caller already loaded; only a
        // lost conditional write pays for a fresh strong read.
        let mut view = first;
        for attempt in 0..STORE_ATTEMPTS {
            if attempt > 0 {
                view = self.load_view(generation, ReadConsistency::Strong).await?;
            }
            if view.head.state == GenerationState::Resuming
                && view
                    .open_intent
                    .as_ref()
                    .is_some_and(|intent| intent.action == LifecycleAction::NativeResume)
            {
                self.reconcile_native_resume(&view).await?;
                continue;
            }
            if !matches!(
                view.head.state,
                GenerationState::Running | GenerationState::Suspended
            ) {
                return Err(admission_error(AdmissionRefused::NotRunning {
                    state: view.head.state,
                }));
            }
            let at = now()?;
            // A durable open-operation count may conservatively over-count after
            // a crash, so it cannot prove the provider is still running. One
            // on-demand exact-identity read avoids both a periodic scanner and an
            // unrecorded implicit resume.
            let observed = Some(self.observe_provider_for_activity(&view).await?);
            let provider_suspended = observed
                .as_ref()
                .is_some_and(|(description, _)| description.state == ProviderState::Suspended);
            if observed.as_ref().is_some_and(|(description, _)| {
                matches!(
                    description.state,
                    ProviderState::Terminated | ProviderState::Terminating
                )
            }) {
                return Err(HandsError::GenerationLost { generation });
            }
            if observed.as_ref().is_some_and(|(description, _)| {
                !matches!(
                    description.state,
                    ProviderState::Running | ProviderState::Suspended
                )
            }) {
                return Err(pre_dispatch(
                    ProviderFailureKind::Transport,
                    "the provider is between native lifecycle states; retry exact-generation admission",
                ));
            }
            let native_resume = view.head.state == GenerationState::Suspended || provider_suspended;
            let plan = if native_resume {
                let microvm = view.microvm.clone().ok_or_else(|| {
                    dispatched(
                        ProviderFailureKind::ProtocolViolation,
                        "native resume has no provider identity",
                    )
                })?;
                admit_native_resume(
                    &view.head,
                    operation,
                    microvm,
                    Self::native_suspended_at(&view, at)?,
                    at,
                )
                .map_err(admission_error)?
            } else {
                admit(
                    &view.head,
                    operation,
                    view.head.fence,
                    view.head.revision,
                    at,
                )
                .map_err(admission_error)?
            };
            match self.store.admit_operation(&plan).await {
                Ok(()) => {
                    let admitted = self.load_view(generation, ReadConsistency::Strong).await?;
                    let expected_admitted_state = if native_resume {
                        GenerationState::Resuming
                    } else {
                        GenerationState::Running
                    };
                    if admitted.head.state != expected_admitted_state {
                        continue;
                    }
                    let endpoint = match observed {
                        Some((description, token)) => {
                            let endpoint =
                                Self::lease_from_observation(&admitted, description, token)?;
                            let identity = LeaseIdentity {
                                generation,
                                fence: admitted.head.fence,
                                microvm: admitted.microvm.clone().ok_or_else(|| {
                                    dispatched(
                                        ProviderFailureKind::ProtocolViolation,
                                        "admitted activity lost its provider identity",
                                    )
                                })?,
                            };
                            let expires_at = endpoint.lease_expires_at();
                            self.leases
                                .lock()
                                .await
                                .insert(identity, expires_at, endpoint)
                        }
                        None => self.connect(&admitted).await?,
                    };
                    if native_resume && endpoint.value.generation != admitted.head.generation {
                        return Err(dispatched(
                            ProviderFailureKind::ProtocolViolation,
                            "native activity endpoint names another generation",
                        ));
                    }
                    return Ok(ActivityAdmission {
                        view: admitted,
                        endpoint,
                        native_resume,
                    });
                }
                Err(RuntimeStoreError::RevisionConflict { .. }) => {}
                Err(error) => return Err(store_error(error)),
            }
        }
        Err(pre_dispatch(
            ProviderFailureKind::Overloaded,
            "operation admission lost its bounded concurrency retry budget",
        ))
    }

    async fn settle_operation(
        &self,
        generation: GenerationId,
        operation: HandsOperationId,
    ) -> Result<(), HandsError> {
        for _ in 0..STORE_ATTEMPTS {
            let view = self.load_view(generation, ReadConsistency::Strong).await?;
            let plan = settle(&view.head, operation, now()?);
            match self.store.settle_operation(&plan).await {
                Ok(()) => return Ok(()),
                Err(RuntimeStoreError::RevisionConflict { .. }) => {}
                Err(error) => return Err(store_error(error)),
            }
        }
        Err(dispatched(
            ProviderFailureKind::Overloaded,
            "operation settlement lost its bounded concurrency retry budget",
        ))
    }
}

impl LiveFileBackend for ProductionHandsBackend {
    fn ensure_ready(
        &self,
        session: SessionId,
        generation: GenerationId,
    ) -> BoxFuture<'_, Result<LiveGenerationReady, HandsError>> {
        Box::pin(async move {
            let (view, _endpoint) = self.materialize(session, generation).await?;
            let lifetime = view.lifetime.ok_or_else(|| {
                dispatched(
                    ProviderFailureKind::ProtocolViolation,
                    "a reachable generation has no provider lifetime authority",
                )
            })?;
            Ok(LiveGenerationReady {
                generation,
                launched_at: lifetime.launched_at,
                expires_at: lifetime.expires_at(),
                observed_at: now()?,
                lifecycle_fence: view.head.fence.0,
            })
        })
    }

    fn hold_tool_waiter(
        &self,
        session: SessionId,
        generation: GenerationId,
        operation: HandsOperationId,
    ) -> BoxFuture<'_, Result<aex_hands_protocol::rpc::Fence, HandsError>> {
        Box::pin(async move {
            let prepared = self.prepare_generation(session, generation).await?;
            let admitted = self
                .admit_operation(generation, operation, prepared)
                .await?;
            let hello: HelloResponse = self
                .call_guest(
                    &admitted.endpoint,
                    Verb::Hello,
                    &HelloRequest {},
                    STATUS_TIMEOUT,
                )
                .await?;
            if hello.protocol_version != aex_hands_protocol::rpc::PROTOCOL_V1
                || hello.max_body_bytes < MAX_FRAME_BYTES as u64
            {
                return Err(dispatched(
                    ProviderFailureKind::ProtocolViolation,
                    "the exact-generation guest failed the Tool Mux hello contract",
                ));
            }
            if admitted.native_resume {
                self.settle_native_activity(&admitted.view).await?;
            }
            Ok(aex_hands_protocol::rpc::Fence(admitted.view.head.fence.0))
        })
    }

    fn settle_tool_waiter(
        &self,
        generation: GenerationId,
        operation: HandsOperationId,
    ) -> BoxFuture<'_, Result<(), HandsError>> {
        Box::pin(async move { self.settle_operation(generation, operation).await })
    }

    fn suspend_ready(
        &self,
        session: SessionId,
        generation: GenerationId,
    ) -> BoxFuture<'_, Result<(), HandsError>> {
        Box::pin(async move {
            const ATTEMPTS: usize = 16;
            for _ in 0..ATTEMPTS {
                match self
                    .runtime
                    .handle_command(
                        aex_runtime_control_aws::RuntimeCommand::SessionSuspend {
                            session,
                            generation,
                        },
                        now()?,
                    )
                    .await
                {
                    CommandOutcome::Settled(Settled::Suspended) => return Ok(()),
                    CommandOutcome::Settled(Settled::Lost | Settled::AlreadyTerminal { .. }) => {
                        return Err(HandsError::GenerationLost { generation });
                    }
                    CommandOutcome::Settled(Settled::Superseded { .. }) => {
                        return Err(dispatched(
                            ProviderFailureKind::ProtocolViolation,
                            "eager suspension found a successor generation",
                        ));
                    }
                    CommandOutcome::Settled(_) | CommandOutcome::Retry { .. } => {}
                    CommandOutcome::Poison { .. } => {
                        return Err(dispatched(
                            ProviderFailureKind::ProtocolViolation,
                            "runtime control refused eager sandbox suspension",
                        ));
                    }
                }
            }
            Err(dispatched(
                ProviderFailureKind::Overloaded,
                "eager sandbox suspension lost its bounded reconciliation budget",
            ))
        })
    }

    fn call<'a>(
        &'a self,
        session: SessionId,
        generation: GenerationId,
        activity: HandsOperationId,
        requests: &'a [FileRequest],
    ) -> BoxFuture<'a, Result<LiveFileReply, HandsError>> {
        Box::pin(async move {
            if requests.is_empty() || requests.len() > FILE_BATCH_MAX_REQUESTS {
                return Err(pre_dispatch(
                    ProviderFailureKind::InvalidRequest,
                    "a live-file batch must contain between one and ten bounded guest calls",
                ));
            }
            let (replies, resumed) = self
                .call_native_activity_batch::<_, FileResponse>(
                    session,
                    generation,
                    activity,
                    Verb::File,
                    requests,
                    FILE_RPC_TIMEOUT,
                )
                .await?;
            let view = self.load_view(generation, ReadConsistency::Strong).await?;
            Self::require_view(&view, session, generation)?;
            let lifecycle_fence = view.head.fence.0;
            let expires_at = view
                .lifetime
                .ok_or_else(|| {
                    dispatched(
                        ProviderFailureKind::ProtocolViolation,
                        "a reachable generation has no provider lifetime authority",
                    )
                })?
                .expires_at();
            Ok(LiveFileReply {
                responses: replies,
                generation,
                resumed,
                lifecycle_fence,
                expires_at,
            })
        })
    }

    fn abort_unpublished(
        &self,
        session: SessionId,
        generation: GenerationId,
    ) -> BoxFuture<'_, Result<(), HandsError>> {
        Box::pin(async move {
            const ATTEMPTS: usize = 16;
            for _ in 0..ATTEMPTS {
                let outcome = self
                    .runtime
                    .handle_command(
                        aex_runtime_control_aws::RuntimeCommand::SessionTerminate {
                            session,
                            generation,
                        },
                        now()?,
                    )
                    .await;
                match outcome {
                    CommandOutcome::Settled(
                        Settled::Terminated { .. }
                        | Settled::AlreadyTerminal { .. }
                        | Settled::Lost
                        | Settled::NotMaterialized
                        | Settled::NoGeneration,
                    ) => return Ok(()),
                    CommandOutcome::Settled(Settled::Superseded { .. }) => {
                        return Err(dispatched(
                            ProviderFailureKind::ProtocolViolation,
                            "an unpublished generation was superseded before compensation",
                        ));
                    }
                    CommandOutcome::Settled(_) | CommandOutcome::Retry { .. } => {}
                    CommandOutcome::Poison { .. } => {
                        return Err(dispatched(
                            ProviderFailureKind::ProtocolViolation,
                            "unpublished generation compensation reached poisoned authority state",
                        ));
                    }
                }
            }
            Err(dispatched(
                ProviderFailureKind::Overloaded,
                "unpublished generation compensation lost its bounded reconciliation budget",
            ))
        })
    }
}

impl core::fmt::Debug for ProductionHandsBackend {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("ProductionHandsBackend")
            .finish_non_exhaustive()
    }
}

impl crate::HandsBackend for ProductionHandsBackend {
    fn ensure_generation<'a>(
        &'a self,
        session: &'a BrainSessionId,
        generation: GenerationId,
    ) -> BoxFuture<'a, Result<HandsEndpoint, HandsError>> {
        Box::pin(async move {
            let (_, endpoint) = self
                .materialize(wire_session(*session)?, generation)
                .await?;
            Ok(HandsEndpoint {
                generation,
                address: endpoint.value.address().to_owned(),
                lease_expires_at: BrainTimestamp::from_millis(
                    endpoint.value.lease_expires_at().unix_millis(),
                ),
            })
        })
    }

    #[allow(
        clippy::too_many_lines,
        reason = "the ordering from ticket validation through durable admission and exact response classification is one security-critical dispatch boundary"
    )]
    fn start<'a>(
        &'a self,
        ticket: &'a DispatchTicket,
        generation: GenerationId,
        start: &'a HandsOperationStart,
    ) -> BoxFuture<'a, Result<HandsAccepted, HandsError>> {
        Box::pin(async move {
            let session = wire_session(ticket.key().session)?;
            // One strong view serves the whole dispatch: materialize returns
            // the view it connected under, the ownership and capability checks
            // read immutable fields, and admission seeds its first conditional
            // write from it. The retired shape loaded the same head three
            // times serially per start.
            let operation = wire_operation(&start.operation)?;
            let prepared = self.prepare_generation(session, generation).await?;
            let request: OperationRequest =
                serde_json::from_value(start.request.clone()).map_err(|_| {
                    pre_dispatch(
                        ProviderFailureKind::InvalidRequest,
                        "the Hands operation request does not match the strict protocol",
                    )
                })?;
            if prepared.workspace != ticket.workspace()
                || prepared.organization != ticket.organization()
                || prepared.session != session
            {
                return Err(pre_dispatch(
                    ProviderFailureKind::Authentication,
                    "the dispatch ticket does not own the generation",
                ));
            }
            if request.requires_browser()
                && !prepared.definition.image.carries(ImageCapability::Browser)
            {
                return Err(pre_dispatch(
                    ProviderFailureKind::InvalidRequest,
                    "the pinned image does not carry the browser capability",
                ));
            }
            if !request.browser_target_is_coherent() {
                return Err(pre_dispatch(
                    ProviderFailureKind::InvalidRequest,
                    "the browser request has an incoherent session target",
                ));
            }
            let admitted = self
                .admit_operation(generation, operation, prepared)
                .await?;
            let view = &admitted.view;
            let delivery = delivery_for(start.bounds.timeout_ms);
            let call = StartRequest {
                operation,
                call_hash: CallHash(ContentHash::from_bytes(start.call_hash.0)),
                request,
                bounds: operation_bounds(view, &start.bounds),
                deadline: wire_timestamp(start.deadline)?,
                delivery,
            };
            let timeout = Duration::from_millis(u64::from(start.bounds.timeout_ms));
            if delivery == DeliveryMode::Attached {
                return self
                    .start_attached(
                        &admitted.endpoint,
                        admitted.native_resume.then_some(view),
                        generation,
                        operation,
                        start,
                        &call,
                        timeout,
                    )
                    .await;
            }
            let reply = match self
                .call_guest(&admitted.endpoint, Verb::Start, &call, timeout)
                .await
            {
                Ok(reply) => reply,
                Err(
                    error @ HandsError::Transport {
                        proof: DispatchProof::NotSent,
                        ..
                    },
                ) => {
                    self.settle_operation(generation, operation).await?;
                    return Err(error);
                }
                Err(error) => return Err(error),
            };
            if admitted.native_resume {
                self.settle_native_activity(view).await?;
            }
            match reply {
                StartResponse::Accepted {
                    operation: found,
                    existing,
                    ..
                } => {
                    require_operation(operation, found)?;
                    Ok(HandsAccepted {
                        operation: start.operation.clone(),
                        generation,
                        created: !existing,
                        poll_after: poll_after(0),
                        result: None,
                    })
                }
                StartResponse::AlreadyTerminal {
                    operation: found, ..
                } => {
                    require_operation(operation, found)?;
                    Ok(HandsAccepted {
                        operation: start.operation.clone(),
                        generation,
                        created: false,
                        poll_after: Duration::ZERO,
                        result: None,
                    })
                }
                StartResponse::Conflict {
                    operation: found, ..
                } => {
                    require_operation(operation, found)?;
                    self.settle_operation(generation, operation).await?;
                    Err(HandsError::CallHashConflict {
                        operation: start.operation.clone(),
                    })
                }
                StartResponse::Rejected {
                    operation: found, ..
                } => {
                    require_operation(operation, found)?;
                    self.settle_operation(generation, operation).await?;
                    Err(response_error(
                        ProviderFailureKind::InvalidRequest,
                        "the guest rejected the Hands operation",
                    ))
                }
            }
        })
    }

    fn status<'a>(
        &'a self,
        generation: GenerationId,
        operation: &'a BrainOperationId,
    ) -> BoxFuture<'a, Result<HandsOperationStatus, HandsError>> {
        Box::pin(async move {
            let wire = wire_operation(operation)?;
            let admitted = self.endpoint_for(generation, wire).await?;
            let view = &admitted.view;
            let reply = self
                .call_guest::<_, StatusResponse>(
                    &admitted.endpoint,
                    Verb::Status,
                    &StatusRequest { operation: wire },
                    STATUS_TIMEOUT,
                )
                .await?;
            if admitted.native_resume {
                self.settle_native_activity(view).await?;
            }
            let unknown = matches!(&reply, StatusResponse::Unknown { .. });
            let status = status(reply, wire, now()?)?;
            if unknown {
                self.settle_operation(generation, wire).await?;
            }
            Ok(status)
        })
    }

    fn cancel<'a>(
        &'a self,
        generation: GenerationId,
        operation: &'a BrainOperationId,
        _fence: BrainFence,
    ) -> BoxFuture<'a, Result<(), HandsError>> {
        Box::pin(async move {
            let wire = wire_operation(operation)?;
            let admitted = self.endpoint_for(generation, wire).await?;
            let view = &admitted.view;
            let reply = self
                .call_guest::<_, CancelResponse>(
                    &admitted.endpoint,
                    Verb::Cancel,
                    &CancelRequest {
                        operation: wire,
                        reason: CancelReason::CustomerStop,
                    },
                    STATUS_TIMEOUT,
                )
                .await?;
            if admitted.native_resume {
                self.settle_native_activity(view).await?;
            }
            let terminal = matches!(&reply, CancelResponse::AlreadyTerminal { .. });
            let unknown = matches!(&reply, CancelResponse::Unknown { .. });
            let outcome = match reply {
                CancelResponse::Cancelling { operation: found }
                | CancelResponse::AlreadyTerminal {
                    operation: found, ..
                }
                | CancelResponse::Unknown { operation: found } => require_operation(wire, found),
            };
            outcome?;
            if unknown || terminal {
                self.settle_operation(generation, wire).await?;
            }
            Ok(())
        })
    }

    #[allow(
        clippy::too_many_lines,
        reason = "the bounded resumable pull, terminal metadata pin, digest verification, and idempotent settlement are one incorporation boundary"
    )]
    fn result<'a>(
        &'a self,
        generation: GenerationId,
        operation: &'a BrainOperationId,
        bounds: &'a ResultBounds,
    ) -> BoxFuture<'a, Result<HandsResult, HandsError>> {
        Box::pin(async move {
            let wire = wire_operation(operation)?;
            let admitted = self.endpoint_for(generation, wire).await?;
            let view = &admitted.view;
            let mut native_resume = admitted.native_resume;
            let maximum = u64::try_from(bounds.max_bytes)
                .unwrap_or(u64::MAX)
                .min(MAX_RESULT_BODY_BYTES);
            let mut assembly: Option<ResultAssembly> = None;
            let mut terminal: Option<TerminalMetadata> = None;
            for _ in 0..RESULT_PULL_ATTEMPTS {
                let offset = assembly.as_ref().map_or(0, ResultAssembly::next_offset);
                let remaining = maximum.saturating_sub(offset);
                let reply = self
                    .call_guest::<_, ResultResponse>(
                        &admitted.endpoint,
                        Verb::Result,
                        &ResultRequest {
                            operation: wire,
                            from_offset: offset,
                            max_bytes: remaining.min(RESULT_CHUNK_BYTES),
                        },
                        Duration::from_millis(u64::from(bounds.timeout_ms)),
                    )
                    .await?;
                if native_resume {
                    self.settle_native_activity(view).await?;
                    native_resume = false;
                }
                match reply {
                    ResultResponse::Terminal {
                        terminal: found_terminal,
                        chunk,
                    } => {
                        if found_terminal.body_len > maximum {
                            return Err(result_rejected(
                                operation,
                                "the terminal body exceeds the caller's result ceiling",
                            ));
                        }
                        if let Some(expected) = &terminal {
                            if expected != &found_terminal {
                                return Err(result_rejected(
                                    operation,
                                    "terminal metadata changed between result chunks",
                                ));
                            }
                        } else {
                            terminal = Some(found_terminal.clone());
                            assembly = Some(ResultAssembly::resume(
                                wire,
                                found_terminal.clone(),
                                Vec::new(),
                                maximum,
                            ));
                        }
                        if let Some(chunk) = chunk {
                            require_operation(wire, chunk.operation)?;
                            let expected = found_terminal
                                .body_len
                                .saturating_sub(offset)
                                .min(remaining.min(RESULT_CHUNK_BYTES));
                            if u64::try_from(chunk.bytes.len()).unwrap_or(u64::MAX) != expected
                                || chunk.last
                                    != (offset.saturating_add(expected) == found_terminal.body_len)
                            {
                                return Err(result_rejected(
                                    operation,
                                    "the guest returned a short or inconsistently final result chunk",
                                ));
                            }
                            assembly
                                .as_mut()
                                .expect("terminal initializes the assembly")
                                .incorporate(&chunk)
                                .map_err(|_| {
                                    result_rejected(
                                        operation,
                                        "a result chunk was out of order or exceeded its declaration",
                                    )
                                })?;
                        } else if !assembly.as_ref().is_some_and(ResultAssembly::is_complete) {
                            return Err(result_rejected(
                                operation,
                                "the guest omitted a result chunk before the declared end",
                            ));
                        }
                        if assembly.as_ref().is_some_and(ResultAssembly::is_complete) {
                            break;
                        }
                        if remaining == 0 {
                            return Err(result_rejected(
                                operation,
                                "the result assembly exhausted its byte ceiling",
                            ));
                        }
                    }
                    ResultResponse::NotTerminal {
                        operation: found, ..
                    }
                    | ResultResponse::Unknown { operation: found } => {
                        require_operation(wire, found)?;
                        return Err(response_error(
                            ProviderFailureKind::InvalidRequest,
                            "the requested Hands result is not terminal",
                        ));
                    }
                }
            }
            if !assembly.as_ref().is_some_and(ResultAssembly::is_complete) {
                return Err(result_rejected(
                    operation,
                    "the result pull exhausted its bounded request budget",
                ));
            }
            let terminal = terminal.expect("a complete assembly has terminal metadata");
            let bytes = assembly
                .expect("a complete result has an assembly")
                .finish()
                .map_err(|_| {
                    result_rejected(
                        operation,
                        "the terminal result failed length or digest verification",
                    )
                })?;
            self.settle_operation(generation, wire).await?;
            if matches!(
                terminal.state,
                TerminalState::Cancelled | TerminalState::Interrupted
            ) {
                return Err(result_rejected(
                    operation,
                    "a cancelled or interrupted operation has no incorporable result",
                ));
            }
            let (inline, sandbox_file) = result_body(operation, &terminal, bytes)?;
            let duration_ms = terminal
                .ended_at
                .unix_millis()
                .saturating_sub(terminal.started_at.unix_millis());
            Ok(HandsResult {
                operation: operation.clone(),
                generation,
                exit_code: exit_code(&terminal.exit),
                inline,
                sandbox_file,
                truncated: terminal.truncated,
                duration_ms: u32::try_from(duration_ms.max(0)).unwrap_or(u32::MAX),
                checksum: BrainContentHash(*terminal.digest.as_bytes()),
            })
        })
    }
}

/// Selects either the complete inline body or a bounded preview plus the
/// guest-retained full-result path. Length and digest have already been
/// verified by [`ResultAssembly`] before this boundary.
fn result_body(
    operation: &BrainOperationId,
    terminal: &TerminalMetadata,
    bytes: Vec<u8>,
) -> Result<(Option<String>, Option<HandsSandboxFile>), HandsError> {
    if let Some(path) = &terminal.result_file {
        let preview_len = bytes.len().min(LIVE_RESULT_PREVIEW_BYTES);
        return Ok((
            None,
            Some(HandsSandboxFile {
                path: path.as_str().to_owned(),
                byte_len: terminal.body_len,
                preview: String::from_utf8_lossy(&bytes[..preview_len]).into_owned(),
            }),
        ));
    }
    let inline = String::from_utf8(bytes)
        .map_err(|_| result_rejected(operation, "the inline terminal result is not UTF-8"))?;
    Ok((Some(inline), None))
}

fn launch_request(definition: &HandsGeneration) -> Result<RunRequest, HandsError> {
    let capabilities = definition
        .image
        .capabilities
        .iter()
        .map(|capability| match capability {
            ImageCapability::Browser => "browser".to_owned(),
        })
        .collect();
    let payload = RunHookPayload {
        v: 1,
        generation: definition.generation,
        protocol_version: definition.protocol_version.0,
        root: definition.root.0.clone(),
        size: definition.size,
        image_digest: definition.image.artifact_digest,
        capabilities,
        bounds: RunHookBounds {
            max_output_bytes: MAX_RESULT_BODY_BYTES,
            max_frame_bytes: MAX_FRAME_BYTES,
            max_wall_ms: MAX_DURATION_SECONDS.saturating_mul(1_000),
            max_concurrent_operations: u16::try_from(definition.size.max_concurrent_operations())
                .unwrap_or(u16::MAX),
        },
    };
    RunRequest::build(
        definition.image.identifier.clone(),
        definition.image.version.clone(),
        definition.network,
        &payload,
        client_token(definition.generation),
    )
    .map_err(|_| {
        pre_dispatch(
            ProviderFailureKind::InvalidRequest,
            "the immutable generation definition cannot form a launch request",
        )
    })
}

fn operation_bounds(view: &GenerationView, bounds: &ResultBounds) -> OperationBounds {
    OperationBounds {
        max_output_bytes: u64::try_from(bounds.max_bytes)
            .unwrap_or(u64::MAX)
            .min(MAX_RESULT_BODY_BYTES),
        max_frame_bytes: MAX_FRAME_BYTES,
        max_wall_ms: u64::from(bounds.timeout_ms),
        max_concurrent_operations: u16::try_from(view.head.size.max_concurrent_operations())
            .unwrap_or(u16::MAX),
    }
}

fn wire_session(session: BrainSessionId) -> Result<SessionId, HandsError> {
    Uuid7::from_bytes(*session.0.as_bytes())
        .map(SessionId::from_uuid7)
        .map_err(|_| {
            pre_dispatch(
                ProviderFailureKind::InvalidRequest,
                "the Brain session identity is not a UUIDv7",
            )
        })
}

fn wire_operation(operation: &BrainOperationId) -> Result<HandsOperationId, HandsError> {
    Uuid7::decode_suffix(operation.0.as_bytes())
        .map(HandsOperationId)
        .map_err(|_| {
            pre_dispatch(
                ProviderFailureKind::InvalidRequest,
                "the Hands operation identity is not a canonical UUIDv7 suffix",
            )
        })
}

fn wire_timestamp(timestamp: BrainTimestamp) -> Result<Timestamp, HandsError> {
    Timestamp::from_unix_millis(timestamp.millis()).map_err(|_| {
        pre_dispatch(
            ProviderFailureKind::InvalidRequest,
            "the Hands deadline is outside the wire timestamp range",
        )
    })
}

fn now() -> Result<Timestamp, HandsError> {
    let elapsed = SystemTime::now().duration_since(UNIX_EPOCH).map_err(|_| {
        pre_dispatch(
            ProviderFailureKind::ServerError,
            "the host clock is before the Unix epoch",
        )
    })?;
    let millis = i64::try_from(elapsed.as_millis()).map_err(|_| {
        pre_dispatch(
            ProviderFailureKind::ServerError,
            "the host clock is outside the representable timestamp range",
        )
    })?;
    Timestamp::from_unix_millis(millis).map_err(|_| {
        pre_dispatch(
            ProviderFailureKind::ServerError,
            "the host clock is outside the wire timestamp range",
        )
    })
}

fn status(
    response: StatusResponse,
    expected: HandsOperationId,
    now: Timestamp,
) -> Result<HandsOperationStatus, HandsError> {
    match response {
        StatusResponse::Unknown { operation } => {
            require_operation(expected, operation)?;
            Ok(HandsOperationStatus::Failed {
                reason: RedactedDetail::internal(
                    ProviderFailureKind::InvalidRequest,
                    "the guest does not know the Hands operation",
                ),
            })
        }
        StatusResponse::Accepted { operation, .. } => {
            require_operation(expected, operation)?;
            Ok(HandsOperationStatus::Running {
                poll_after: poll_after(0),
            })
        }
        StatusResponse::Running {
            operation,
            started_at,
            ..
        } => {
            require_operation(expected, operation)?;
            let age_ms = u64::try_from(now.unix_millis().saturating_sub(started_at.unix_millis()))
                .unwrap_or(0);
            Ok(HandsOperationStatus::Running {
                poll_after: poll_after(age_ms),
            })
        }
        StatusResponse::Terminal {
            operation,
            terminal,
            ..
        } => {
            require_operation(expected, operation)?;
            Ok(match terminal.state {
                TerminalState::Succeeded => HandsOperationStatus::Completed {
                    exit_code: exit_code(&terminal.exit),
                },
                TerminalState::Cancelled => HandsOperationStatus::Cancelled,
                TerminalState::Failed | TerminalState::Interrupted => {
                    HandsOperationStatus::Failed {
                        reason: RedactedDetail::internal(
                            ProviderFailureKind::ServerError,
                            "the Hands operation ended without a successful result",
                        ),
                    }
                }
            })
        }
    }
}

fn exit_code(exit: &OperationExit) -> i32 {
    match exit {
        OperationExit::Ok => 0,
        OperationExit::NonZero { code } => *code,
        OperationExit::Signal { .. } | OperationExit::Timeout => 1,
    }
}

fn require_operation(
    expected: HandsOperationId,
    found: HandsOperationId,
) -> Result<(), HandsError> {
    if expected == found {
        Ok(())
    } else {
        Err(response_error(
            ProviderFailureKind::ProtocolViolation,
            "the guest response names a different operation",
        ))
    }
}

fn admission_error(error: AdmissionRefused) -> HandsError {
    let kind = match error {
        AdmissionRefused::ConcurrencyExhausted { .. } => ProviderFailureKind::Overloaded,
        AdmissionRefused::Fenced { .. } | AdmissionRefused::StaleRevision { .. } => {
            ProviderFailureKind::Transport
        }
        AdmissionRefused::NotRunning { .. }
        | AdmissionRefused::Terminal { .. }
        | AdmissionRefused::SuspendLocked { .. } => ProviderFailureKind::InvalidRequest,
    };
    pre_dispatch(kind, "the generation refused operation admission")
}

fn provider_error(call: &ProviderCall, generation: GenerationId, effect: bool) -> HandsError {
    if matches!(call, ProviderCall::NotFound) {
        return HandsError::GenerationLost { generation };
    }
    let kind = match call {
        ProviderCall::Throttled { .. } => ProviderFailureKind::RateLimited,
        ProviderCall::Capacity { .. } => ProviderFailureKind::InsufficientProviderResource,
        ProviderCall::Transient { .. } | ProviderCall::Fatal { .. } => {
            ProviderFailureKind::ServerError
        }
        ProviderCall::Invalid { .. } => ProviderFailureKind::InvalidRequest,
        ProviderCall::Unknown { .. } => ProviderFailureKind::Transport,
        ProviderCall::NotFound => unreachable!("handled above"),
    };
    transport(
        if effect {
            DispatchStage::Dispatched
        } else {
            DispatchStage::PreDispatch
        },
        if effect {
            DispatchProof::PossiblySent
        } else {
            DispatchProof::NotSent
        },
        kind,
        "the MicroVM provider call did not produce a usable response",
    )
}

fn store_error(_error: RuntimeStoreError) -> HandsError {
    pre_dispatch(
        ProviderFailureKind::ServerError,
        "the runtime-activity authority could not serve the request",
    )
}

fn possibly_sent_store(_error: RuntimeStoreError) -> HandsError {
    dispatched(
        ProviderFailureKind::ServerError,
        "provider evidence could not be committed to runtime activity",
    )
}

fn result_rejected(operation: &BrainOperationId, message: &str) -> HandsError {
    HandsError::ResultRejected {
        operation: operation.clone(),
        reason: RedactedDetail::internal(ProviderFailureKind::ProtocolViolation, message),
    }
}

fn pre_dispatch(kind: ProviderFailureKind, message: &str) -> HandsError {
    transport(
        DispatchStage::PreDispatch,
        DispatchProof::NotSent,
        kind,
        message,
    )
}

fn dispatched(kind: ProviderFailureKind, message: &str) -> HandsError {
    transport(
        DispatchStage::Dispatched,
        DispatchProof::PossiblySent,
        kind,
        message,
    )
}

fn response_error(kind: ProviderFailureKind, message: &str) -> HandsError {
    transport(
        DispatchStage::Terminal,
        DispatchProof::ResponseStarted,
        kind,
        message,
    )
}

fn should_invalidate_lease(error: &HandsError) -> bool {
    matches!(
        error,
        HandsError::Transport { proof, detail, .. }
            if !matches!(proof, DispatchProof::NotSent)
                || detail.kind == ProviderFailureKind::Authentication
    )
}

fn transport(
    stage: DispatchStage,
    proof: DispatchProof,
    kind: ProviderFailureKind,
    message: &str,
) -> HandsError {
    HandsError::Transport {
        stage,
        proof,
        detail: RedactedDetail::internal(kind, message),
    }
}

#[cfg(test)]
mod tests {
    use super::{
        launch_request, result_body, should_invalidate_lease, wire_operation, wire_session,
    };
    use aex_brain_app::ports::{HandsError, ProviderFailureKind, RedactedDetail};
    use aex_brain_domain::effect::{DispatchProof, DispatchStage};
    use aex_brain_domain::ids::{
        HandsOperationId as BrainOperationId, SessionId as BrainSessionId,
    };
    use aex_hands_protocol::operation::{
        GuestPath, GuestRoot, OperationExit, TerminalMetadata, TerminalState,
    };
    use aex_runtime_control::generation::{
        HandsGeneration, ImageCapability, ImageIdentifier, ImagePin, ImageVersion, LimitsRevision,
        NetworkPolicy,
    };
    use aex_wire::ids::{
        ContentHash, GenerationId, OrganizationId, PrefixedId as _, SessionId, Uuid7, WorkspaceId,
    };
    use aex_wire::types::{ComputeSize, Timestamp};

    fn generation() -> HandsGeneration {
        let generation = GenerationId::from_uuid7(Uuid7::compose(4, [4; 10]));
        HandsGeneration {
            generation,
            session: SessionId::from_uuid7(Uuid7::compose(1, [1; 10])),
            workspace: WorkspaceId::from_uuid7(Uuid7::compose(2, [2; 10])),
            organization: OrganizationId::from_uuid7(Uuid7::compose(3, [3; 10])),
            size: ComputeSize::Gb2,
            image: ImagePin {
                identifier: ImageIdentifier("pinned-image".to_owned()),
                version: ImageVersion("17".to_owned()),
                artifact_digest: ContentHash::from_bytes([7; 32]),
                capabilities: vec![ImageCapability::Browser],
            },
            network: NetworkPolicy::None,
            protocol_version: aex_internal_contracts::SchemaVersion(1),
            limits_revision: LimitsRevision(9),
            root: GuestRoot::workspace(),
        }
    }

    #[test]
    fn the_poll_hint_grows_with_operation_age_to_a_bounded_ceiling() {
        assert_eq!(
            super::poll_after(0),
            core::time::Duration::from_millis(250),
            "a fresh operation polls tightly"
        );
        assert_eq!(
            super::poll_after(8_000),
            core::time::Duration::from_millis(1_250)
        );
        assert_eq!(
            super::poll_after(30_000),
            core::time::Duration::from_secs(4)
        );
        assert_eq!(
            super::poll_after(u64::MAX),
            core::time::Duration::from_secs(4),
            "the ceiling holds for any age"
        );
    }

    #[test]
    fn launch_reconstruction_uses_only_the_immutable_generation_definition() {
        let definition = generation();
        let request = launch_request(&definition).expect("request");
        assert_eq!(request.image_identifier, definition.image.identifier);
        assert_eq!(request.image_version, definition.image.version);
        assert!(request.egress_network_connectors.is_empty());
        assert_eq!(
            request.client_token,
            format!("aexgen-{}", definition.generation)
        );
        assert!(request.run_hook_payload.contains("browser"));
        assert!(!request.run_hook_payload.contains("pinned-image"));
    }

    #[test]
    fn brain_identity_translation_is_strictly_uuid_v7() {
        let session = Uuid7::compose(1, [1; 10]);
        let brain = BrainSessionId(uuid::Uuid::from_bytes(*session.as_bytes()));
        assert_eq!(wire_session(brain).expect("session").uuid7(), session);
        assert!(wire_session(BrainSessionId(uuid::Uuid::nil())).is_err());

        let operation = Uuid7::compose(2, [2; 10]);
        assert_eq!(
            wire_operation(&BrainOperationId(operation.to_string()))
                .expect("operation")
                .0,
            operation
        );
        assert!(wire_operation(&BrainOperationId("operation-1".to_owned())).is_err());
    }

    #[test]
    fn verified_large_result_selects_a_bounded_preview_and_exact_guest_path() {
        let bytes = vec![b'x'; 65_537];
        let terminal = TerminalMetadata {
            state: TerminalState::Succeeded,
            exit: OperationExit::Ok,
            started_at: Timestamp::from_unix_millis(0).expect("timestamp"),
            ended_at: Timestamp::from_unix_millis(1).expect("timestamp"),
            body_len: bytes.len() as u64,
            digest: ContentHash::of(&bytes),
            truncated: false,
            result_file: Some(
                GuestPath::parse(
                    &GuestRoot::workspace(),
                    "/workspace/.aex/tool-results/operation.out",
                )
                .expect("workspace path"),
            ),
            failure: None,
        };
        let (inline, file) =
            result_body(&BrainOperationId("operation".to_owned()), &terminal, bytes)
                .expect("verified result is selected");
        assert!(inline.is_none());
        let file = file.expect("large result file");
        assert_eq!(file.path, "/workspace/.aex/tool-results/operation.out");
        assert_eq!(file.byte_len, 65_537);
        assert_eq!(file.preview.len(), 65_536);
    }

    #[test]
    fn ambiguous_or_authentication_guest_failures_evict_only_the_current_lease() {
        let ambiguous = HandsError::Transport {
            stage: DispatchStage::Dispatched,
            proof: DispatchProof::PossiblySent,
            detail: RedactedDetail::internal(
                ProviderFailureKind::Transport,
                "bounded test failure",
            ),
        };
        assert!(should_invalidate_lease(&ambiguous));

        let rejected_token = HandsError::Transport {
            stage: DispatchStage::PreDispatch,
            proof: DispatchProof::NotSent,
            detail: RedactedDetail::internal(
                ProviderFailureKind::Authentication,
                "bounded test failure",
            ),
        };
        assert!(should_invalidate_lease(&rejected_token));

        let invalid_request = HandsError::Transport {
            stage: DispatchStage::PreDispatch,
            proof: DispatchProof::NotSent,
            detail: RedactedDetail::internal(
                ProviderFailureKind::InvalidRequest,
                "bounded test failure",
            ),
        };
        assert!(!should_invalidate_lease(&invalid_request));
    }
}
