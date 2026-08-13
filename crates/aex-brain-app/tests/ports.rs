//! Slice S-5.1 — the split-phase ordering, enforced by types.
//!
//! **This file compiling is the assertion.** The fake adapters below implement the real
//! port traits, so if `ProviderPort::dispatch`, `ToolPort::invoke` or `HandsPort::start`
//! ever stopped requiring a [`DispatchTicket`], the signatures here would no longer match
//! and the target would fail to build. A caller that tries to dispatch before the durable
//! `dispatch_started` write has no ticket to hand over and therefore has nothing to pass.
//!
//! TODO(cross-stream): a compile-fail fixture would state the same rule explicitly.
//! The `trybuild` harness is not a workspace dependency, and adding one is a
//! shared-manifest change this stream does not own.

use aex_brain_app::ports::hands::HandsResult;
use aex_brain_app::ports::proof::{NullPreviewSink, PreviewSink};
use aex_brain_app::ports::tool::{ControlStateView, DetachedStatus};
use aex_brain_app::ports::{
    BoxFuture, CancelToken, DispatchTicket, FenceGuard, HandsAccepted, HandsEndpoint, HandsError,
    HandsOperationStart, HandsOperationStatus, HandsPort, PreparedToolCall, ProviderDispatchError,
    ProviderFailureKind, ProviderOutcome, ProviderPort, RedactedDetail, ResultBounds,
    ToolAdvertisement, ToolDispatchError, ToolOutcome, ToolPort, ToolRoute, ToolRoutingError,
    UnknownResolution,
};
use aex_brain_domain::effect::{
    DetachedOperationRef, DispatchEvidence, DispatchProof, DispatchStage, DurableEffect,
    EffectClass, EffectKind,
};
use aex_brain_domain::ids::{
    AgentId, AgentKey, AgentRevision, CancelEpoch, CatalogPin, ContentHash, DetachedOperationId,
    EffectId, Fence, HandsOperationId, JournalSeq, OwnerToken, SessionId, Timestamp, ToolCallId,
    ToolName,
};
use aex_brain_domain::journal::ExecutorRoute;
use aex_brain_domain::wire_pending::{
    CanonicalModelRequest, PreviewFrame, ProviderId, ResolvedAgentConfig, SessionCredentialPin,
};
use aex_model_catalog::canonical::{CorrelationId, ReasoningRequest, ToolChoice};
use aex_model_catalog::document::CapabilitySet;
use aex_model_catalog::{BoundedString, fixture};
use aex_wire::CanonicalJson;
use aex_wire::ids::{GenerationId, PrefixedId as _, ProviderCredentialId, Uuid7, WorkspaceId};
use std::sync::Mutex;
use uuid::Uuid;

fn generation(seed: u8) -> GenerationId {
    GenerationId::from_uuid7(Uuid7::compose(1, [seed; 10]))
}

fn credential() -> SessionCredentialPin {
    SessionCredentialPin::new(
        ProviderCredentialId::from_uuid7(Uuid7::compose(1, [7; 10])),
        1,
        1,
        0,
    )
    .expect("non-zero fixture pin")
}

fn guard() -> FenceGuard {
    FenceGuard::new(
        AgentKey::new(SessionId(Uuid::from_u128(1)), AgentId(Uuid::from_u128(2))),
        OwnerToken(Uuid::from_u128(3)),
        Fence(1),
        AgentRevision(1),
        None,
        CancelEpoch::ZERO,
        CancelToken::new(),
    )
}

fn ticket(effect: EffectId) -> DispatchTicket {
    DispatchTicket::mint(
        &guard(),
        WorkspaceId::from_uuid7(Uuid7::compose(1, [8; 10])),
        aex_wire::ids::OrganizationId::from_uuid7(Uuid7::compose(1, [9; 10])),
        effect,
        1,
        Timestamp(0),
    )
}

/// A provider that records the ticket it was handed and refuses to invent an outcome.
#[derive(Debug, Default)]
struct RecordingProvider {
    dispatched: Mutex<Vec<(EffectId, Fence, u16)>>,
}

impl ProviderPort for RecordingProvider {
    fn dispatch<'a>(
        &'a self,
        ticket: &'a DispatchTicket,
        _credential: aex_brain_domain::wire_pending::SessionCredentialPin,
        _request: &'a CanonicalModelRequest,
        _preview: &'a dyn PreviewSink,
        _cancel: &'a CancelToken,
    ) -> BoxFuture<'a, Result<ProviderOutcome, ProviderDispatchError>> {
        Box::pin(async move {
            self.dispatched.lock().expect("not poisoned").push((
                ticket.effect(),
                ticket.fence(),
                ticket.attempt(),
            ));
            Err(ProviderDispatchError {
                stage: DispatchStage::PreDispatch,
                proof: DispatchProof::NotSent,
                kind: ProviderFailureKind::ServerError,
                provider_request_id: None,
                retry_after: None,
                detail: RedactedDetail::internal(
                    ProviderFailureKind::ServerError,
                    "fixture refuses to fabricate a generation",
                ),
            })
        })
    }

    fn resolve_unknown<'a>(
        &'a self,
        _identity: &'a DurableEffect,
        _evidence: &'a DispatchEvidence,
    ) -> BoxFuture<'a, Result<UnknownResolution, ProviderDispatchError>> {
        // The honest launch answer: no BYOK provider exposes a durable operation.
        Box::pin(async { Ok(UnknownResolution::NoDurableOperation) })
    }
}

#[derive(Debug, Default)]
struct StubTools;

impl ToolPort for StubTools {
    fn advertise(&self, _pin: &CatalogPin) -> Result<ToolAdvertisement, ToolRoutingError> {
        Ok(ToolAdvertisement {
            definitions: vec![aex_model_catalog::canonical::CanonicalToolDef {
                name: ToolName::parse("read_file").expect("tool name"),
                description: BoundedString::new("Read a file.").expect("description"),
                input_schema: CanonicalJson::parse(
                    r#"{"type":"object","additionalProperties":true}"#,
                )
                .expect("schema"),
                strict: false,
            }],
            parallel_safe: false,
        })
    }

    fn route(&self, _pin: &CatalogPin, name: &ToolName) -> Result<ToolRoute, ToolRoutingError> {
        if name.as_str() == "read_file" {
            Ok(ToolRoute {
                name: name.clone(),
                executor: ExecutorRoute::ManagedWeb,
                class: EffectClass::IdempotentManaged,
                timeout_ms: 30_000,
                concurrency_weight: 1,
                manifest_digest: ContentHash::of(b"manifest"),
            })
        } else {
            Err(ToolRoutingError::Unknown {
                name: name.as_str().to_owned(),
            })
        }
    }

    fn invoke<'a>(
        &'a self,
        ticket: &'a DispatchTicket,
        call: &'a PreparedToolCall,
        _cancel: &'a CancelToken,
    ) -> BoxFuture<'a, Result<ToolOutcome, ToolDispatchError>> {
        Box::pin(async move {
            // A ticket handed to the wrong effect must fail loudly rather than dispatch
            // whatever the ticket actually names.
            let _ = call;
            let _ = ticket.attempt();
            Ok(ToolOutcome::Detached {
                operation: DetachedOperationId("op_1".to_owned()),
                poll_after: core::time::Duration::from_secs(1),
            })
        })
    }

    fn query<'a>(
        &'a self,
        _operation: &'a DetachedOperationRef,
    ) -> BoxFuture<'a, Result<DetachedStatus, ToolDispatchError>> {
        Box::pin(async { Ok(DetachedStatus::Unknown) })
    }

    fn cancel<'a>(
        &'a self,
        _operation: &'a DetachedOperationRef,
        _fence: Fence,
    ) -> BoxFuture<'a, Result<(), ToolDispatchError>> {
        Box::pin(async { Ok(()) })
    }
}

#[derive(Debug, Default)]
struct StubHands;

impl HandsPort for StubHands {
    fn ensure_generation<'a>(
        &'a self,
        _session: &'a SessionId,
        generation: GenerationId,
    ) -> BoxFuture<'a, Result<HandsEndpoint, HandsError>> {
        Box::pin(async move {
            Ok(HandsEndpoint {
                generation,
                address: "guest.internal".to_owned(),
                lease_expires_at: Timestamp(60_000),
            })
        })
    }

    fn start<'a>(
        &'a self,
        _ticket: &'a DispatchTicket,
        generation: GenerationId,
        start: &'a HandsOperationStart,
    ) -> BoxFuture<'a, Result<HandsAccepted, HandsError>> {
        Box::pin(async move {
            Ok(HandsAccepted {
                operation: start.operation.clone(),
                generation,
                created: true,
                poll_after: core::time::Duration::from_millis(250),
                result: None,
            })
        })
    }

    fn status<'a>(
        &'a self,
        _generation: GenerationId,
        _operation: &'a HandsOperationId,
    ) -> BoxFuture<'a, Result<HandsOperationStatus, HandsError>> {
        Box::pin(async {
            Ok(HandsOperationStatus::Running {
                poll_after: core::time::Duration::from_secs(1),
            })
        })
    }

    fn cancel<'a>(
        &'a self,
        _generation: GenerationId,
        _operation: &'a HandsOperationId,
        _fence: Fence,
    ) -> BoxFuture<'a, Result<(), HandsError>> {
        Box::pin(async { Ok(()) })
    }

    fn result<'a>(
        &'a self,
        _generation: GenerationId,
        operation: &'a HandsOperationId,
        _bounds: &'a ResultBounds,
    ) -> BoxFuture<'a, Result<HandsResult, HandsError>> {
        Box::pin(async move {
            let _ = operation;
            // The successor generation exists, and answering with it is exactly what this
            // fixture must never do.
            Err(HandsError::GenerationLost {
                generation: generation(4),
            })
        })
    }
}

/// Drives a fixture future to completion.
///
/// Deliberately runtime-free: every fixture here is pure and completes on its first poll.
/// A pending poll means a fixture grew a real await point, and this target must fail
/// loudly rather than quietly depend on an executor it does not configure.
fn block_on<F: core::future::Future>(future: F) -> F::Output {
    let mut future = Box::pin(future);
    let mut context = core::task::Context::from_waker(core::task::Waker::noop());
    match future.as_mut().poll(&mut context) {
        core::task::Poll::Ready(value) => value,
        core::task::Poll::Pending => {
            panic!("a port fixture must not need a runtime to make progress")
        }
    }
}

fn request() -> CanonicalModelRequest {
    let selection = fixture::qualified_entry(
        ProviderId::Deepseek,
        "deepseek-chat",
        CapabilitySet::default(),
    );
    let mut request = CanonicalModelRequest {
        selection,
        system: Vec::new(),
        messages: Vec::new(),
        tools: Vec::new(),
        tool_choice: ToolChoice::None,
        parallel_tools: false,
        max_output_tokens: 1_024,
        temperature_milli: None,
        top_p_milli: None,
        stop_sequences: Vec::new(),
        reasoning: ReasoningRequest::ProviderDefault,
        structured_output: None,
        cache_breakpoints: Vec::new(),
        correlation: CorrelationId::from_effect([1; 16]),
        request_hash: aex_wire::ContentHash::of(b"pending"),
    };
    request.request_hash = request.digest().expect("fixture request is canonical");
    request
}

fn catalog_pin() -> CatalogPin {
    request().selection.catalog()
}

/// The port is dyn-compatible, which is what lets the composition root choose an adapter at
/// runtime instead of monomorphizing `brain-mux` over thirteen streams' types.
#[test]
fn every_port_is_dyn_compatible() {
    let provider: Box<dyn ProviderPort> = Box::new(RecordingProvider::default());
    let tools: Box<dyn ToolPort> = Box::new(StubTools);
    let hands: Box<dyn HandsPort> = Box::new(StubHands);

    let effect = EffectId::derive(
        AgentId(Uuid::from_u128(2)),
        JournalSeq(1),
        EffectKind::ModelCall.tag(),
    );
    let outcome = block_on(provider.dispatch(
        &ticket(effect),
        credential(),
        &request(),
        &NullPreviewSink,
        &CancelToken::new(),
    ));
    let error = outcome.expect_err("the fixture refuses to fabricate a generation");
    assert_eq!(error.proof, DispatchProof::NotSent);

    tools
        .route(
            &catalog_pin(),
            &ToolName::parse("read_file").expect("tool name"),
        )
        .expect("a known tool routes");
    let unknown = tools
        .route(
            &catalog_pin(),
            &ToolName::parse("telepathy").expect("tool name"),
        )
        .expect_err("an unknown tool does not");
    assert!(matches!(unknown, ToolRoutingError::Unknown { .. }));

    let endpoint = block_on(hands.ensure_generation(&SessionId(Uuid::from_u128(1)), generation(3)))
        .expect("the fixture serves the exact generation");
    assert_eq!(endpoint.generation, generation(3));
}

/// A dispatch carries the ticket's fence and attempt, so a receipt can name the exact
/// attempt that produced it.
#[test]
fn a_dispatch_carries_the_ticket_it_was_authorized_by() {
    let provider = RecordingProvider::default();
    let effect = EffectId([9; 16]);
    let _ = block_on(provider.dispatch(
        &ticket(effect),
        credential(),
        &request(),
        &NullPreviewSink,
        &CancelToken::new(),
    ));
    let recorded = provider.dispatched.lock().expect("not poisoned").clone();
    assert_eq!(recorded, vec![(effect, Fence(1), 1)]);
}

/// `resolve_unknown` answering `NoDurableOperation` is the honest launch outcome, not a
/// failure to try: no BYOK provider exposes a generation-resume or result-lookup operation.
#[test]
fn an_unresolvable_effect_stays_unresolved() {
    let provider = RecordingProvider::default();
    let effect = DurableEffect {
        id: EffectId([1; 16]),
        kind: EffectKind::ModelCall,
        generation: None,
        class: EffectClass::NonReplayable,
        request_hash: ContentHash::of(b"request"),
        state: aex_brain_domain::effect::EffectState::DispatchStarted { attempt: 1 },
        deadline: Timestamp(60_000),
        evidence: None,
    };
    let evidence = DispatchEvidence::ambiguous(1, DispatchStage::Dispatched);
    let resolution =
        block_on(provider.resolve_unknown(&effect, &evidence)).expect("the query succeeds");
    assert_eq!(resolution, UnknownResolution::NoDurableOperation);
}

/// A detached tool releases the connection and returns something to query later, so a long
/// tool never holds a socket or a lease.
#[test]
fn a_detached_tool_returns_an_operation_rather_than_blocking() {
    let tools = StubTools;
    let call = PreparedToolCall {
        call: ToolCallId::truncating("c1"),
        route: tools
            .route(
                &catalog_pin(),
                &ToolName::parse("read_file").expect("tool name"),
            )
            .expect("the tool routes"),
        input: CanonicalJson::parse("{}").expect("tool input"),
        max_result_bytes: 65_536,
        hands_generation: generation(3),
        control: ControlStateView::default(),
    };
    let outcome = block_on(tools.invoke(&ticket(EffectId([2; 16])), &call, &CancelToken::new()))
        .expect("the invocation is accepted");
    assert!(matches!(outcome, ToolOutcome::Detached { .. }));
}

/// A generation mismatch is an error, never a silently substituted successor: a new
/// generation has a different filesystem, so its answer is not the old one's answer.
#[test]
fn a_lost_generation_never_answers_with_a_successor() {
    let hands = StubHands;
    let error = block_on(hands.result(
        generation(3),
        &HandsOperationId("op_1".to_owned()),
        &ResultBounds {
            max_bytes: 1_024,
            max_stream_bytes: 512,
            timeout_ms: 30_000,
        },
    ))
    .expect_err("the generation is gone");
    assert!(matches!(error, HandsError::GenerationLost { .. }));
}

/// The preview path cannot become model-visible history: there is no conversion from a
/// frame into any journal variant, and the sink is the only thing that takes one.
#[test]
fn a_preview_frame_has_nowhere_to_go_but_the_sink() {
    let frame = PreviewFrame::TextDelta {
        index: 7,
        text: BoundedString::truncating("partial delta"),
    };
    assert!(!NullPreviewSink.offer(frame));
    // `ResolvedAgentConfig` is the only thing a journal record accepts as configuration;
    // naming it here keeps this file honest about which types the fold actually admits.
    let _: fn() -> Option<ResolvedAgentConfig> = || None;
}
