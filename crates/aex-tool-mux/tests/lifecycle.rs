//! End-to-end application tests over fake trusted ports.

use std::sync::{Arc, Mutex};

use aex_hands_protocol::operation::{GuestPath, GuestRoot};
use aex_hands_protocol::rpc::{Fence, HandsOperationId};
use aex_runtime_control::{HandId, HandRecord};
use aex_tool_mux::{
    ExecutorOutput, FullOutput, GuestPort, McpPort, OfficialSandboxTool, PreparationProgress,
    ReadyHand, ResultRetentionPort, RuntimePort, SandboxConfig, StoragePersistPort, TelemetryEvent,
    TelemetryKind, TelemetryPort, ToolCallIdentity, ToolCompletion, ToolHandle, ToolMux,
    ToolMuxFuture, ToolStart, ToolStartRequest, ToolTarget,
};
use aex_wire::ids::{
    AgentId, ContentHash, GenerationId, MessageId, PrefixedId, ResourceName, SessionId, Uuid7,
    WorkspaceId,
};

fn id<T: PrefixedId>(seed: u8) -> T {
    T::from_uuid7(Uuid7::compose(u64::from(seed), [seed; 10]))
}

fn identity() -> ToolCallIdentity {
    ToolCallIdentity {
        organization: id(0),
        workspace: id::<WorkspaceId>(1),
        session: id::<SessionId>(2),
        agent: id::<AgentId>(3),
        message: id::<MessageId>(4),
        batch: 5,
        call: "call-6".to_owned(),
        attempt: 1,
    }
}

fn sandbox(enabled: bool) -> SandboxConfig {
    SandboxConfig {
        enabled,
        generation: enabled.then(|| id::<GenerationId>(7)),
    }
}

fn request(target: ToolTarget, enabled: bool) -> ToolStartRequest {
    ToolStartRequest {
        identity: identity(),
        sandbox: sandbox(enabled),
        target,
        arguments: serde_json::json!({"path": "/workspace/input.txt"}),
        deadline_ms: 60_000,
        max_result_bytes: 1_048_576,
        timeout_ms: 60_000,
    }
}

#[derive(Default)]
struct RuntimeFake {
    eager: Mutex<u32>,
    waits: Mutex<u32>,
    settles: Mutex<u32>,
    foreign_generation: Mutex<bool>,
}

impl RuntimePort for RuntimeFake {
    fn eager_prepare<'a>(
        &'a self,
        session: SessionId,
        sandbox: SandboxConfig,
    ) -> ToolMuxFuture<'a, Result<Vec<PreparationProgress>, String>> {
        *self.eager.lock().expect("eager mutex") += 1;
        Box::pin(async move {
            let mut hand = HandRecord::new(
                session,
                sandbox.generation.expect("enabled generation"),
                sandbox.enabled,
            );
            let _ = hand.eager_action();
            let _ = hand.phase_succeeded().expect("provisioned");
            let _ = hand.phase_succeeded().expect("booted");
            let _ = hand.phase_succeeded().expect("materialized");
            let _ = hand.phase_succeeded().expect("qualified");
            let _ = hand.phase_succeeded().expect("suspended");
            Ok(vec![
                PreparationProgress::Provisioning,
                PreparationProgress::Booting,
                PreparationProgress::MaterializingWorkspace,
                PreparationProgress::QualifyingSandboxMcp,
                PreparationProgress::Ready,
                PreparationProgress::Suspending,
                PreparationProgress::Suspended,
            ])
        })
    }

    fn wait_ready<'a>(
        &'a self,
        session: SessionId,
        hand: HandId,
        generation: GenerationId,
        _call: &'a ToolCallIdentity,
    ) -> ToolMuxFuture<'a, Result<ReadyHand, String>> {
        *self.waits.lock().expect("wait mutex") += 1;
        let foreign = *self.foreign_generation.lock().expect("foreign mutex");
        Box::pin(async move {
            assert_eq!(hand, HandId::for_session(session));
            Ok(ReadyHand {
                hand,
                generation: if foreign {
                    id::<GenerationId>(99)
                } else {
                    generation
                },
                fence: Fence(8),
            })
        })
    }

    fn settle_waiter<'a>(
        &'a self,
        _ready: ReadyHand,
        _call: &'a ToolCallIdentity,
    ) -> ToolMuxFuture<'a, Result<(), String>> {
        *self.settles.lock().expect("settle mutex") += 1;
        Box::pin(async { Ok(()) })
    }
}

#[derive(Default)]
struct GuestFake {
    hellos: Mutex<Vec<ReadyHand>>,
    starts: Mutex<Vec<(ReadyHand, ToolTarget)>>,
    large: Mutex<bool>,
    fail_hello: Mutex<bool>,
    cancels: Mutex<u32>,
}

impl GuestPort for GuestFake {
    fn hello<'a>(&'a self, ready: ReadyHand) -> ToolMuxFuture<'a, Result<(), String>> {
        self.hellos.lock().expect("hello mutex").push(ready);
        let fail = *self.fail_hello.lock().expect("fail hello mutex");
        Box::pin(async move {
            if fail {
                Err("guest hello failed".to_owned())
            } else {
                Ok(())
            }
        })
    }

    fn start<'a>(
        &'a self,
        ready: ReadyHand,
        target: &'a ToolTarget,
        _arguments: &'a serde_json::Value,
        _call: &'a ToolCallIdentity,
    ) -> ToolMuxFuture<'a, Result<Result<ExecutorOutput, HandsOperationId>, String>> {
        self.starts
            .lock()
            .expect("start mutex")
            .push((ready, target.clone()));
        let large = *self.large.lock().expect("large mutex");
        Box::pin(async move {
            if large {
                let path = GuestPath::parse(
                    &GuestRoot::workspace(),
                    "/workspace/.aex/tool-results/call-6.bin",
                )
                .expect("contained result path");
                Ok(Ok(ExecutorOutput {
                    preview: vec![b'x'; 70_000],
                    full: FullOutput::SandboxFile {
                        path,
                        bytes: 12_000_000,
                        hash: ContentHash::from_bytes([9; 32]),
                    },
                    is_error: false,
                }))
            } else {
                Ok(Ok(ExecutorOutput {
                    preview: b"ok".to_vec(),
                    full: FullOutput::Inline(b"ok".to_vec()),
                    is_error: false,
                }))
            }
        })
    }

    fn read<'a>(
        &'a self,
        _ready: ReadyHand,
        _operation: HandsOperationId,
        _max_result_bytes: usize,
        _timeout_ms: u32,
    ) -> ToolMuxFuture<'a, Result<Option<ExecutorOutput>, String>> {
        Box::pin(async { Ok(None) })
    }

    fn cancel<'a>(
        &'a self,
        _ready: ReadyHand,
        _operation: HandsOperationId,
    ) -> ToolMuxFuture<'a, Result<(), String>> {
        *self.cancels.lock().expect("cancel mutex") += 1;
        Box::pin(async { Ok(()) })
    }
}

#[derive(Default)]
struct McpFake {
    calls: Mutex<u32>,
}

impl McpPort for McpFake {
    fn call_remote<'a>(
        &'a self,
        _endpoint: &'a str,
        _headers: &'a std::collections::BTreeMap<String, ResourceName>,
        _server: &'a ResourceName,
        _tool: &'a str,
        _arguments: &'a serde_json::Value,
        _call: &'a ToolCallIdentity,
    ) -> ToolMuxFuture<'a, Result<ExecutorOutput, String>> {
        *self.calls.lock().expect("mcp mutex") += 1;
        Box::pin(async {
            Ok(ExecutorOutput {
                preview: b"remote".to_vec(),
                full: FullOutput::Inline(b"remote".to_vec()),
                is_error: false,
            })
        })
    }
}

#[derive(Default)]
struct StorageFake {
    calls: Mutex<Vec<ReadyHand>>,
}

impl StoragePersistPort for StorageFake {
    fn persist<'a>(
        &'a self,
        ready: ReadyHand,
        _source: &'a GuestPath,
        logical_name: &'a str,
        _media_type: Option<&'a str>,
        _call: &'a ToolCallIdentity,
    ) -> ToolMuxFuture<'a, Result<ExecutorOutput, String>> {
        self.calls.lock().expect("storage mutex").push(ready);
        let output = format!(r#"{{"name":"{logical_name}","hash":"sha256:09"}}"#);
        Box::pin(async move {
            Ok(ExecutorOutput {
                preview: output.as_bytes().to_vec(),
                full: FullOutput::Inline(output.into_bytes()),
                is_error: false,
            })
        })
    }
}

#[derive(Default)]
struct ResultsFake {
    inline: Mutex<u32>,
    sandbox: Mutex<Vec<ReadyHand>>,
}

impl ResultRetentionPort for ResultsFake {
    fn retain_inline<'a>(
        &'a self,
        call: &'a ToolCallIdentity,
        body: &'a [u8],
    ) -> ToolMuxFuture<'a, Result<aex_tool_mux::RetainedResult, String>> {
        *self.inline.lock().expect("inline mutex") += 1;
        let bytes = body.len() as u64;
        let hash = ContentHash::from_bytes(*blake3::hash(body).as_bytes());
        let object_ref = format!("session/{}/tool/{}", call.session, call.call);
        Box::pin(async move {
            Ok(aex_tool_mux::RetainedResult {
                object_ref,
                bytes,
                hash,
                sandbox_path: None,
            })
        })
    }

    fn retain_sandbox_file<'a>(
        &'a self,
        call: &'a ToolCallIdentity,
        ready: ReadyHand,
        path: &'a GuestPath,
        bytes: u64,
        hash: ContentHash,
    ) -> ToolMuxFuture<'a, Result<aex_tool_mux::RetainedResult, String>> {
        self.sandbox.lock().expect("sandbox mutex").push(ready);
        let object_ref = format!("session/{}/tool/{}", call.session, call.call);
        let sandbox_path = Some(path.as_str().to_owned());
        Box::pin(async move {
            Ok(aex_tool_mux::RetainedResult {
                object_ref,
                bytes,
                hash,
                sandbox_path,
            })
        })
    }

    fn read_handle<'a>(
        &'a self,
        _handle: &'a ToolHandle,
    ) -> ToolMuxFuture<'a, Result<Option<ExecutorOutput>, String>> {
        Box::pin(async { Ok(None) })
    }
}

#[derive(Default)]
struct TelemetryFake {
    events: Mutex<Vec<TelemetryEvent>>,
}

impl TelemetryPort for TelemetryFake {
    fn try_emit(
        &self,
        envelope: aex_tool_mux::TelemetryEnvelope,
    ) -> Result<(), aex_tool_mux::TelemetryPressure> {
        self.events
            .lock()
            .expect("telemetry mutex")
            .push(envelope.event);
        Ok(())
    }
}

struct Fixture {
    mux: ToolMux,
    runtime: Arc<RuntimeFake>,
    guest: Arc<GuestFake>,
    mcp: Arc<McpFake>,
    storage: Arc<StorageFake>,
    results: Arc<ResultsFake>,
    telemetry: Arc<TelemetryFake>,
}

fn fixture() -> Fixture {
    let runtime = Arc::new(RuntimeFake::default());
    let guest = Arc::new(GuestFake::default());
    let mcp = Arc::new(McpFake::default());
    let storage = Arc::new(StorageFake::default());
    let results = Arc::new(ResultsFake::default());
    let telemetry = Arc::new(TelemetryFake::default());
    let mux = ToolMux::new(
        runtime.clone(),
        guest.clone(),
        mcp.clone(),
        storage.clone(),
        results.clone(),
        telemetry.clone(),
    );
    Fixture {
        mux,
        runtime,
        guest,
        mcp,
        storage,
        results,
        telemetry,
    }
}

#[tokio::test]
async fn eager_setup_is_visible_and_disabled_sessions_make_no_runtime_call() {
    let fixture = fixture();
    fixture
        .mux
        .eager_prepare(identity().session, sandbox(true))
        .await
        .expect("eager setup");
    assert_eq!(*fixture.runtime.eager.lock().expect("eager mutex"), 1);
    let events = fixture.telemetry.events.lock().expect("telemetry mutex");
    assert_eq!(events[0].kind, TelemetryKind::SandboxRequested);
    assert!(events.iter().any(|event| {
        event.kind
            == TelemetryKind::SandboxProgress {
                progress: PreparationProgress::MaterializingWorkspace,
            }
    }));
    assert!(events.iter().any(|event| {
        event.kind
            == TelemetryKind::SandboxProgress {
                progress: PreparationProgress::Suspended,
            }
    }));
    drop(events);
    fixture
        .mux
        .eager_prepare(identity().session, sandbox(false))
        .await
        .expect("disabled is a no-op");
    assert_eq!(*fixture.runtime.eager.lock().expect("eager mutex"), 1);
}

#[tokio::test]
async fn sandbox_opt_out_is_model_visible_and_never_touches_runtime_or_guest() {
    let fixture = fixture();
    let outcome = fixture
        .mux
        .start(&request(
            ToolTarget::OfficialSandbox {
                tool: OfficialSandboxTool::Read,
            },
            false,
        ))
        .await
        .expect("typed completion");
    let ToolStart::Completed { result } = outcome else {
        panic!("disabled is terminal")
    };
    assert_eq!(
        result.error.as_ref().map(|error| error.code.as_str()),
        Some("sandbox_disabled")
    );
    assert_eq!(*fixture.runtime.waits.lock().expect("wait mutex"), 0);
    assert!(fixture.guest.hellos.lock().expect("hello mutex").is_empty());
}

#[tokio::test]
async fn remote_streamable_mcp_runs_without_a_hand_even_when_sandbox_is_disabled() {
    let fixture = fixture();
    let outcome = fixture
        .mux
        .start(&request(
            ToolTarget::RemoteMcp {
                server: ResourceName::parse("clock").expect("name"),
                endpoint: "https://mcp.example.test/api".to_owned(),
                headers: std::collections::BTreeMap::new(),
                tool: "now".to_owned(),
            },
            false,
        ))
        .await
        .expect("remote MCP");
    assert!(matches!(outcome, ToolStart::Completed { .. }));
    assert_eq!(*fixture.mcp.calls.lock().expect("mcp mutex"), 1);
    assert_eq!(*fixture.runtime.waits.lock().expect("wait mutex"), 0);
    assert!(fixture.guest.hellos.lock().expect("hello mutex").is_empty());
}

#[tokio::test]
async fn official_and_sandbox_mcp_wait_for_and_hello_the_exact_generation() {
    let fixture = fixture();
    for target in [
        ToolTarget::OfficialSandbox {
            tool: OfficialSandboxTool::Bash,
        },
        ToolTarget::SandboxMcp {
            server: ResourceName::parse("local").expect("name"),
            command: "server".to_owned(),
            args: Vec::new(),
            environment: std::collections::BTreeMap::new(),
            working_directory: None,
            tool: "compile".to_owned(),
        },
    ] {
        fixture
            .mux
            .start(&request(target, true))
            .await
            .expect("sandbox target");
    }
    assert_eq!(*fixture.runtime.waits.lock().expect("wait mutex"), 2);
    let hellos = fixture.guest.hellos.lock().expect("hello mutex");
    assert_eq!(hellos.len(), 2);
    assert!(
        hellos
            .iter()
            .all(|ready| ready.generation == id::<GenerationId>(7))
    );
    assert_eq!(*fixture.runtime.settles.lock().expect("settle mutex"), 2);
}

#[tokio::test]
async fn a_foreign_ready_generation_is_refused_before_guest_dispatch() {
    let fixture = fixture();
    *fixture
        .runtime
        .foreign_generation
        .lock()
        .expect("foreign mutex") = true;
    let error = fixture
        .mux
        .start(&request(
            ToolTarget::OfficialSandbox {
                tool: OfficialSandboxTool::Read,
            },
            true,
        ))
        .await
        .expect_err("foreign generation");
    assert!(error.contains("foreign Hand generation"));
    assert!(
        fixture
            .guest
            .starts
            .lock()
            .expect("starts mutex")
            .is_empty()
    );
}

#[tokio::test]
async fn a_large_result_is_a_bounded_preview_plus_sandbox_file_and_retained_object() {
    let fixture = fixture();
    *fixture.guest.large.lock().expect("large mutex") = true;
    let ToolStart::Completed { result } = fixture
        .mux
        .start(&request(
            ToolTarget::OfficialSandbox {
                tool: OfficialSandboxTool::Bash,
            },
            true,
        ))
        .await
        .expect("large output")
    else {
        panic!("fake completes")
    };
    assert_eq!(result.preview.len(), 65_536);
    assert!(result.truncated);
    let retained = result.retained.expect("retained full result");
    assert_eq!(retained.bytes, 12_000_000);
    assert_eq!(
        retained.sandbox_path.as_deref(),
        Some("/workspace/.aex/tool-results/call-6.bin")
    );
    assert_eq!(
        fixture.results.sandbox.lock().expect("sandbox mutex")[0].generation,
        id::<GenerationId>(7)
    );
    let events = fixture.telemetry.events.lock().expect("telemetry mutex");
    assert!(events.iter().any(|event| matches!(
        event.kind,
        TelemetryKind::ToolPreview {
            bytes: 65_536,
            truncated: true
        }
    )));
    assert!(events.iter().any(|event| matches!(
        event.kind,
        TelemetryKind::ToolResultRetained {
            bytes: 12_000_000,
            ..
        }
    )));
}

#[tokio::test]
async fn storage_persist_uses_the_same_exact_generation_readiness_path() {
    let fixture = fixture();
    let source =
        GuestPath::parse(&GuestRoot::workspace(), "/workspace/report.pdf").expect("source path");
    let outcome = fixture
        .mux
        .start(&request(
            ToolTarget::StoragePersist {
                source,
                logical_name: "report.pdf".to_owned(),
                media_type: Some("application/pdf".to_owned()),
            },
            true,
        ))
        .await
        .expect("persist");
    let ToolStart::Completed {
        result:
            ToolCompletion {
                error: None,
                retained: Some(_),
                ..
            },
    } = outcome
    else {
        panic!("storage persisted and retained")
    };
    assert_eq!(
        fixture.storage.calls.lock().expect("storage mutex")[0].generation,
        id::<GenerationId>(7)
    );
}

#[tokio::test]
async fn readiness_waiter_is_settled_when_guest_startup_fails() {
    let fixture = fixture();
    *fixture.guest.fail_hello.lock().expect("fail hello mutex") = true;
    fixture
        .mux
        .start(&request(
            ToolTarget::OfficialSandbox {
                tool: OfficialSandboxTool::Read,
            },
            true,
        ))
        .await
        .expect_err("guest startup must fail");
    assert_eq!(*fixture.runtime.settles.lock().expect("settle mutex"), 1);
}

#[tokio::test]
async fn cancellation_releases_the_durable_waiter() {
    let fixture = fixture();
    let request = aex_tool_mux::ToolHandleRequest {
        identity: identity(),
        handle: ToolHandle::Sandbox {
            hand: HandId::for_session(identity().session),
            generation: id::<GenerationId>(7),
            fence: Fence(8),
            operation: HandsOperationId(Uuid7::compose(9, [9; 10])),
            max_result_bytes: 1_048_576,
            timeout_ms: 60_000,
        },
    };
    fixture.mux.cancel(&request).await.expect("cancel");
    assert_eq!(*fixture.guest.cancels.lock().expect("cancel mutex"), 1);
    assert_eq!(*fixture.runtime.settles.lock().expect("settle mutex"), 1);
}
