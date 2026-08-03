//! Concrete Brain-to-Hands composition.
//!
//! Durable generation state remains in `runtime-activity`; the only process-local
//! state here is a weak per-generation single-flight lock. Provider idempotency on
//! `aexgen-{generation}` is the final launch-collapse layer, so losing this map on
//! restart can add latency but cannot create a second `MicroVM`.

use core::time::Duration;
use std::collections::HashMap;
use std::sync::{Arc, Weak};
use std::time::{SystemTime, UNIX_EPOCH};

use aex_brain_application::ports::{
    BoxFuture, DispatchTicket, HandsAccepted, HandsEndpoint, HandsError, HandsOperationStart,
    HandsOperationStatus, HandsResult, ProviderFailureKind, RedactedDetail, ResultBounds,
};
use aex_brain_domain::effect::{DispatchProof, DispatchStage};
use aex_brain_domain::ids::{
    ContentHash as BrainContentHash, Fence as BrainFence, HandsOperationId as BrainOperationId,
    SessionId as BrainSessionId, Timestamp as BrainTimestamp,
};
use aex_hands_agent::wire::Verb;
use aex_hands_control_aws::{
    AGENT_PORT, MAX_DURATION_SECONDS, MicrovmControlApi, MicrovmDescription, RunHookBounds,
    RunHookPayload, RunRequest, TOKEN_TTL_SECONDS,
};
use aex_hands_protocol::lifecycle::ProviderRequestId;
use aex_hands_protocol::operation::{
    DeliveryMode, OperationBounds, OperationExit, OperationRequest, TerminalMetadata, TerminalState,
};
use aex_hands_protocol::rpc::{
    CallHash, CancelReason, CancelRequest, CancelResponse, GenerationBinding, HandsOperationId,
    ResultRequest, ResultResponse, StartRequest, StartResponse, StatusRequest, StatusResponse,
};
use aex_internal_contracts::SchemaVersion;
use aex_runtime_control::generation::{
    AdmissionRefused, GenerationState, HandsGeneration, ImageCapability, next_fence,
};
use aex_runtime_control::lifecycle::{
    IntentState, LifecycleAction, LifecycleIntentId, MicrovmId, ProviderCall, ProviderState,
    client_token,
};
use aex_runtime_control::shape::ShapeCapacity as _;
use aex_runtime_control::store::{
    GenerationAccountingPlan, GenerationPlan, GenerationView, LifecycleIntentPlan,
    LifecycleReceiptPlan, LifecycleRequestPlan, RuntimeActivityStore, RuntimeStoreError,
};
use aex_runtime_control_aws::{
    AWAIT_BUDGET_MS, CommandOutcome, RuntimeCommand, RuntimeControl, Settled, intent_id,
};
use aex_wire::ids::{ContentHash, GenerationId, PrefixedId as _, SessionId, Uuid7};
use aex_wire::types::Timestamp;
use tokio::sync::Mutex;

use crate::{
    AuthenticatedGuestEndpoint, HttpGuestTransport, MAX_FRAME_BYTES, MAX_RESULT_BODY_BYTES,
    ResultAssembly, admit, launch_backoff_ms, settle,
};

const MATERIALIZE_ATTEMPTS: u32 = 64;
const STORE_ATTEMPTS: usize = 16;
const STATUS_TIMEOUT: Duration = Duration::from_secs(5);
const RESULT_CHUNK_BYTES: u64 = 180_000;
const RESULT_PULL_ATTEMPTS: usize = 32;

/// Production implementation of Brain's lower Hands backend.
pub struct ProductionHandsBackend {
    store: Arc<dyn RuntimeActivityStore>,
    provider: Arc<dyn MicrovmControlApi>,
    runtime: Arc<RuntimeControl>,
    guest: HttpGuestTransport,
    flights: Mutex<HashMap<GenerationId, Weak<Mutex<()>>>>,
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
        Ok(Self {
            store,
            provider,
            runtime,
            guest: HttpGuestTransport::new()?,
            flights: Mutex::new(HashMap::new()),
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

    async fn load_view(&self, generation: GenerationId) -> Result<GenerationView, HandsError> {
        self.store
            .load_generation_view(generation)
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

    async fn materialize(
        &self,
        session: SessionId,
        generation: GenerationId,
    ) -> Result<AuthenticatedGuestEndpoint, HandsError> {
        let flight = self.flight(generation).await;
        let _guard = flight.lock().await;
        for attempt in 0..MATERIALIZE_ATTEMPTS {
            let view = self.load_view(generation).await?;
            Self::require_view(&view, session, generation)?;
            match view.head.state {
                GenerationState::Running | GenerationState::LifetimeDraining => {
                    return self.connect(&view).await;
                }
                GenerationState::Requested => {
                    let _ = self.launch(&view).await?;
                }
                GenerationState::Launching | GenerationState::Unknown
                    if view
                        .open_intent
                        .as_ref()
                        .is_some_and(|intent| intent.action == LifecycleAction::Launch) =>
                {
                    self.recover_launch(&view).await?;
                }
                GenerationState::Launching
                | GenerationState::Resuming
                | GenerationState::Suspending => {
                    tokio::time::sleep(Duration::from_millis(launch_backoff_ms(attempt))).await;
                }
                GenerationState::Suspended => {
                    self.resume(&view).await?;
                }
                GenerationState::Terminating
                | GenerationState::Terminated
                | GenerationState::Lost => {
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

    async fn launch(&self, view: &GenerationView) -> Result<bool, HandsError> {
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
            ) => return Ok(true),
            Err(error) => return Err(store_error(error)),
        };
        match self.provider.run(&request).await {
            Ok(description) => {
                self.finish_launch(view, &commit.generation.head, &intent_id, description)
                    .await?;
                Ok(true)
            }
            Err(call) => {
                self.close_failed_launch(view, &commit.generation.head, &intent_id, &call)
                    .await?;
                Err(provider_error(&call, view.head.generation, true))
            }
        }
    }

    async fn recover_launch(&self, view: &GenerationView) -> Result<(), HandsError> {
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
            self.settle_launch(
                view,
                &intent.intent_id,
                microvm,
                request,
                description.launched_at,
            )
            .await?;
            return Ok(());
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
    ) -> Result<(), HandsError> {
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
        self.settle_launch(
            &recovered,
            intent_id,
            &running.microvm,
            &request,
            running.launched_at,
        )
        .await
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

    async fn resume(&self, view: &GenerationView) -> Result<(), HandsError> {
        match self
            .runtime
            .handle_command(
                RuntimeCommand::LiveWorkspaceWake {
                    session: view.session,
                    generation: view.head.generation,
                },
                now()?,
            )
            .await
        {
            CommandOutcome::Settled(Settled::Resumed | Settled::Raced | Settled::Held { .. }) => {
                Ok(())
            }
            CommandOutcome::Settled(
                Settled::Lost
                | Settled::NoGeneration
                | Settled::Superseded { .. }
                | Settled::AlreadyTerminal { .. }
                | Settled::ResumeRefused { .. },
            ) => Err(HandsError::GenerationLost {
                generation: view.head.generation,
            }),
            CommandOutcome::Settled(Settled::Reconciling) => Err(dispatched(
                ProviderFailureKind::Transport,
                "the resume outcome is under durable reconciliation",
            )),
            CommandOutcome::Retry { .. } => Err(pre_dispatch(
                ProviderFailureKind::ServerError,
                "runtime control could not resume the generation",
            )),
            CommandOutcome::Poison { .. } => Err(pre_dispatch(
                ProviderFailureKind::ProtocolViolation,
                "runtime control rejected the generation state",
            )),
            CommandOutcome::Settled(_) => Ok(()),
        }
    }

    async fn connect(
        &self,
        view: &GenerationView,
    ) -> Result<AuthenticatedGuestEndpoint, HandsError> {
        let microvm = view.microvm.as_ref().ok_or_else(|| {
            dispatched(
                ProviderFailureKind::ProtocolViolation,
                "a running generation has no provider identity",
            )
        })?;
        let description = self
            .provider
            .get(microvm)
            .await
            .map_err(|call| provider_error(&call, view.head.generation, false))?;
        if description.microvm != *microvm || description.state != ProviderState::Running {
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
        let token = self
            .provider
            .auth_token(microvm, TOKEN_TTL_SECONDS, &[AGENT_PORT])
            .await
            .map_err(|call| provider_error(&call, view.head.generation, false))?;
        AuthenticatedGuestEndpoint::new(view.head.generation, view.head.fence, endpoint, token)
    }

    async fn endpoint_for(
        &self,
        generation: GenerationId,
    ) -> Result<(GenerationView, AuthenticatedGuestEndpoint), HandsError> {
        let view = self.load_view(generation).await?;
        if !matches!(
            view.head.state,
            GenerationState::Running | GenerationState::LifetimeDraining
        ) {
            return Err(HandsError::GenerationLost { generation });
        }
        let endpoint = self.connect(&view).await?;
        Ok((view, endpoint))
    }

    async fn admit_operation(
        &self,
        generation: GenerationId,
        operation: HandsOperationId,
    ) -> Result<GenerationView, HandsError> {
        for _ in 0..STORE_ATTEMPTS {
            let view = self.load_view(generation).await?;
            let plan = admit(
                &view.head,
                operation,
                view.head.fence,
                view.head.revision,
                now()?,
            )
            .map_err(admission_error)?;
            match self.store.admit_operation(&plan).await {
                Ok(()) => return Ok(view),
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
            let view = self.load_view(generation).await?;
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
            let endpoint = self
                .materialize(wire_session(*session)?, generation)
                .await?;
            Ok(HandsEndpoint {
                generation,
                address: endpoint.address().to_owned(),
                lease_expires_at: BrainTimestamp::from_millis(
                    endpoint.lease_expires_at().unix_millis(),
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
            let _ = self.materialize(session, generation).await?;
            let operation = wire_operation(&start.operation)?;
            let request: OperationRequest =
                serde_json::from_value(start.request.clone()).map_err(|_| {
                    pre_dispatch(
                        ProviderFailureKind::InvalidRequest,
                        "the Hands operation request does not match the strict protocol",
                    )
                })?;
            let view = self.load_view(generation).await?;
            if view.workspace != ticket.workspace()
                || view.organization != ticket.organization()
                || view.session != session
            {
                return Err(pre_dispatch(
                    ProviderFailureKind::Authentication,
                    "the dispatch ticket does not own the generation",
                ));
            }
            if request.requires_browser()
                && !view.definition.image.carries(ImageCapability::Browser)
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
            let admitted = self.admit_operation(generation, operation).await?;
            let endpoint = match self.connect(&admitted).await {
                Ok(endpoint) => endpoint,
                Err(error) => {
                    self.settle_operation(generation, operation).await?;
                    return Err(error);
                }
            };
            let call = StartRequest {
                binding: binding(&admitted),
                operation,
                call_hash: CallHash(ContentHash::from_bytes(start.call_hash.0)),
                request,
                bounds: operation_bounds(&admitted, &start.bounds),
                deadline: wire_timestamp(start.deadline)?,
                delivery: DeliveryMode::Detached,
            };
            let timeout = Duration::from_millis(u64::from(start.bounds.timeout_ms));
            let reply = match self
                .guest
                .call(&endpoint, Verb::Start, &call, timeout)
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
            match reply.payload {
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
                        poll_after: Duration::from_millis(250),
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
            let (view, endpoint) = self.endpoint_for(generation).await?;
            let reply = self
                .guest
                .call::<_, StatusResponse>(
                    &endpoint,
                    Verb::Status,
                    &StatusRequest {
                        binding: binding(&view),
                        operation: wire,
                    },
                    STATUS_TIMEOUT,
                )
                .await?;
            let unknown = matches!(&reply.payload, StatusResponse::Unknown { .. });
            let status = status(reply.payload, wire)?;
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
            let (view, endpoint) = self.endpoint_for(generation).await?;
            let reply = self
                .guest
                .call::<_, CancelResponse>(
                    &endpoint,
                    Verb::Cancel,
                    &CancelRequest {
                        binding: binding(&view),
                        operation: wire,
                        reason: CancelReason::CustomerStop,
                    },
                    STATUS_TIMEOUT,
                )
                .await?;
            let unknown = matches!(&reply.payload, CancelResponse::Unknown { .. });
            let outcome = match reply.payload {
                CancelResponse::Cancelling { operation: found }
                | CancelResponse::AlreadyTerminal {
                    operation: found, ..
                }
                | CancelResponse::Unknown { operation: found } => require_operation(wire, found),
            };
            outcome?;
            if unknown {
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
            let (view, endpoint) = self.endpoint_for(generation).await?;
            let maximum = u64::try_from(bounds.max_bytes)
                .unwrap_or(u64::MAX)
                .min(MAX_RESULT_BODY_BYTES);
            let mut assembly: Option<ResultAssembly> = None;
            let mut terminal: Option<TerminalMetadata> = None;
            for _ in 0..RESULT_PULL_ATTEMPTS {
                let offset = assembly.as_ref().map_or(0, ResultAssembly::next_offset);
                let remaining = maximum.saturating_sub(offset);
                let reply = self
                    .guest
                    .call::<_, ResultResponse>(
                        &endpoint,
                        Verb::Result,
                        &ResultRequest {
                            binding: binding(&view),
                            operation: wire,
                            from_offset: offset,
                            max_bytes: remaining.min(RESULT_CHUNK_BYTES),
                        },
                        Duration::from_millis(u64::from(bounds.timeout_ms)),
                    )
                    .await?;
                match reply.payload {
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
            let inline = String::from_utf8(bytes).map_err(|_| {
                result_rejected(operation, "the inline terminal result is not UTF-8")
            })?;
            Ok(HandsResult {
                operation: operation.clone(),
                generation,
                exit_code: exit_code(&terminal.exit),
                inline: Some(inline),
                placed: None,
                truncated: terminal.truncated,
                checksum: BrainContentHash(*terminal.digest.as_bytes()),
            })
        })
    }
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

fn binding(view: &GenerationView) -> GenerationBinding {
    GenerationBinding {
        schema_version: SchemaVersion(view.definition.protocol_version.0),
        generation: view.head.generation,
        fence: view.head.fence,
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
        StatusResponse::Accepted { operation, .. } | StatusResponse::Running { operation, .. } => {
            require_operation(expected, operation)?;
            Ok(HandsOperationStatus::Running {
                poll_after: Duration::from_millis(250),
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
    use super::{launch_request, wire_operation, wire_session};
    use aex_brain_domain::ids::{
        HandsOperationId as BrainOperationId, SessionId as BrainSessionId,
    };
    use aex_hands_protocol::operation::GuestRoot;
    use aex_runtime_control::generation::{
        HandsGeneration, ImageCapability, ImageIdentifier, ImagePin, ImageVersion, LimitsRevision,
        NetworkPolicy,
    };
    use aex_wire::ids::{
        ContentHash, GenerationId, OrganizationId, PrefixedId as _, SessionId, Uuid7, WorkspaceId,
    };
    use aex_wire::types::ComputeSize;

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
}
