//! Provider protocol properties against a local mock: the compiled router
//! admits, streams, seals, retries 429/503, and stops terminal.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use aex_brain_app::ports::proof::{CancelToken, DispatchTicket, FenceGuard, NullPreviewSink};
use aex_brain_app::ports::provider::ProviderPort;
use aex_brain_app::ports::store::EffectStore;
use aex_brain_app::ports::{BoxFuture, CommitError, StoreError};
use aex_brain_domain::effect::{DispatchEvidence, DispatchProof, DispatchStage, DurableEffect};
use aex_brain_domain::ids::{
    AgentId, AgentKey, AgentRevision, CancelEpoch, EffectId, Fence, JournalSeq, OwnerToken,
    SessionId,
};
use aex_brain_domain::wire_pending::SessionCredentialPin;
use aex_brain_provider::RigProviderRouter;
use aex_brain_provider_custody::credential::{
    BindingState, CredentialResolveError, CredentialRevision, ProviderApiKey,
    ProviderCredentialBinding, ProviderCredentialDecryptor, ProviderCredentialDirectory,
    RevocationEpoch,
};
use aex_model_catalog::QualifiedModel;
use aex_model_catalog::canonical::{
    CanonicalBlock, CanonicalMessage, CanonicalModelRequest, CorrelationId, ReasoningRequest, Role,
    StopReason, SystemBlock, ToolChoice, seal,
};
use aex_model_catalog::document::CapabilitySet;
use aex_model_catalog::fixture;
use aex_model_catalog::primitives::BoundedString;
use aex_wire::ids::{OrganizationId, PrefixedId as _, ProviderCredentialId, WorkspaceId};
use aex_wire::provider::ProviderId;
use aex_wire::types::Region;
use aex_wire::{CanonicalJson, ContentHash};

const PROVIDER: ProviderId = ProviderId::Deepseek;
const MODEL: &str = "deepseek-v4-pro";

// ---------------------------------------------------------------------------
// mock server
// ---------------------------------------------------------------------------

#[derive(Default)]
struct MockState {
    requests: AtomicUsize,
    mode: Mutex<MockMode>,
}

#[derive(Clone, Default)]
enum MockMode {
    #[default]
    StreamOk,
    Refuse(u16),
    RefuseTwiceThenOk,
    DropMidStream,
}

fn sse(chunk: &str) -> Vec<u8> {
    format!("data: {chunk}\n\n").into_bytes()
}

fn handle_conn(mut stream: TcpStream, state: &Arc<MockState>) {
    let _ = stream.set_read_timeout(Some(Duration::from_secs(10)));
    let mut buf = Vec::new();
    let mut tmp = [0u8; 4096];
    let header_end;
    loop {
        match stream.read(&mut tmp) {
            Ok(0) | Err(_) => return,
            Ok(n) => {
                buf.extend_from_slice(&tmp[..n]);
                if let Some(pos) = buf.windows(4).position(|window| window == b"\r\n\r\n") {
                    header_end = pos + 4;
                    break;
                }
            }
        }
    }
    let head = String::from_utf8_lossy(&buf[..header_end]).to_string();
    let mut content_length = 0usize;
    for line in head.lines() {
        if let Some(value) = line.to_ascii_lowercase().strip_prefix("content-length:") {
            content_length = value.trim().parse().unwrap_or(0);
        }
    }
    while buf.len() < header_end + content_length {
        match stream.read(&mut tmp) {
            Ok(0) | Err(_) => break,
            Ok(n) => buf.extend_from_slice(&tmp[..n]),
        }
    }
    let body = String::from_utf8_lossy(&buf[header_end..header_end + content_length]).to_string();
    state.requests.fetch_add(1, Ordering::SeqCst);
    let mode = state.mode.lock().expect("not poisoned").clone();

    let _ = stream.set_write_timeout(Some(Duration::from_secs(10)));
    let write_response = |stream: &mut TcpStream, status: u16, payload: &str| {
        let _ = stream.write_all(
            format!(
                "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                payload.len(),
                payload
            )
            .as_bytes(),
        );
    };
    match mode {
        MockMode::Refuse(status) => {
            write_response(
                &mut stream,
                status,
                &format!("{{\"error\":{{\"message\":\"rejected\",\"code\":{status}}}}}"),
            );
        }
        MockMode::RefuseTwiceThenOk => {
            if state.requests.load(Ordering::SeqCst) <= 2 {
                write_response(
                    &mut stream,
                    503,
                    "{\"error\":{\"message\":\"overloaded\",\"code\":503}}",
                );
            } else {
                stream_ok(&mut stream, &body);
            }
        }
        MockMode::StreamOk => stream_ok(&mut stream, &body),
        MockMode::DropMidStream => {
            let _ = stream.write_all(
                b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n",
            );
            let _ = stream.write_all(&sse(
                r#"{"id":"cmpl-1","choices":[{"index":0,"delta":{"content":"partial","tool_calls":[]},"finish_reason":null}],"usage":null}"#,
            ));
            let _ = stream.flush();
            std::thread::sleep(Duration::from_millis(60));
            // Hard reset (RST) so the client sees a transport error, not a
            // clean EOF that would read as a completed stream.
            let sock = socket2::SockRef::from(&stream);
            let _ = sock.set_linger(Some(Duration::ZERO));
            drop(stream);
        }
    }
}

fn stream_ok(stream: &mut TcpStream, _body: &str) {
    let _ = stream.write_all(
        b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n",
    );
    let chunks = [
        sse(
            r#"{"id":"cmpl-1","choices":[{"index":0,"delta":{"content":"Hello","tool_calls":[]},"finish_reason":null}],"usage":null}"#,
        ),
        sse(
            r#"{"id":"cmpl-1","choices":[{"index":0,"delta":{"content":" world","tool_calls":[]},"finish_reason":null}],"usage":null}"#,
        ),
        sse(
            r#"{"id":"cmpl-1","choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"id":"call_1","function":{"name":"read_file","arguments":"{\"path\":\"a\"}"}}]},"finish_reason":"tool_calls"}],"usage":null}"#,
        ),
        sse(
            r#"{"id":"cmpl-1","choices":[],"usage":{"prompt_tokens":10,"completion_tokens":5,"total_tokens":15,"prompt_cache_hit_tokens":0,"prompt_cache_miss_tokens":10}}"#,
        ),
        b"data: [DONE]\n\n".to_vec(),
    ];
    for (index, chunk) in chunks.iter().enumerate() {
        let _ = stream.write_all(chunk);
        let _ = stream.flush();
        if index + 1 < chunks.len() {
            std::thread::sleep(Duration::from_millis(30));
        }
    }
}

fn mock_server(state: Arc<MockState>) -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().expect("local").port();
    std::thread::spawn(move || {
        for conn in listener.incoming() {
            let Ok(stream) = conn else { continue };
            let state = state.clone();
            std::thread::spawn(move || handle_conn(stream, &state));
        }
    });
    port
}

// ---------------------------------------------------------------------------
// fixtures
// ---------------------------------------------------------------------------

fn workspace() -> WorkspaceId {
    WorkspaceId::from_uuid7(aex_wire::Uuid7::compose(1, [1; 10]))
}

fn organization() -> OrganizationId {
    OrganizationId::from_uuid7(aex_wire::Uuid7::compose(1, [2; 10]))
}

fn binding_id() -> ProviderCredentialId {
    ProviderCredentialId::from_uuid7(aex_wire::Uuid7::compose(2, [7; 10]))
}

fn binding() -> ProviderCredentialBinding {
    ProviderCredentialBinding {
        id: binding_id(),
        workspace: workspace(),
        provider: PROVIDER,
        revision: CredentialRevision(1),
        generation: aex_secret_domain::SourceGeneration(1),
        revocation_epoch: RevocationEpoch(0),
        is_default: true,
        state: BindingState::Ready,
        ciphertext: aex_secret_domain::CiphertextRef {
            key_generation: 1,
            wrapped_key: vec![1; 32],
            nonce: Vec::new(),
            ciphertext: vec![2; 32],
        },
        context: aex_secret_domain::EncryptionContext {
            plane: aex_secret_domain::context::Plane::Dev,
            region: Region::EuWest1,
            organization: organization(),
            workspace: workspace(),
            name: aex_secret_domain::SecretName::parse("provider-key").expect("name"),
            generation: aex_secret_domain::SourceGeneration(1),
            custody_revision: None,
        },
        context_digest: [3; 32],
    }
}

struct FixedDirectory;
impl ProviderCredentialDirectory for FixedDirectory {
    fn resolve(
        &self,
        _organization: OrganizationId,
        _workspace: WorkspaceId,
        _provider: ProviderId,
        _id: Option<ProviderCredentialId>,
    ) -> BoxFuture<'_, Result<ProviderCredentialBinding, CredentialResolveError>> {
        let binding = binding();
        Box::pin(async move { Ok(binding) })
    }

    fn revalidate<'a>(
        &'a self,
        binding: &'a ProviderCredentialBinding,
    ) -> BoxFuture<'a, Result<RevocationEpoch, CredentialResolveError>> {
        let epoch = binding.revocation_epoch;
        Box::pin(async move { Ok(epoch) })
    }
}

struct FixedDecryptor;
impl ProviderCredentialDecryptor for FixedDecryptor {
    fn decrypt<'a>(
        &'a self,
        _binding: &'a ProviderCredentialBinding,
        _now: aex_wire::types::Timestamp,
    ) -> BoxFuture<'a, Result<ProviderApiKey, CredentialResolveError>> {
        Box::pin(async move { Ok(ProviderApiKey::new("sk-test-0123456789".to_owned())) })
    }
}

#[derive(Default)]
struct RecordingEffects {
    response_started: Mutex<usize>,
}
impl EffectStore for RecordingEffects {
    fn mark_dispatch_started<'a>(
        &'a self,
        _guard: &'a FenceGuard,
        _authority: &'a aex_brain_app::ports::SessionAuthority,
        _effect: &'a EffectId,
        _attempt: u16,
        _now: aex_brain_domain::ids::Timestamp,
    ) -> BoxFuture<'a, Result<DispatchTicket, CommitError>> {
        panic!("the router never mints tickets")
    }

    fn mark_response_started<'a>(
        &'a self,
        _ticket: &'a DispatchTicket,
        _evidence: &'a DispatchEvidence,
    ) -> BoxFuture<'a, Result<(), CommitError>> {
        Box::pin(async move {
            *self.response_started.lock().expect("not poisoned") += 1;
            Ok(())
        })
    }

    fn load_open<'a>(
        &'a self,
        _key: &'a AgentKey,
    ) -> BoxFuture<'a, Result<Vec<DurableEffect>, StoreError>> {
        Box::pin(async move { Ok(Vec::new()) })
    }
}

fn guard() -> FenceGuard {
    FenceGuard::new(
        AgentKey::new(
            SessionId(uuid::Uuid::from_u128(1)),
            AgentId(uuid::Uuid::from_u128(2)),
        ),
        OwnerToken(uuid::Uuid::from_u128(3)),
        Fence(4),
        AgentRevision(5),
        Some(JournalSeq(6)),
        CancelEpoch(7),
        CancelToken::new(),
    )
}

fn ticket() -> DispatchTicket {
    DispatchTicket::mint(
        &guard(),
        workspace(),
        organization(),
        EffectId([1; 16]),
        1,
        aex_brain_domain::ids::Timestamp(10),
    )
}

fn pin() -> SessionCredentialPin {
    SessionCredentialPin::new(binding_id(), 1, 1, 0).expect("non-zero fixture pin")
}

fn model() -> QualifiedModel {
    fixture::qualified_entry(
        PROVIDER,
        MODEL,
        CapabilitySet::from_slice(&[
            aex_model_catalog::document::Capability::Tools,
            aex_model_catalog::document::Capability::ParallelTools,
        ]),
    )
}

fn request() -> CanonicalModelRequest {
    let mut request = CanonicalModelRequest {
        selection: model(),
        system: vec![SystemBlock {
            text: BoundedString::truncating("be brief"),
            cacheable: true,
        }],
        messages: vec![CanonicalMessage {
            role: Role::User,
            blocks: vec![CanonicalBlock::Text {
                text: BoundedString::truncating("summarize"),
                annotations: Vec::new(),
            }],
        }],
        tools: Vec::new(),
        tool_choice: ToolChoice::Auto,
        parallel_tools: false,
        max_output_tokens: 256,
        temperature_milli: None,
        top_p_milli: None,
        stop_sequences: Vec::new(),
        reasoning: ReasoningRequest::ProviderDefault,
        structured_output: None,
        cache_breakpoints: Vec::new(),
        correlation: CorrelationId(BoundedString::truncating("aex-correlation")),
        request_hash: ContentHash::of(b"placeholder"),
    };
    request.request_hash = request.digest().expect("digest");
    request
}

fn router(state: Arc<MockState>) -> (RigProviderRouter, u16, Arc<RecordingEffects>) {
    let port = mock_server(state);
    let effects = Arc::new(RecordingEffects::default());
    let router = RigProviderRouter::from_build(
        Arc::new(FixedDirectory),
        Arc::new(FixedDecryptor),
        Arc::clone(&effects) as Arc<_>,
        reqwest::Client::new(),
    )
    .with_test_base_url(format!("http://127.0.0.1:{port}"));
    (router, port, effects)
}

fn block_on<F: std::future::Future>(future: F) -> F::Output {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime")
        .block_on(future)
}

// ---------------------------------------------------------------------------
// protocol properties
// ---------------------------------------------------------------------------

#[test]
fn a_streamed_turn_seals_a_tool_use_message_with_a_consistent_receipt() {
    let state = Arc::new(MockState::default());
    *state.mode.lock().expect("not poisoned") = MockMode::StreamOk;
    let (router, _port, effects) = router(state);
    let outcome = block_on(router.dispatch(
        &ticket(),
        pin(),
        &request(),
        &NullPreviewSink,
        &CancelToken::new(),
    ))
    .expect("the mock stream completes");

    let tool_use = outcome
        .message
        .blocks
        .iter()
        .find_map(|block| match block {
            CanonicalBlock::ToolUse { id, name, input } => {
                Some((id.clone(), name.clone(), input.clone()))
            }
            _ => None,
        })
        .expect("the tool call is assembled");
    assert_eq!(tool_use.0.as_str(), "call_1");
    assert_eq!(tool_use.1.as_str(), "read_file");
    assert_eq!(
        tool_use.2,
        CanonicalJson::parse("{\"path\":\"a\"}").expect("canonical arguments")
    );
    let text = outcome
        .message
        .blocks
        .iter()
        .find_map(|block| match block {
            CanonicalBlock::Text { text, .. } => Some(text.as_str().to_owned()),
            _ => None,
        })
        .expect("the text block is assembled");
    assert_eq!(text, "Hello world");
    assert_eq!(outcome.message.stop_reason, StopReason::ToolUse);
    assert_eq!(outcome.usage.input_tokens, 10);
    assert_eq!(outcome.usage.output_tokens, 5);
    assert_eq!(outcome.receipt.attempts, 1);
    assert!(
        outcome.is_consistent(),
        "the receipt commits to the outcome"
    );
    assert_eq!(
        *effects.response_started.lock().expect("not poisoned"),
        1,
        "the durable response-started write landed exactly once"
    );
}

#[test]
fn two_definitive_rejections_then_a_success_records_three_attempts() {
    let state = Arc::new(MockState::default());
    *state.mode.lock().expect("not poisoned") = MockMode::RefuseTwiceThenOk;
    let (router, _port, _effects) = router(state);
    let outcome = block_on(router.dispatch(
        &ticket(),
        pin(),
        &request(),
        &NullPreviewSink,
        &CancelToken::new(),
    ))
    .expect("the third attempt succeeds");
    assert_eq!(outcome.receipt.attempts, 3);
    assert!(outcome.is_consistent());
}

#[test]
fn an_exhausted_retry_loop_stops_terminal_with_the_rejection_kind() {
    let state = Arc::new(MockState::default());
    *state.mode.lock().expect("not poisoned") = MockMode::Refuse(503);
    let (router, _port, _effects) = router(Arc::clone(&state));
    let error = block_on(router.dispatch(
        &ticket(),
        pin(),
        &request(),
        &NullPreviewSink,
        &CancelToken::new(),
    ))
    .expect_err("three 503s exhaust the loop");
    assert_eq!(error.stage, DispatchStage::Terminal);
    assert_eq!(error.proof, DispatchProof::ResponseStarted);
    assert_eq!(
        error.kind,
        aex_model_catalog::ProviderFailureKind::Overloaded
    );
}

#[test]
fn a_mid_stream_drop_never_re_sends_and_stops_ambiguous() {
    let state = Arc::new(MockState::default());
    *state.mode.lock().expect("not poisoned") = MockMode::DropMidStream;
    let (router, _port, _effects) = router(Arc::clone(&state));
    let error = block_on(router.dispatch(
        &ticket(),
        pin(),
        &request(),
        &NullPreviewSink,
        &CancelToken::new(),
    ))
    .expect_err("a drop is ambiguous, never retried");
    assert_eq!(error.stage, DispatchStage::Terminal);
    assert_eq!(error.proof, DispatchProof::PossiblySent);
    assert_eq!(
        error.kind,
        aex_model_catalog::ProviderFailureKind::Transport
    );
    assert_eq!(
        state.requests.load(Ordering::SeqCst),
        1,
        "one send, no retry"
    );
}

#[test]
fn a_hash_mismatch_never_reaches_the_socket() {
    let state = Arc::new(MockState::default());
    *state.mode.lock().expect("not poisoned") = MockMode::StreamOk;
    let (router, _port, _effects) = router(Arc::clone(&state));
    let mut request = request();
    request.max_output_tokens = 1;
    let error = block_on(router.dispatch(
        &ticket(),
        pin(),
        &request,
        &NullPreviewSink,
        &CancelToken::new(),
    ))
    .expect_err("the stale hash is refused");
    assert_eq!(error.stage, DispatchStage::PreDispatch);
    assert_eq!(error.proof, DispatchProof::NotSent);
    assert_eq!(state.requests.load(Ordering::SeqCst), 0);
}

// ---------------------------------------------------------------------------
// the request translation is complete enough for the golden body
// ---------------------------------------------------------------------------

#[test]
fn the_translated_request_names_the_compiled_model_and_tools() {
    let mut request = request();
    request.tools = vec![aex_model_catalog::canonical::CanonicalToolDef {
        name: aex_wire::ResourceName::parse("read_file").expect("name"),
        description: BoundedString::truncating("reads a file"),
        input_schema: CanonicalJson::parse("{\"type\":\"object\"}").expect("schema"),
        strict: true,
    }];
    let rig = aex_brain_provider::translate_request(&request).expect("translates");
    let rendered = serde_json::to_string(&rig).expect("serializes");
    assert!(rendered.contains(MODEL), "{rendered}");
    assert!(rendered.contains("read_file"), "{rendered}");
}

#[test]
fn the_seal_contract_still_holds_for_a_fixture_model() {
    let selected = model();
    let message = seal(
        vec![CanonicalBlock::Text {
            text: BoundedString::truncating("answer"),
            annotations: Vec::new(),
        }],
        StopReason::EndTurn,
        &aex_model_catalog::canonical::NormalizedUsage::default(),
        &selected,
    )
    .expect("seals");
    assert_eq!(message.provider, PROVIDER);
    assert_eq!(message.model.as_str(), MODEL);
}
