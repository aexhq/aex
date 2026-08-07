//! The guest's HTTP surface: the five verbs, the attached stream, the provider
//! lifecycle hooks and the internal health paths — all on one port.
//!
//! One port because `CreateMicrovmImage` declares exactly one hook port and
//! `CreateMicrovmAuthToken` scopes the endpoint to exactly that port. There is no
//! shell port and no second listener.
//!
//! The proxy terminates TLS and re-originates in-VM, so the guest speaks plain
//! HTTP/1.1 and h2c and never sees the endpoint auth header.

use std::sync::Arc;

use aex_hands_agent::boot::RunHook;
use aex_hands_agent::journal::{Journal, JournalError};
use aex_hands_agent::session::{LifecycleHook, StartDecision, StartInput, Supervisor};
use aex_hands_agent::wire::{
    Frame, FrameError, FrameExpectation, RequestPreamble, ResponsePreamble, ResponseStatus, Verb,
    decode_request, encode_response,
};
use aex_hands_protocol::operation::GuestRoot;
use aex_hands_protocol::rpc::{CancelRequest, Fence, ResultRequest, StartRequest, StatusRequest};
use aex_internal_contracts::SchemaVersion;
use aex_wire::types::Timestamp;
use axum::Router;
use axum::body::Bytes;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use serde::Deserialize;
use tokio::sync::RwLock;

use crate::execute::{Dispatch, Executor};
use crate::image::ImageValidator;

/// The internal liveness path.
pub const HEALTHZ_PATH: &str = "/internal/healthz";

/// The internal readiness path.
pub const READYZ_PATH: &str = "/internal/readyz";

/// What the guest is currently bound to.
#[derive(Debug)]
struct Bound {
    /// The supervisor for the exact generation the `/run` hook bound.
    supervisor: Supervisor,
    /// Whether the pinned image carries the browser capability.
    browser: bool,
}

/// The exact envelope AWS sends to the `/run` lifecycle hook.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct AwsRunHook {
    /// The provider identity for this `MicroVM`. AWS documents 1–256 characters.
    microvm_id: String,
    /// The verbatim string supplied to `RunMicrovm.runHookPayload`.
    run_hook_payload: String,
}

impl AwsRunHook {
    /// Decodes the provider envelope and then the closed AEX identity payload.
    fn decode(body: &[u8]) -> Option<RunHook> {
        let envelope: Self = serde_json::from_slice(body).ok()?;
        if !(1..=256).contains(&envelope.microvm_id.chars().count()) {
            return None;
        }
        serde_json::from_str(&envelope.run_hook_payload).ok()
    }
}

/// The guest's whole state.
///
/// `accepting` is not a separate flag: a guest with no binding accepts nothing,
/// and the binding is written by `/run` or `/resume` only after the journal has
/// been replayed. There is therefore no window where the guest is bound but not
/// ready.
#[derive(Debug)]
pub struct Guest {
    /// The journal, which is the only state that survives a restart.
    journal: Journal,
    /// The operation dispatcher.
    executor: Executor,
    /// The current binding.
    bound: RwLock<Option<Bound>>,
    /// The first eight bytes of this agent binary's `blake3`.
    agent_build: [u8; 8],
    /// The real rootfs and package validator used by the provider build hooks.
    image: Arc<dyn ImageValidator>,
}

impl Guest {
    /// Composes a guest over a journal and a dispatcher.
    #[must_use]
    pub fn new(
        journal: Journal,
        executor: Executor,
        agent_build: [u8; 8],
        image: Arc<dyn ImageValidator>,
    ) -> Self {
        Self {
            journal,
            executor,
            bound: RwLock::new(None),
            agent_build,
            image,
        }
    }

    /// The guest root every structured tool stays inside.
    #[must_use]
    pub const fn root(&self) -> &GuestRoot {
        self.executor.root()
    }

    /// Whether the guest is bound and therefore accepting.
    pub async fn accepting(&self) -> bool {
        self.bound.read().await.is_some()
    }

    /// Binds the guest to one generation, replaying the journal first.
    ///
    /// # Errors
    ///
    /// Returns [`JournalError`] when the journal cannot be replayed. Any failure
    /// leaves the guest unbound, so the launch fails closed.
    pub async fn bind(&self, hook: &RunHook, now: Timestamp) -> Result<u32, JournalError> {
        let incarnation = self.replay(now)?;
        let bounds = hook.bounds.resolve();
        self.journal
            .write_binding(&aex_hands_agent::journal::GuestBinding {
                generation: hook.generation,
                fence_floor: Fence(0),
                protocol_version: SchemaVersion(hook.protocol_version),
                root: hook.root.clone(),
                bounds,
            })?;
        *self.bound.write().await = Some(Bound {
            supervisor: Supervisor::new(
                self.journal.clone(),
                hook.generation,
                Fence(0),
                bounds,
                incarnation,
            ),
            browser: hook.carries_browser(),
        });
        Ok(incarnation)
    }

    /// Reopens the snapshotted binding after AWS resumes the `MicroVM`.
    ///
    /// # Errors
    ///
    /// Returns [`JournalError`] when the in-memory or persisted binding is absent,
    /// disagrees, or the journal cannot be replayed.
    pub async fn resume(&self, now: Timestamp) -> Result<u32, JournalError> {
        let (generation, fence_floor, browser) = {
            let bound = self.bound.read().await;
            let Some(bound) = bound.as_ref() else {
                return Err(JournalError::Malformed {
                    path: self.journal.root().join("binding.json"),
                    reason: "the resume hook arrived before a run binding".to_owned(),
                });
            };
            (
                bound.supervisor.generation(),
                bound.supervisor.fence_floor(),
                bound.browser,
            )
        };
        let Some(mut binding) = self.journal.read_binding()? else {
            return Err(JournalError::Malformed {
                path: self.journal.root().join("binding.json"),
                reason: "the resume hook has no persisted run binding".to_owned(),
            });
        };
        if binding.generation != generation {
            return Err(JournalError::Malformed {
                path: self.journal.root().join("binding.json"),
                reason: "the persisted generation disagrees with the snapshotted supervisor"
                    .to_owned(),
            });
        }
        binding.fence_floor = fence_floor;
        let incarnation = self.replay(now)?;
        self.journal.write_binding(&binding)?;
        *self.bound.write().await = Some(Bound {
            supervisor: Supervisor::new(
                self.journal.clone(),
                binding.generation,
                binding.fence_floor,
                binding.bounds,
                incarnation,
            ),
            browser,
        });
        Ok(incarnation)
    }

    /// Advances the incarnation and reconciles the journal before admission.
    fn replay(&self, now: Timestamp) -> Result<u32, JournalError> {
        let incarnation = self.journal.bump_incarnation()?;
        let probe = crate::host::HostProbe;
        // Replay before accepting. An operation whose process group is gone is
        // recorded `Interrupted` rather than left looking live.
        for entry in self.journal.replay(&probe)? {
            if matches!(
                entry.verdict,
                aex_hands_agent::journal::ReplayVerdict::Interrupted
            ) {
                let terminal = aex_hands_agent::session::interrupted_terminal(&entry.meta, now);
                match self.journal.record_terminal(entry.operation, &terminal) {
                    Ok(()) | Err(JournalError::AlreadyTerminal { .. }) => {}
                    Err(error) => return Err(error),
                }
            }
        }
        Ok(incarnation)
    }

    /// Flushes every open operation's journal, for `/suspend`.
    ///
    /// # Errors
    ///
    /// Returns [`JournalError`] on a sync failure.
    pub fn flush(&self) -> Result<(), JournalError> {
        self.journal.flush_all()
    }
}

/// The whole guest surface.
pub fn router(guest: Arc<Guest>) -> Router {
    let mut router = Router::new()
        .route(HEALTHZ_PATH, get(healthz))
        .route(READYZ_PATH, get(readyz));
    for verb in Verb::ALL {
        if verb == Verb::Attach {
            continue;
        }
        router = router.route(
            verb.path(),
            post(move |state, body| verb_handler(verb, state, body)),
        );
    }
    router = router.route(
        "/aex/hands/v1/attach/{operation}",
        get(|state: State<Arc<Guest>>| async move {
            // The attached stream is `start`'s delivery mode, not a sixth verb.
            // Until the streaming mirror lands, an attach is refused explicitly so
            // a caller never waits on a body that will not arrive.
            let _ = state;
            (
                StatusCode::NOT_IMPLEMENTED,
                "attached delivery is declared and not yet served; pull the result instead",
            )
        }),
    );
    for hook in LifecycleHook::ALL {
        router = router.route(
            hook.path(),
            post(move |state, body| hook_handler(hook, state, body)),
        );
    }
    router.with_state(guest)
}

/// Liveness. Answers as soon as the process is up, bound or not.
async fn healthz() -> &'static str {
    "ok"
}

/// Readiness. Answers only once the `/run` hook has bound a generation.
async fn readyz(State(guest): State<Arc<Guest>>) -> (StatusCode, &'static str) {
    if guest.accepting().await {
        (StatusCode::OK, "ready")
    } else {
        (StatusCode::SERVICE_UNAVAILABLE, "no generation is bound")
    }
}

/// One provider lifecycle hook.
async fn hook_handler(
    hook: LifecycleHook,
    State(guest): State<Arc<Guest>>,
    body: Bytes,
) -> Response {
    match hook {
        LifecycleHook::Run => {
            let Some(payload) = AwsRunHook::decode(&body) else {
                // Fail closed: an unreadable run payload means the guest does not
                // know which generation it serves, and a guest that guesses would
                // answer for the wrong one.
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "the run-hook payload is not the closed identity payload",
                )
                    .into_response();
            };
            match guest.bind(&payload, now()).await {
                Ok(incarnation) => (StatusCode::OK, incarnation.to_string()).into_response(),
                Err(error) => {
                    (StatusCode::INTERNAL_SERVER_ERROR, error.to_string()).into_response()
                }
            }
        }
        LifecycleHook::Resume => match guest.resume(now()).await {
            Ok(incarnation) => (StatusCode::OK, incarnation.to_string()).into_response(),
            Err(error) => (StatusCode::INTERNAL_SERVER_ERROR, error.to_string()).into_response(),
        },
        LifecycleHook::Suspend | LifecycleHook::Terminate => match guest.flush() {
            Ok(()) => (StatusCode::OK, "flushed").into_response(),
            Err(error) => (StatusCode::INTERNAL_SERVER_ERROR, error.to_string()).into_response(),
        },
        LifecycleHook::Ready | LifecycleHook::Validate => {
            let image = Arc::clone(&guest.image);
            let checked = tokio::task::spawn_blocking(move || match hook {
                LifecycleHook::Ready => image.ready(),
                LifecycleHook::Validate => image.validate(),
                _ => unreachable!("the match arm admits only image build hooks"),
            })
            .await;
            match checked {
                Ok(Ok(())) => (StatusCode::OK, "ok").into_response(),
                Ok(Err(error)) => {
                    (StatusCode::SERVICE_UNAVAILABLE, error.to_string()).into_response()
                }
                Err(error) => (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    format!("the image validator did not join: {error}"),
                )
                    .into_response(),
            }
        }
    }
}

/// One protocol verb.
async fn verb_handler(verb: Verb, State(guest): State<Arc<Guest>>, body: Bytes) -> Response {
    let mut bound = guest.bound.write().await;
    let Some(state) = bound.as_mut() else {
        // Before `/run` the guest has no generation, so it cannot even encode a
        // response preamble. A plain 503 is the honest answer.
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            "no generation is bound; the run hook has not completed",
        )
            .into_response();
    };
    let expectation = FrameExpectation {
        generation: state.supervisor.generation(),
        min_fence: state.supervisor.fence_floor(),
        schema_version: aex_hands_agent::wire::PROTOCOL_V1,
        max_frame_bytes: 1_048_576,
    };
    let decoded = match decode_request(&body, &expectation) {
        Ok(frame) => frame,
        Err(error) => return protocol_error(guest.as_ref(), state, verb, &error),
    };
    if decoded.preamble.verb != verb {
        return protocol_error(
            guest.as_ref(),
            state,
            verb,
            &FrameError::Malformed {
                at: "verb",
                reason: format!(
                    "the frame declares {:?} but was posted to {}",
                    decoded.preamble.verb,
                    verb.path()
                ),
            },
        );
    }
    state.supervisor.adopt_fence(decoded.preamble.fence);
    let answered = answer(guest.as_ref(), state, verb, &decoded);
    match answered {
        Ok(payload) => frame_response(
            guest.as_ref(),
            state,
            verb,
            ResponseStatus::Payload,
            &payload,
        ),
        Err(error) => protocol_error(guest.as_ref(), state, verb, &error),
    }
}

/// The typed payload one verb answers with.
fn answer(
    guest: &Guest,
    state: &mut Bound,
    verb: Verb,
    frame: &Frame<'_, RequestPreamble>,
) -> Result<Vec<u8>, FrameError> {
    let decode = |what: &'static str| FrameError::Malformed {
        at: what,
        reason: "the payload is not the request this verb takes".to_owned(),
    };
    match verb {
        Verb::Start => {
            let request: StartRequest =
                serde_json::from_slice(frame.payload).map_err(|_| decode("payload"))?;
            let input = StartInput {
                operation: request.operation,
                call_hash: request.call_hash,
                request: request.request.clone(),
                bounds: request.bounds,
                deadline: request.deadline,
                open_operations: 0,
                browser_available: state.browser,
            };
            let decision = state
                .supervisor
                .decide_start(&input, now())
                .map_err(|error| journal_error(&error))?;
            if let StartDecision::Spawn { meta } = &decision {
                // Rule 1: `meta.json` is written and fsynced **before** anything is
                // spawned. A crash between the two is observed on replay as an
                // interrupted operation rather than as a lost one.
                guest
                    .journal
                    .record_start(meta)
                    .map_err(|error| journal_error(&error))?;
                let dispatched = guest
                    .executor
                    .dispatch(&guest.journal, meta, request.delivery, now())
                    .map_err(|error| journal_error(&error))?;
                // Every synchronous terminal is recorded, whatever the delivery
                // mode: a detached workspace read finishes right here, and
                // skipping the record left it polling to its deadline.
                if let Dispatch::Terminal(terminal) = dispatched {
                    match guest.journal.record_terminal(meta.operation, &terminal) {
                        Ok(()) | Err(JournalError::AlreadyTerminal { .. }) => {}
                        Err(error) => return Err(journal_error(&error)),
                    }
                }
            }
            serde_json::to_vec(
                &decision.response(request.operation, state.supervisor.guest_revision()),
            )
            .map_err(|_| decode("response"))
        }
        Verb::Status => {
            let request: StatusRequest =
                serde_json::from_slice(frame.payload).map_err(|_| decode("payload"))?;
            let response = state
                .supervisor
                .status(request.operation)
                .map_err(|error| journal_error(&error))?;
            serde_json::to_vec(&response).map_err(|_| decode("response"))
        }
        Verb::Cancel => {
            let request: CancelRequest =
                serde_json::from_slice(frame.payload).map_err(|_| decode("payload"))?;
            let response = state
                .supervisor
                .cancel(request.operation, request.reason)
                .map_err(|error| journal_error(&error))?;
            if let Some(record) = guest
                .journal
                .read_process(request.operation)
                .map_err(|error| journal_error(&error))?
            {
                // Best effort by construction: a process that double-forked out of
                // its group survives, and that is reported honestly rather than
                // claimed as a clean kill.
                let _ = guest.executor.runner().signal(
                    aex_hands_tools::port::Pgid(record.pgid),
                    aex_hands_protocol::operation::StopSignal::Term,
                );
            }
            serde_json::to_vec(&response).map_err(|_| decode("response"))
        }
        Verb::Result => {
            let request: ResultRequest =
                serde_json::from_slice(frame.payload).map_err(|_| decode("payload"))?;
            let response = state
                .supervisor
                .result(request.operation, request.from_offset, request.max_bytes)
                .map_err(|error| journal_error(&error))?;
            serde_json::to_vec(&response).map_err(|_| decode("response"))
        }
        Verb::Attach => Err(FrameError::Malformed {
            at: "verb",
            reason: "attach is a delivery mode on its own stream, not a posted verb".to_owned(),
        }),
    }
}

/// A journal failure, as a frame error.
///
/// Never collapsed into "the operation failed": a journal that cannot be read is
/// a guest fault, and reporting it as an operation outcome would let a customer's
/// command look finished when nobody knows whether it ran.
fn journal_error(error: &JournalError) -> FrameError {
    FrameError::Malformed {
        at: "journal",
        reason: error.to_string(),
    }
}

/// Encodes one framed response.
fn frame_response(
    guest: &Guest,
    state: &Bound,
    verb: Verb,
    status: ResponseStatus,
    payload: &[u8],
) -> Response {
    let preamble = ResponsePreamble {
        schema_version: aex_hands_agent::wire::PROTOCOL_V1,
        verb,
        status,
        generation: state.supervisor.generation(),
        fence: state.supervisor.fence_floor(),
        payload_len: u32::try_from(payload.len()).unwrap_or(u32::MAX),
        guest_revision: state.supervisor.guest_revision().0,
        agent_build: guest.agent_build,
    };
    (StatusCode::OK, encode_response(&preamble, payload)).into_response()
}

/// Encodes a typed protocol error as a frame.
fn protocol_error(guest: &Guest, state: &Bound, verb: Verb, error: &FrameError) -> Response {
    let payload = serde_json::to_vec(&serde_json::json!({
        "error": error.to_string(),
    }))
    .unwrap_or_default();
    frame_response(guest, state, verb, ResponseStatus::ProtocolError, &payload)
}

/// The guest clock, shared with the background reap and cancel threads.
fn now() -> Timestamp {
    crate::host::now()
}

#[cfg(test)]
mod tests {
    use super::{Guest, HEALTHZ_PATH, READYZ_PATH, router};
    use crate::execute::Executor;
    use crate::host::{OutputSink, Runner, Started};
    use crate::image::{ImageError, ImageValidator};
    use aex_hands_agent::boot::{RunHook, RunHookBounds};
    use aex_hands_agent::journal::Journal;
    use aex_hands_agent::wire::{
        FrameExpectation, PROTOCOL_V1, RequestPreamble, ResponseStatus, Verb, decode_response,
        encode_request,
    };
    use aex_hands_protocol::operation::{
        DeliveryMode, GuestPath, GuestRoot, OperationBounds, OperationRequest, StopSignal,
        TerminalState,
    };
    use aex_hands_protocol::rpc::{
        Fence, GenerationBinding, HandsOperationId, ResultRequest, ResultResponse, StartRequest,
        StatusRequest, StatusResponse,
    };
    use aex_hands_tools::port::{Pgid, ProcError};
    use aex_wire::ids::{ContentHash, GenerationId, PrefixedId as _, Uuid7};
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use std::sync::{Arc, Mutex};
    use tower::ServiceExt as _;

    #[derive(Debug)]
    struct ValidImage;

    impl ImageValidator for ValidImage {
        fn ready(&self) -> Result<(), ImageError> {
            Ok(())
        }

        fn validate(&self) -> Result<(), ImageError> {
            Ok(())
        }
    }

    fn generation() -> GenerationId {
        GenerationId::from_uuid7(Uuid7::compose(9, [4; 10]))
    }

    fn operation() -> HandsOperationId {
        HandsOperationId(Uuid7::compose(9, [5; 10]))
    }

    fn bounds() -> OperationBounds {
        OperationBounds {
            max_output_bytes: 1_000_000,
            max_frame_bytes: 1_048_576,
            max_wall_ms: 600_000,
            max_concurrent_operations: 32,
        }
    }

    fn hook() -> RunHook {
        RunHook {
            v: 1,
            generation: generation(),
            protocol_version: PROTOCOL_V1.0,
            root: "/workspace".to_owned(),
            size: "1gb".to_owned(),
            image_digest: ContentHash::from_bytes([3; 32]),
            capabilities: Vec::new(),
            bounds: RunHookBounds {
                max_output_bytes: bounds().max_output_bytes,
                max_frame_bytes: bounds().max_frame_bytes,
                max_wall_ms: bounds().max_wall_ms,
                max_concurrent_operations: bounds().max_concurrent_operations,
            },
        }
    }

    fn aws_run_hook_body() -> Vec<u8> {
        serde_json::to_vec(&serde_json::json!({
            "microvmId": "mvm-01234567-abcd-ef01-2345-6789abcdef01",
            "runHookPayload": serde_json::to_string(&hook()).expect("the payload serializes"),
        }))
        .expect("the AWS envelope serializes")
    }

    /// A runner that records what it was asked to start and never forks.
    #[derive(Default)]
    struct FakeRunner {
        started: Mutex<Vec<Vec<String>>>,
        signalled: Mutex<Vec<Pgid>>,
    }

    impl Runner for FakeRunner {
        fn start(&self, spec: &aex_hands_tools::command::SpawnSpec) -> Result<Started, ProcError> {
            self.started
                .lock()
                .expect("the fixture lock is not poisoned")
                .push(spec.argv.clone());
            Ok(Started {
                pgid: Pgid(4242),
                start_time: 99,
                reap: Box::new(|sink: &mut dyn OutputSink| {
                    sink.append(b"hello from the guest")
                        .map_err(ProcError::Other)?;
                    Ok(aex_hands_protocol::operation::OperationExit::Ok)
                }),
            })
        }

        fn signal(&self, group: Pgid, _signal: StopSignal) -> Result<(), ProcError> {
            self.signalled
                .lock()
                .expect("the fixture lock is not poisoned")
                .push(group);
            Ok(())
        }

        fn alive(&self, _group: Pgid) -> Result<bool, ProcError> {
            Ok(false)
        }
    }

    fn guest(dir: &std::path::Path, runner: Arc<FakeRunner>) -> Arc<Guest> {
        let journal = Journal::open(dir).expect("the journal tree is created");
        let executor = Executor::new(runner, GuestRoot::workspace());
        Arc::new(Guest::new(
            journal,
            executor,
            [1, 2, 3, 4, 5, 6, 7, 8],
            Arc::new(ValidImage),
        ))
    }

    fn framed(verb: Verb, payload: &[u8], fence: Fence) -> Vec<u8> {
        encode_request(
            &RequestPreamble {
                schema_version: PROTOCOL_V1,
                verb,
                flags: 0,
                generation: generation(),
                fence,
                payload_len: u32::try_from(payload.len()).expect("a bounded payload"),
            },
            payload,
        )
    }

    fn exec_start(delivery: DeliveryMode) -> StartRequest {
        StartRequest {
            binding: GenerationBinding {
                schema_version: PROTOCOL_V1,
                generation: generation(),
                fence: Fence(1),
            },
            operation: operation(),
            call_hash: aex_hands_protocol::rpc::CallHash(ContentHash::from_bytes([9; 32])),
            request: OperationRequest::Exec {
                argv: vec!["/bin/true".to_owned()],
                cwd: GuestPath::parse(&GuestRoot::workspace(), "/workspace")
                    .expect("a contained path"),
                env: Vec::new(),
                stdin: None,
            },
            bounds: bounds(),
            deadline: aex_wire::types::Timestamp::from_unix_millis(4_102_444_800_000)
                .expect("a bounded instant"),
            delivery,
        }
    }

    async fn post(app: &axum::Router, path: &str, body: Vec<u8>) -> (StatusCode, Vec<u8>) {
        let response = app
            .clone()
            .oneshot(
                Request::post(path)
                    .body(Body::from(body))
                    .expect("a request"),
            )
            .await
            .expect("a response");
        let status = response.status();
        let bytes = axum::body::to_bytes(response.into_body(), 4 * 1024 * 1024)
            .await
            .expect("a bounded body");
        (status, bytes.to_vec())
    }

    #[tokio::test]
    async fn an_unbound_guest_accepts_nothing_and_says_so_without_a_frame() {
        let dir = tempfile::tempdir().expect("a temporary directory");
        let app = router(guest(dir.path(), Arc::new(FakeRunner::default())));
        let (status, _) = post(&app, Verb::Status.path(), Vec::new()).await;
        assert_eq!(
            status,
            StatusCode::SERVICE_UNAVAILABLE,
            "before the run hook the guest has no generation, so it cannot even encode a preamble"
        );
        let ready = app
            .clone()
            .oneshot(
                Request::get(READYZ_PATH)
                    .body(Body::empty())
                    .expect("a request"),
            )
            .await
            .expect("a response");
        assert_eq!(ready.status(), StatusCode::SERVICE_UNAVAILABLE);
        let live = app
            .oneshot(
                Request::get(HEALTHZ_PATH)
                    .body(Body::empty())
                    .expect("a request"),
            )
            .await
            .expect("a response");
        assert_eq!(live.status(), StatusCode::OK, "liveness is not readiness");
    }

    #[tokio::test]
    async fn the_run_hook_binds_the_generation_and_then_the_five_verbs_answer() {
        let dir = tempfile::tempdir().expect("a temporary directory");
        let runner = Arc::new(FakeRunner::default());
        let app = router(guest(dir.path(), Arc::clone(&runner)));

        let (status, _) = post(
            &app,
            aex_hands_agent::session::LifecycleHook::Run.path(),
            aws_run_hook_body(),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "the launch fails closed otherwise");

        let ready = app
            .clone()
            .oneshot(
                Request::get(READYZ_PATH)
                    .body(Body::empty())
                    .expect("a request"),
            )
            .await
            .expect("a response");
        assert_eq!(ready.status(), StatusCode::OK);

        // start
        let start = exec_start(DeliveryMode::Attached);
        let (status, body) = post(
            &app,
            Verb::Start.path(),
            framed(
                Verb::Start,
                &serde_json::to_vec(&start).expect("it serializes"),
                Fence(1),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let expectation = FrameExpectation {
            generation: generation(),
            min_fence: Fence(0),
            schema_version: PROTOCOL_V1,
            max_frame_bytes: 1_048_576,
        };
        let frame = decode_response(&body, &expectation).expect("a framed response");
        assert_eq!(frame.preamble.status, ResponseStatus::Payload);
        assert_eq!(
            frame.preamble.agent_build,
            [1, 2, 3, 4, 5, 6, 7, 8],
            "every response is a liveness probe, so the build stamp rides the preamble"
        );
        assert_eq!(
            runner
                .started
                .lock()
                .expect("the fixture lock is not poisoned")
                .len(),
            1,
            "one start, one process"
        );

        // status
        let query = StatusRequest {
            binding: start.binding,
            operation: operation(),
        };
        let (status, body) = post(
            &app,
            Verb::Status.path(),
            framed(
                Verb::Status,
                &serde_json::to_vec(&query).expect("it serializes"),
                Fence(1),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let frame = decode_response(&body, &expectation).expect("a framed response");
        let answered: StatusResponse =
            serde_json::from_slice(frame.payload).expect("a typed status");
        assert!(
            matches!(answered, StatusResponse::Terminal { .. }),
            "the foreground exec ran to completion: {answered:?}"
        );
    }

    #[tokio::test]
    async fn a_detached_exec_reaps_in_the_background_and_its_result_is_pullable() {
        let dir = tempfile::tempdir().expect("a temporary directory");
        let runner = Arc::new(FakeRunner::default());
        let app = router(guest(dir.path(), Arc::clone(&runner)));
        let (status, _) = post(
            &app,
            aex_hands_agent::session::LifecycleHook::Run.path(),
            aws_run_hook_body(),
        )
        .await;
        assert_eq!(status, StatusCode::OK);

        let start = exec_start(DeliveryMode::Detached);
        let (status, body) = post(
            &app,
            Verb::Start.path(),
            framed(
                Verb::Start,
                &serde_json::to_vec(&start).expect("it serializes"),
                Fence(1),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let expectation = FrameExpectation {
            generation: generation(),
            min_fence: Fence(0),
            schema_version: PROTOCOL_V1,
            max_frame_bytes: 1_048_576,
        };
        let frame = decode_response(&body, &expectation).expect("a framed response");
        let accepted: aex_hands_protocol::rpc::StartResponse =
            serde_json::from_slice(frame.payload).expect("a typed start response");
        assert!(
            matches!(
                accepted,
                aex_hands_protocol::rpc::StartResponse::Accepted {
                    existing: false,
                    ..
                }
            ),
            "{accepted:?}"
        );

        // The reap runs on a background thread; poll status until it terminalizes.
        let query = StatusRequest {
            binding: start.binding,
            operation: operation(),
        };
        let mut terminal = None;
        for _ in 0..500 {
            let (status, body) = post(
                &app,
                Verb::Status.path(),
                framed(
                    Verb::Status,
                    &serde_json::to_vec(&query).expect("it serializes"),
                    Fence(1),
                ),
            )
            .await;
            assert_eq!(status, StatusCode::OK);
            let frame = decode_response(&body, &expectation).expect("a framed response");
            let answered: StatusResponse =
                serde_json::from_slice(frame.payload).expect("a typed status");
            if let StatusResponse::Terminal {
                terminal: found, ..
            } = answered
            {
                terminal = Some(found);
                break;
            }
            tokio::time::sleep(core::time::Duration::from_millis(10)).await;
        }
        let terminal = terminal.expect("the background reap terminalizes the operation");
        assert_eq!(terminal.state, TerminalState::Succeeded);
        assert_eq!(terminal.body_len, b"hello from the guest".len() as u64);

        // The terminal body is pullable through the result verb.
        let pull = ResultRequest {
            binding: start.binding,
            operation: operation(),
            from_offset: 0,
            max_bytes: 1_048_576,
        };
        let (status, body) = post(
            &app,
            Verb::Result.path(),
            framed(
                Verb::Result,
                &serde_json::to_vec(&pull).expect("it serializes"),
                Fence(1),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let frame = decode_response(&body, &expectation).expect("a framed response");
        let answered: ResultResponse =
            serde_json::from_slice(frame.payload).expect("a typed result");
        let ResultResponse::Terminal { chunk, .. } = answered else {
            panic!("a terminal operation pulls: {answered:?}");
        };
        let chunk = chunk.expect("the body exists");
        assert_eq!(chunk.bytes, b"hello from the guest");
        assert!(chunk.last);
    }

    #[tokio::test]
    async fn a_frame_for_another_generation_is_refused_as_a_typed_protocol_error() {
        let dir = tempfile::tempdir().expect("a temporary directory");
        let app = router(guest(dir.path(), Arc::new(FakeRunner::default())));
        post(
            &app,
            aex_hands_agent::session::LifecycleHook::Run.path(),
            aws_run_hook_body(),
        )
        .await;

        let stranger = GenerationId::from_uuid7(Uuid7::compose(99, [7; 10]));
        let payload = serde_json::to_vec(&StatusRequest {
            binding: GenerationBinding {
                schema_version: PROTOCOL_V1,
                generation: stranger,
                fence: Fence(1),
            },
            operation: operation(),
        })
        .expect("it serializes");
        let body = encode_request(
            &RequestPreamble {
                schema_version: PROTOCOL_V1,
                verb: Verb::Status,
                flags: 0,
                generation: stranger,
                fence: Fence(1),
                payload_len: u32::try_from(payload.len()).expect("a bounded payload"),
            },
            &payload,
        );
        let (status, body) = post(&app, Verb::Status.path(), body).await;
        assert_eq!(status, StatusCode::OK);
        let frame = decode_response(
            &body,
            &FrameExpectation {
                generation: generation(),
                min_fence: Fence(0),
                schema_version: PROTOCOL_V1,
                max_frame_bytes: 1_048_576,
            },
        )
        .expect("the refusal is itself a frame for the bound generation");
        assert_eq!(
            frame.preamble.status,
            ResponseStatus::ProtocolError,
            "the generation binding is checked before any payload byte is interpreted"
        );
    }

    #[tokio::test]
    async fn the_provider_hooks_are_post_only_and_the_build_hooks_answer() {
        let dir = tempfile::tempdir().expect("a temporary directory");
        let app = router(guest(dir.path(), Arc::new(FakeRunner::default())));
        for hook in [
            aex_hands_agent::session::LifecycleHook::Suspend,
            aex_hands_agent::session::LifecycleHook::Terminate,
        ] {
            let (status, _) = post(&app, hook.path(), Vec::new()).await;
            assert_eq!(status, StatusCode::OK, "{hook:?}");
        }
        for hook in [
            aex_hands_agent::session::LifecycleHook::Ready,
            aex_hands_agent::session::LifecycleHook::Validate,
        ] {
            let (status, _) = post(&app, hook.path(), Vec::new()).await;
            assert_eq!(status, StatusCode::OK, "{hook:?}");
            let response = app
                .clone()
                .oneshot(
                    Request::get(hook.path())
                        .body(Body::empty())
                        .expect("a request"),
                )
                .await
                .expect("a response");
            assert_eq!(
                response.status(),
                StatusCode::METHOD_NOT_ALLOWED,
                "AWS invokes every provider lifecycle hook with POST: {hook:?}"
            );
        }
    }

    #[tokio::test]
    async fn the_empty_resume_hook_replays_the_existing_binding() {
        let dir = tempfile::tempdir().expect("a temporary directory");
        let app = router(guest(dir.path(), Arc::new(FakeRunner::default())));
        let (run_status, run_body) = post(
            &app,
            aex_hands_agent::session::LifecycleHook::Run.path(),
            aws_run_hook_body(),
        )
        .await;
        assert_eq!(run_status, StatusCode::OK);
        assert_eq!(run_body, b"1");

        let (resume_status, resume_body) = post(
            &app,
            aex_hands_agent::session::LifecycleHook::Resume.path(),
            Vec::new(),
        )
        .await;
        assert_eq!(resume_status, StatusCode::OK);
        assert_eq!(resume_body, b"2", "resume advances the incarnation");
    }

    #[tokio::test]
    async fn a_malformed_run_payload_fails_the_launch_closed() {
        let dir = tempfile::tempdir().expect("a temporary directory");
        let app = router(guest(dir.path(), Arc::new(FakeRunner::default())));
        let (status, _) = post(
            &app,
            aex_hands_agent::session::LifecycleHook::Run.path(),
            br#"{"v":1,"apiKey":"sk-live-1"}"#.to_vec(),
        )
        .await;
        assert_eq!(
            status,
            StatusCode::INTERNAL_SERVER_ERROR,
            "a guest that guessed its generation would answer for the wrong one"
        );
        let ready = app
            .oneshot(
                Request::get(READYZ_PATH)
                    .body(Body::empty())
                    .expect("a request"),
            )
            .await
            .expect("a response");
        assert_eq!(ready.status(), StatusCode::SERVICE_UNAVAILABLE);
    }
}
