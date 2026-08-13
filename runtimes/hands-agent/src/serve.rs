//! The guest's HTTP surface: the six verbs, the attached stream, the provider
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
use aex_hands_agent::journal::{Journal, JournalError, OperationMeta};
use aex_hands_agent::session::{LifecycleHook, StartDecision, StartInput, Supervisor};
use aex_hands_protocol::operation::{DeliveryMode, GuestRoot};
use aex_hands_protocol::rpc::{
    AttachResponse, CancelRequest, Fence, GenerationBinding, GuestRequest, GuestResponse,
    HandsOperationId, MAX_GUEST_BODY_BYTES, MAX_RESULT_CHUNK_BYTES, PROTOCOL_V1, ResultRequest,
    ResultResponse, StartRequest, StatusRequest, StatusResponse, Verb,
};
use aex_internal_contracts::SchemaVersion;
use aex_wire::types::Timestamp;
use axum::Router;
use axum::body::Bytes;
use axum::extract::{DefaultBodyLimit, State};
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use tokio::sync::RwLock;

use crate::execute::{Dispatch, Executor};
use crate::image::ImageValidator;

mod attached;

use attached::attach_handler;

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

/// A bounded request or guest-local operation failure rendered over HTTP.
#[derive(Debug)]
struct GuestError {
    status: StatusCode,
    message: String,
}

impl GuestError {
    fn invalid(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::BAD_REQUEST,
            message: message.into(),
        }
    }

    fn stale(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::CONFLICT,
            message: message.into(),
        }
    }

    fn internal(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            message: message.into(),
        }
    }
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
    /// Generation-local binary file transfers.
    files: crate::file::FileService,
    /// The current binding.
    bound: RwLock<Option<Bound>>,
    /// The real rootfs and package validator used by the provider build hooks.
    image: Arc<dyn ImageValidator>,
}

impl Guest {
    /// Composes a guest over a journal and a dispatcher.
    #[must_use]
    pub fn new(
        journal: Journal,
        executor: Executor,
        files: crate::file::FileService,
        image: Arc<dyn ImageValidator>,
    ) -> Self {
        Self {
            journal,
            executor,
            files,
            bound: RwLock::new(None),
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
    // Attach is `start`'s attached delivery mode, expressed as its own path only
    // because HTTP cannot carry two response bodies. It is posted, bounded and
    // fenced exactly like the other verbs; what differs is that the caller
    // keeps the connection and the terminal record comes back on it.
    router = router.route(Verb::Attach.path(), post(attach_handler));
    for hook in LifecycleHook::ALL {
        router = router.route(
            hook.path(),
            post(move |state, body| hook_handler(hook, state, body)),
        );
    }
    router
        .layer(DefaultBodyLimit::max(MAX_GUEST_BODY_BYTES))
        .with_state(guest)
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
        // Before `/run` the guest has no generation binding. A plain 503 is the
        // honest answer.
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            "no generation is bound; the run hook has not completed",
        )
            .into_response();
    };
    let answered = answer(guest.as_ref(), state, verb, &body);
    match answered {
        Ok(payload) => json_bytes_response(payload),
        Err(error) => protocol_error(&error),
    }
}

/// Decodes one already-bounded JSON request and checks its cooperative freshness
/// binding before any operation is dispatched.
fn decode_guest_request<T: DeserializeOwned>(
    body: &[u8],
    state: &mut Bound,
) -> Result<T, GuestError> {
    let envelope: GuestRequest<T> = serde_json::from_slice(body)
        .map_err(|_| GuestError::invalid("the body is not the request this route takes"))?;
    if envelope.binding.schema_version != PROTOCOL_V1 {
        return Err(GuestError::invalid(format!(
            "unsupported protocol version {}",
            envelope.binding.schema_version.0
        )));
    }
    let expected_generation = state.supervisor.generation();
    if envelope.binding.generation != expected_generation {
        return Err(GuestError::stale(format!(
            "request generation {} does not match {}",
            envelope.binding.generation, expected_generation
        )));
    }
    let fence_floor = state.supervisor.fence_floor();
    if envelope.binding.fence < fence_floor {
        return Err(GuestError::stale(format!(
            "request fence {} is older than {}",
            envelope.binding.fence.0, fence_floor.0
        )));
    }
    state.supervisor.adopt_fence(envelope.binding.fence);
    Ok(envelope.request)
}

fn encode_json<T: Serialize>(value: &T) -> Result<Vec<u8>, GuestError> {
    let payload = serde_json::to_vec(value)
        .map_err(|_| GuestError::internal("the guest response could not be encoded"))?;
    if payload.len() > MAX_GUEST_BODY_BYTES {
        return Err(GuestError::internal(
            "the guest response exceeds the JSON body ceiling",
        ));
    }
    Ok(payload)
}

/// Encodes one typed response with the binding observed after dispatch.
fn encode_guest_response<T: Serialize>(value: &T, state: &Bound) -> Result<Vec<u8>, GuestError> {
    encode_json(&GuestResponse {
        binding: GenerationBinding {
            schema_version: PROTOCOL_V1,
            generation: state.supervisor.generation(),
            fence: state.supervisor.fence_floor(),
        },
        response: value,
    })
}

/// The typed payload one verb answers with.
fn answer(
    guest: &Guest,
    state: &mut Bound,
    verb: Verb,
    body: &[u8],
) -> Result<Vec<u8>, GuestError> {
    match verb {
        Verb::Start => {
            let request: StartRequest = decode_guest_request(body, state)?;
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
            encode_guest_response(&decision.response(request.operation), state)
        }
        Verb::Status => {
            let request: StatusRequest = decode_guest_request(body, state)?;
            let response = state
                .supervisor
                .status(request.operation)
                .map_err(|error| journal_error(&error))?;
            encode_guest_response(&response, state)
        }
        Verb::Cancel => {
            let request: CancelRequest = decode_guest_request(body, state)?;
            let response = state
                .supervisor
                .cancel(request.operation, request.reason)
                .map_err(|error| journal_error(&error))?;
            if matches!(
                response,
                aex_hands_protocol::rpc::CancelResponse::Cancelling { .. }
            ) && let (Some(record), Some(meta)) = (
                guest
                    .journal
                    .read_process(request.operation)
                    .map_err(|error| journal_error(&error))?,
                guest
                    .journal
                    .read_meta(request.operation)
                    .map_err(|error| journal_error(&error))?,
            ) {
                // The marker precedes the first signal, so a reap thread that
                // observes the kill also observes why, and records `Cancelled`
                // rather than an ordinary failure.
                guest
                    .journal
                    .record_cancel(request.operation, request.reason)
                    .map_err(|error| journal_error(&error))?;
                spawn_cancel_driver(guest, meta, record.pgid)?;
            }
            encode_guest_response(&response, state)
        }
        Verb::Result => {
            let request: ResultRequest = decode_guest_request(body, state)?;
            if request.max_bytes == 0 || request.max_bytes > MAX_RESULT_CHUNK_BYTES {
                return Err(GuestError::invalid(
                    "result maxBytes is outside the bounded response window",
                ));
            }
            let response = state
                .supervisor
                .result(request.operation, request.from_offset, request.max_bytes)
                .map_err(|error| journal_error(&error))?;
            encode_guest_response(&response, state)
        }
        Verb::Attach => Err(GuestError::invalid(
            "attach is a delivery mode on its own route",
        )),
        Verb::File => {
            let request: aex_hands_protocol::files::FileRequest =
                decode_guest_request(body, state)?;
            encode_guest_response(&guest.files.answer(request), state)
        }
    }
}

/// Hands the cancel ladder to a background thread.
///
/// The verb answers `Cancelling` immediately; the thread walks
/// `TERM → grace → KILL → reap → Cancelled` against the real clock. A driver
/// that cannot even start is a refused cancel, not a silently polite one: the
/// error surfaces so Brain retries rather than waiting on an escalation that
/// will never happen.
fn spawn_cancel_driver(guest: &Guest, meta: OperationMeta, pgid: i32) -> Result<(), GuestError> {
    let runner = Arc::clone(guest.executor.runner());
    let journal = guest.journal.clone();
    std::thread::Builder::new()
        .name(format!("cancel-{}", meta.operation.0))
        .spawn(move || {
            let started = std::time::Instant::now();
            let mut pace = |millis: u64| {
                std::thread::sleep(core::time::Duration::from_millis(millis));
                u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX)
            };
            if let Err(error) = crate::cancel::drive(
                runner.as_ref(),
                &journal,
                &meta,
                aex_hands_tools::port::Pgid(pgid),
                &mut pace,
            ) {
                eprintln!(
                    "hands-agent: cancel of {} did not terminalize: {error}",
                    meta.operation.0
                );
            }
        })
        .map(|_| ())
        .map_err(|error| {
            GuestError::internal(format!("the guest cannot drive the cancel ladder: {error}"))
        })
}

/// A journal failure, as a guest HTTP error.
///
/// Never collapsed into "the operation failed": a journal that cannot be read is
/// a guest fault, and reporting it as an operation outcome would let a customer's
/// command look finished when nobody knows whether it ran.
fn journal_error(error: &JournalError) -> GuestError {
    GuestError::internal(error.to_string())
}

/// Encodes one already-typed JSON response.
fn json_bytes_response(payload: Vec<u8>) -> Response {
    (
        StatusCode::OK,
        [(header::CONTENT_TYPE, "application/json")],
        payload,
    )
        .into_response()
}

/// Encodes one bounded protocol error over ordinary HTTP semantics.
fn protocol_error(error: &GuestError) -> Response {
    let payload = serde_json::to_vec(&serde_json::json!({
        "error": error.message.as_str(),
    }))
    .unwrap_or_default();
    (
        error.status,
        [(header::CONTENT_TYPE, "application/json")],
        payload,
    )
        .into_response()
}

/// The guest clock, shared with the background reap and cancel threads.
fn now() -> Timestamp {
    crate::host::now()
}

#[cfg(test)]
mod tests {
    use super::{Guest, HEALTHZ_PATH, READYZ_PATH, router};

    mod attached;
    use crate::execute::Executor;
    use crate::host::{OutputSink, Runner, Started};
    use crate::image::{ImageError, ImageValidator};
    use aex_hands_agent::boot::{RunHook, RunHookBounds};
    use aex_hands_agent::journal::Journal;
    use aex_hands_protocol::operation::{
        DeliveryMode, GuestPath, GuestRoot, OperationBounds, OperationRequest, StopSignal,
        TerminalState,
    };
    use aex_hands_protocol::rpc::{
        AttachResponse, CancelReason, CancelRequest, CancelResponse, Fence, GenerationBinding,
        GuestRequest, GuestResponse, HandsOperationId, PROTOCOL_V1, ResultRequest, ResultResponse,
        StartRequest, StatusRequest, StatusResponse, Verb,
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
        guest_with(dir, runner)
    }

    fn guest_with(dir: &std::path::Path, runner: Arc<dyn Runner>) -> Arc<Guest> {
        let journal = Journal::open(dir).expect("the journal tree is created");
        let executor = Executor::new(runner, GuestRoot::workspace());
        let workspace = dir.join("workspace");
        std::fs::create_dir_all(&workspace).expect("the workspace exists");
        let files = crate::file::FileService::open(GuestRoot::workspace(), workspace, dir)
            .expect("the file store opens");
        Arc::new(Guest::new(journal, executor, files, Arc::new(ValidImage)))
    }

    fn enveloped(_verb: Verb, payload: &[u8], fence: Fence) -> Vec<u8> {
        let request: serde_json::Value =
            serde_json::from_slice(payload).expect("the typed test request decodes");
        serde_json::to_vec(&GuestRequest {
            binding: GenerationBinding {
                schema_version: PROTOCOL_V1,
                generation: generation(),
                fence,
            },
            request,
        })
        .expect("the bounded request envelope encodes")
    }

    fn exec_start(delivery: DeliveryMode) -> StartRequest {
        StartRequest {
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

    fn decode_json<T: serde::de::DeserializeOwned>(body: &[u8]) -> T {
        let envelope: GuestResponse<T> =
            serde_json::from_slice(body).expect("the typed JSON response decodes");
        assert_eq!(envelope.binding.schema_version, PROTOCOL_V1);
        assert_eq!(envelope.binding.generation, generation());
        envelope.response
    }

    #[tokio::test]
    async fn an_unbound_guest_accepts_nothing_and_says_so_without_a_binding() {
        let dir = tempfile::tempdir().expect("a temporary directory");
        let app = router(guest(dir.path(), Arc::new(FakeRunner::default())));
        let (status, _) = post(&app, Verb::Status.path(), Vec::new()).await;
        assert_eq!(
            status,
            StatusCode::SERVICE_UNAVAILABLE,
            "before the run hook the guest has no generation binding"
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
    async fn the_run_hook_binds_the_generation_and_then_the_operation_verbs_answer() {
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
            enveloped(
                Verb::Start,
                &serde_json::to_vec(&start).expect("it serializes"),
                Fence(1),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let _: aex_hands_protocol::rpc::StartResponse = decode_json(&body);
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
            operation: operation(),
        };
        let (status, body) = post(
            &app,
            Verb::Status.path(),
            enveloped(
                Verb::Status,
                &serde_json::to_vec(&query).expect("it serializes"),
                Fence(1),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let answered: StatusResponse = decode_json(&body);
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
            enveloped(
                Verb::Start,
                &serde_json::to_vec(&start).expect("it serializes"),
                Fence(1),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let accepted: aex_hands_protocol::rpc::StartResponse = decode_json(&body);
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
            operation: operation(),
        };
        let mut terminal = None;
        for _ in 0..500 {
            let (status, body) = post(
                &app,
                Verb::Status.path(),
                enveloped(
                    Verb::Status,
                    &serde_json::to_vec(&query).expect("it serializes"),
                    Fence(1),
                ),
            )
            .await;
            assert_eq!(status, StatusCode::OK);
            let answered: StatusResponse = decode_json(&body);
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
            operation: operation(),
            from_offset: 0,
            max_bytes: aex_hands_protocol::rpc::MAX_RESULT_CHUNK_BYTES,
        };
        let (status, body) = post(
            &app,
            Verb::Result.path(),
            enveloped(
                Verb::Result,
                &serde_json::to_vec(&pull).expect("it serializes"),
                Fence(1),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let answered: ResultResponse = decode_json(&body);
        let ResultResponse::Terminal { chunk, .. } = answered else {
            panic!("a terminal operation pulls: {answered:?}");
        };
        let chunk = chunk.expect("the body exists");
        assert_eq!(chunk.bytes, b"hello from the guest");
        assert!(chunk.last);
    }

    // This end-to-end cancellation fixture intentionally keeps the complete
    // request/terminal sequence together so the race remains executable.
    #[allow(clippy::too_many_lines)]
    #[tokio::test]
    async fn a_cancelled_detached_exec_is_escalated_and_terminalizes_cancelled() {
        use std::sync::atomic::{AtomicBool, Ordering};

        /// A group whose reap parks until the cancel ladder signals it.
        struct ParkedRunner {
            release: Mutex<Option<std::sync::mpsc::Sender<()>>>,
            dead: AtomicBool,
        }

        impl Runner for ParkedRunner {
            fn start(
                &self,
                _spec: &aex_hands_tools::command::SpawnSpec,
            ) -> Result<Started, ProcError> {
                let (sender, receiver) = std::sync::mpsc::channel::<()>();
                *self
                    .release
                    .lock()
                    .expect("the fixture lock is not poisoned") = Some(sender);
                Ok(Started {
                    pgid: Pgid(7),
                    start_time: 1,
                    reap: Box::new(move |sink: &mut dyn OutputSink| {
                        // Parked, exactly like a long-running child, until the
                        // cancel ladder signals the group.
                        let _ = receiver.recv();
                        sink.append(b"partial output").map_err(ProcError::Other)?;
                        Ok(aex_hands_protocol::operation::OperationExit::Signal {
                            name: "SIG15".to_owned(),
                        })
                    }),
                })
            }

            fn signal(&self, _group: Pgid, _signal: StopSignal) -> Result<(), ProcError> {
                self.dead.store(true, Ordering::SeqCst);
                if let Some(sender) = self
                    .release
                    .lock()
                    .expect("the fixture lock is not poisoned")
                    .take()
                {
                    let _ = sender.send(());
                }
                Ok(())
            }

            fn alive(&self, _group: Pgid) -> Result<bool, ProcError> {
                Ok(!self.dead.load(Ordering::SeqCst))
            }
        }

        let dir = tempfile::tempdir().expect("a temporary directory");
        let runner = Arc::new(ParkedRunner {
            release: Mutex::new(None),
            dead: AtomicBool::new(false),
        });
        let app = router(guest_with(dir.path(), runner));
        let (status, _) = post(
            &app,
            aex_hands_agent::session::LifecycleHook::Run.path(),
            aws_run_hook_body(),
        )
        .await;
        assert_eq!(status, StatusCode::OK);

        let start = exec_start(DeliveryMode::Detached);
        let (status, _) = post(
            &app,
            Verb::Start.path(),
            enveloped(
                Verb::Start,
                &serde_json::to_vec(&start).expect("it serializes"),
                Fence(1),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::OK);

        let cancel = CancelRequest {
            operation: operation(),
            reason: CancelReason::CustomerStop,
        };
        let (status, body) = post(
            &app,
            Verb::Cancel.path(),
            enveloped(
                Verb::Cancel,
                &serde_json::to_vec(&cancel).expect("it serializes"),
                Fence(1),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let answered: CancelResponse = decode_json(&body);
        assert!(
            matches!(answered, CancelResponse::Cancelling { .. }),
            "{answered:?}"
        );

        // The ladder runs on a background thread; poll until it terminalizes.
        let query = StatusRequest {
            operation: operation(),
        };
        let mut terminal = None;
        for _ in 0..500 {
            let (_, body) = post(
                &app,
                Verb::Status.path(),
                enveloped(
                    Verb::Status,
                    &serde_json::to_vec(&query).expect("it serializes"),
                    Fence(1),
                ),
            )
            .await;
            let answered: StatusResponse = decode_json(&body);
            if let StatusResponse::Terminal {
                terminal: found, ..
            } = answered
            {
                terminal = Some(found);
                break;
            }
            tokio::time::sleep(core::time::Duration::from_millis(10)).await;
        }
        let terminal = terminal.expect("the cancel ladder terminalizes the operation");
        assert_eq!(
            terminal.state,
            TerminalState::Cancelled,
            "whichever of the ladder and the reap lands first, the state is Cancelled"
        );
    }

    #[tokio::test]
    async fn a_request_for_another_generation_is_refused_before_dispatch() {
        let dir = tempfile::tempdir().expect("a temporary directory");
        let app = router(guest(dir.path(), Arc::new(FakeRunner::default())));
        post(
            &app,
            aex_hands_agent::session::LifecycleHook::Run.path(),
            aws_run_hook_body(),
        )
        .await;

        let stranger = GenerationId::from_uuid7(Uuid7::compose(99, [7; 10]));
        let body = serde_json::to_vec(&GuestRequest {
            binding: GenerationBinding {
                schema_version: PROTOCOL_V1,
                generation: stranger,
                fence: Fence(1),
            },
            request: StatusRequest {
                operation: operation(),
            },
        })
        .expect("it serializes");
        let (status, _) = post(&app, Verb::Status.path(), body).await;
        assert_eq!(
            status,
            StatusCode::CONFLICT,
            "the generation binding is checked before operation dispatch"
        );
    }

    #[tokio::test]
    async fn an_older_fence_is_refused_after_a_newer_request_is_observed() {
        let dir = tempfile::tempdir().expect("a temporary directory");
        let app = router(guest(dir.path(), Arc::new(FakeRunner::default())));
        post(
            &app,
            aex_hands_agent::session::LifecycleHook::Run.path(),
            aws_run_hook_body(),
        )
        .await;
        let request = serde_json::to_vec(&StatusRequest {
            operation: operation(),
        })
        .expect("it serializes");

        let (fresh, _) = post(
            &app,
            Verb::Status.path(),
            enveloped(Verb::Status, &request, Fence(8)),
        )
        .await;
        assert_eq!(fresh, StatusCode::OK);
        let (stale, _) = post(
            &app,
            Verb::Status.path(),
            enveloped(Verb::Status, &request, Fence(7)),
        )
        .await;
        assert_eq!(stale, StatusCode::CONFLICT);
    }

    #[tokio::test]
    async fn the_http_adapter_rejects_a_body_over_the_protocol_limit() {
        let dir = tempfile::tempdir().expect("a temporary directory");
        let app = router(guest(dir.path(), Arc::new(FakeRunner::default())));
        post(
            &app,
            aex_hands_agent::session::LifecycleHook::Run.path(),
            aws_run_hook_body(),
        )
        .await;

        let (status, _) = post(
            &app,
            Verb::Status.path(),
            vec![b' '; aex_hands_protocol::rpc::MAX_GUEST_BODY_BYTES + 1],
        )
        .await;
        assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE);
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
