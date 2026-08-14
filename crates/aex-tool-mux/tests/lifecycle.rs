//! End-to-end application tests over fake trusted ports.

use std::sync::{Arc, Mutex};

use aex_hands_protocol::operation::{GuestPath, GuestRoot};
use aex_hands_protocol::rpc::{Fence, HandsOperationId};
use aex_runtime_control::HandId;
use aex_tool_mux::{
    ExecutorOutput, FullOutput, GuestPort, OfficialSandboxTool, ReadyHand, RuntimePort,
    SandboxConfig, StoragePersistPort, TelemetryEvent, TelemetryKind, TelemetryPort,
    ToolCallIdentity, ToolHandle, ToolMux, ToolMuxFuture, ToolRead, ToolStart, ToolStartRequest,
    ToolTarget,
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

struct RuntimeFake {
    waits: Mutex<u32>,
    settles: Mutex<u32>,
    cancels: Mutex<u32>,
    ready: Mutex<bool>,
    foreign_generation: Mutex<bool>,
}

impl Default for RuntimeFake {
    fn default() -> Self {
        Self {
            waits: Mutex::new(0),
            settles: Mutex::new(0),
            cancels: Mutex::new(0),
            ready: Mutex::new(true),
            foreign_generation: Mutex::new(false),
        }
    }
}

impl RuntimePort for RuntimeFake {
    fn start_waiter<'a>(
        &'a self,
        session: SessionId,
        hand: HandId,
        _generation: GenerationId,
        _call: &'a ToolCallIdentity,
    ) -> ToolMuxFuture<'a, Result<(), String>> {
        *self.waits.lock().expect("wait mutex") += 1;
        Box::pin(async move {
            assert_eq!(hand, HandId::for_session(session));
            Ok(())
        })
    }

    fn poll_waiter<'a>(
        &'a self,
        session: SessionId,
        hand: HandId,
        generation: GenerationId,
        _call: &'a ToolCallIdentity,
    ) -> ToolMuxFuture<'a, Result<Option<ReadyHand>, String>> {
        let ready = *self.ready.lock().expect("ready mutex");
        let foreign = *self.foreign_generation.lock().expect("foreign mutex");
        Box::pin(async move {
            assert_eq!(hand, HandId::for_session(session));
            Ok(ready.then_some(ReadyHand {
                hand,
                generation: if foreign {
                    id::<GenerationId>(99)
                } else {
                    generation
                },
                fence: Fence(8),
            }))
        })
    }

    fn cancel_waiter<'a>(
        &'a self,
        generation: GenerationId,
        _call: &'a ToolCallIdentity,
    ) -> ToolMuxFuture<'a, Result<Option<ReadyHand>, String>> {
        *self.cancels.lock().expect("cancel mutex") += 1;
        let ready = *self.ready.lock().expect("ready mutex");
        Box::pin(async move {
            Ok(ready.then_some(ReadyHand {
                hand: HandId::for_session(identity().session),
                generation,
                fence: Fence(8),
            }))
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
    fn hello(&self, ready: ReadyHand) -> ToolMuxFuture<'_, Result<(), String>> {
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
        _deadline_ms: i64,
        _max_result_bytes: usize,
        _timeout_ms: u32,
    ) -> ToolMuxFuture<'a, Result<HandsOperationId, String>> {
        self.starts
            .lock()
            .expect("start mutex")
            .push((ready, target.clone()));
        Box::pin(async { Ok(HandsOperationId(Uuid7::compose(9, [9; 10]))) })
    }

    fn read(
        &self,
        _ready: ReadyHand,
        _operation: HandsOperationId,
        _max_result_bytes: usize,
        _timeout_ms: u32,
    ) -> ToolMuxFuture<'_, Result<Option<ExecutorOutput>, String>> {
        let large = *self.large.lock().expect("large mutex");
        Box::pin(async move {
            let output = if large {
                let path = GuestPath::parse(
                    &GuestRoot::workspace(),
                    "/workspace/.aex/tool-results/call-6.bin",
                )
                .expect("contained result path");
                ExecutorOutput {
                    preview: vec![b'x'; 70_000],
                    full: FullOutput::SandboxFile {
                        path,
                        bytes: 12_000_000,
                        hash: ContentHash::from_bytes([9; 32]),
                    },
                    is_error: false,
                }
            } else {
                ExecutorOutput {
                    preview: b"ok".to_vec(),
                    full: FullOutput::Inline(b"ok".to_vec()),
                    is_error: false,
                }
            };
            Ok(Some(output))
        })
    }

    fn cancel<'a>(
        &'a self,
        _ready: ReadyHand,
        _call: &'a ToolCallIdentity,
    ) -> ToolMuxFuture<'a, Result<(), String>> {
        *self.cancels.lock().expect("cancel mutex") += 1;
        Box::pin(async { Ok(()) })
    }
}

#[derive(Default)]
struct StorageFake {
    calls: Mutex<Vec<ReadyHand>>,
}

impl StoragePersistPort for StorageFake {
    fn start_persist<'a>(
        &'a self,
        ready: ReadyHand,
        _source: &'a GuestPath,
        logical_name: &'a str,
        _media_type: Option<&'a str>,
        _call: &'a ToolCallIdentity,
    ) -> ToolMuxFuture<'a, Result<(), String>> {
        self.calls.lock().expect("storage mutex").push(ready);
        let _ = logical_name;
        Box::pin(async move { Ok(()) })
    }

    fn read_persist<'a>(
        &'a self,
        _ready: ReadyHand,
        _call: &'a ToolCallIdentity,
    ) -> ToolMuxFuture<'a, Result<Option<ExecutorOutput>, String>> {
        Box::pin(async move {
            let output = br#"{"name":"report.pdf","hash":"sha256:09"}"#.to_vec();
            Ok(Some(ExecutorOutput {
                preview: output.clone(),
                full: FullOutput::Inline(output),
                is_error: false,
            }))
        })
    }

    fn cancel_persist<'a>(
        &'a self,
        _ready: ReadyHand,
        _call: &'a ToolCallIdentity,
    ) -> ToolMuxFuture<'a, Result<(), String>> {
        Box::pin(async { Ok(()) })
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
    storage: Arc<StorageFake>,
    telemetry: Arc<TelemetryFake>,
}

fn fixture() -> Fixture {
    let runtime = Arc::new(RuntimeFake::default());
    let guest = Arc::new(GuestFake::default());
    let storage = Arc::new(StorageFake::default());
    let telemetry = Arc::new(TelemetryFake::default());
    let mux = ToolMux::new(
        runtime.clone(),
        guest.clone(),
        storage.clone(),
        telemetry.clone(),
    );
    Fixture {
        mux,
        runtime,
        guest,
        storage,
        telemetry,
    }
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
async fn start_returns_a_detached_handle_before_first_call_preparation_is_ready() {
    let fixture = fixture();
    *fixture.runtime.ready.lock().expect("ready mutex") = false;
    let ToolStart::Accepted { handle } = fixture
        .mux
        .start(&request(
            ToolTarget::OfficialSandbox {
                tool: OfficialSandboxTool::Read,
            },
            true,
        ))
        .await
        .expect("start only schedules preparation")
    else {
        panic!("sandbox call is detached")
    };
    assert_eq!(*fixture.runtime.waits.lock().expect("wait mutex"), 1);
    assert!(fixture.guest.hellos.lock().expect("hello mutex").is_empty());
    let read = aex_tool_mux::ToolHandleRequest {
        identity: identity(),
        handle: handle.clone(),
    };
    assert_eq!(
        fixture.mux.read(&read).await.expect("poll"),
        ToolRead::Pending
    );
    assert!(fixture.guest.hellos.lock().expect("hello mutex").is_empty());
    *fixture.runtime.ready.lock().expect("ready mutex") = true;
    assert!(matches!(
        fixture.mux.read(&read).await.expect("completed poll"),
        ToolRead::Completed { .. }
    ));
}

#[tokio::test]
async fn remote_streamable_mcp_requires_the_same_sandbox_as_every_other_tool() {
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
    let ToolStart::Completed { result } = outcome else {
        panic!("disabled sandbox is terminal")
    };
    assert_eq!(result.error.expect("disabled").code, "sandbox_disabled");
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
        ToolTarget::RemoteMcp {
            server: ResourceName::parse("clock").expect("name"),
            endpoint: "https://mcp.example.test/api".to_owned(),
            headers: std::collections::BTreeMap::new(),
            tool: "now".to_owned(),
        },
    ] {
        let ToolStart::Accepted { handle } = fixture
            .mux
            .start(&request(target, true))
            .await
            .expect("sandbox target")
        else {
            panic!("sandbox tool starts detached")
        };
        fixture
            .mux
            .read(&aex_tool_mux::ToolHandleRequest {
                identity: identity(),
                handle,
            })
            .await
            .expect("sandbox result");
    }
    assert_eq!(*fixture.runtime.waits.lock().expect("wait mutex"), 3);
    let hellos = fixture.guest.hellos.lock().expect("hello mutex");
    assert_eq!(hellos.len(), 3);
    assert!(
        hellos
            .iter()
            .all(|ready| ready.generation == id::<GenerationId>(7))
    );
    assert_eq!(*fixture.runtime.settles.lock().expect("settle mutex"), 3);
}

#[tokio::test]
async fn a_foreign_ready_generation_is_refused_before_guest_dispatch() {
    let fixture = fixture();
    *fixture
        .runtime
        .foreign_generation
        .lock()
        .expect("foreign mutex") = true;
    let ToolStart::Accepted { handle } = fixture
        .mux
        .start(&request(
            ToolTarget::OfficialSandbox {
                tool: OfficialSandboxTool::Read,
            },
            true,
        ))
        .await
        .expect("start")
    else {
        panic!("accepted")
    };
    let error = fixture
        .mux
        .read(&aex_tool_mux::ToolHandleRequest {
            identity: identity(),
            handle,
        })
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
async fn a_large_result_is_a_bounded_preview_plus_a_local_sandbox_file() {
    let fixture = fixture();
    *fixture.guest.large.lock().expect("large mutex") = true;
    let ToolStart::Accepted { handle } = fixture
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
        panic!("sandbox start is detached")
    };
    let ToolRead::Completed { result } = fixture
        .mux
        .read(&aex_tool_mux::ToolHandleRequest {
            identity: identity(),
            handle,
        })
        .await
        .expect("large output poll")
    else {
        panic!("fake completes on poll")
    };
    assert_eq!(result.preview.len(), 65_536);
    assert!(result.truncated);
    let output_file = result.output_file.expect("local full result");
    assert_eq!(output_file.bytes, 12_000_000);
    assert_eq!(output_file.path, "/workspace/.aex/tool-results/call-6.bin");
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
        TelemetryKind::ToolResultPlaced {
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
    let ToolStart::Accepted { handle } = fixture
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
        .expect("persist")
    else {
        panic!("storage persistence starts detached")
    };
    let ToolRead::Completed { result } = fixture
        .mux
        .read(&aex_tool_mux::ToolHandleRequest {
            identity: identity(),
            handle,
        })
        .await
        .expect("persist result")
    else {
        panic!("storage persistence completed")
    };
    assert!(result.error.is_none());
    assert!(result.output_file.is_none());
    assert_eq!(
        fixture.storage.calls.lock().expect("storage mutex")[0].generation,
        id::<GenerationId>(7)
    );
}

#[tokio::test]
async fn readiness_waiter_is_settled_when_guest_startup_fails() {
    let fixture = fixture();
    *fixture.guest.fail_hello.lock().expect("fail hello mutex") = true;
    let ToolStart::Accepted { handle } = fixture
        .mux
        .start(&request(
            ToolTarget::OfficialSandbox {
                tool: OfficialSandboxTool::Read,
            },
            true,
        ))
        .await
        .expect("start")
    else {
        panic!("accepted")
    };
    fixture
        .mux
        .read(&aex_tool_mux::ToolHandleRequest {
            identity: identity(),
            handle,
        })
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
            target: Box::new(ToolTarget::OfficialSandbox {
                tool: OfficialSandboxTool::Read,
            }),
            arguments: serde_json::json!({}),
            deadline_ms: 60_000,
            max_result_bytes: 1_048_576,
            timeout_ms: 60_000,
        },
    };
    fixture.mux.cancel(&request).await.expect("cancel");
    assert_eq!(*fixture.runtime.cancels.lock().expect("cancel mutex"), 1);
    assert_eq!(*fixture.guest.cancels.lock().expect("cancel mutex"), 1);
    assert_eq!(*fixture.runtime.settles.lock().expect("settle mutex"), 1);
}

#[tokio::test]
async fn pre_dispatch_cancellation_never_calls_the_guest() {
    let fixture = fixture();
    *fixture.runtime.ready.lock().expect("ready mutex") = false;
    let request = aex_tool_mux::ToolHandleRequest {
        identity: identity(),
        handle: ToolHandle::Sandbox {
            hand: HandId::for_session(identity().session),
            generation: id::<GenerationId>(7),
            target: Box::new(ToolTarget::OfficialSandbox {
                tool: OfficialSandboxTool::Read,
            }),
            arguments: serde_json::json!({}),
            deadline_ms: 60_000,
            max_result_bytes: 1_048_576,
            timeout_ms: 60_000,
        },
    };
    fixture.mux.cancel(&request).await.expect("cancel");
    assert_eq!(*fixture.runtime.cancels.lock().expect("cancel mutex"), 1);
    assert_eq!(*fixture.guest.cancels.lock().expect("cancel mutex"), 0);
    assert_eq!(*fixture.runtime.settles.lock().expect("settle mutex"), 0);
}
